//! Built-in tools with permission enforcement — the leash.
//!
//! The agent can only do what you granted. Its own vault is always
//! readable/writable; other folders, the shell, the web, and command
//! execution are all gated by config (`read_paths`, `write_paths`, and
//! the per-tool switches). Leashed = defaults deny everything but its own
//! data dir; Unleashed = the leash comes off: folder scopes are dropped
//! AND every ability turns on (the switches resume only when re-leashed).
//! Denied calls return "DENIED" as the tool result — the run continues,
//! the refusal is logged and reported.

use crate::chatlog::ChatLog;
use crate::config::Config;
use crate::models::{ToolFn, ToolSpec};
use crate::paths::Paths;
use crate::util::url_encode;
use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct ToolCtx<'a> {
    pub cfg: &'a Config,
    pub paths: &'a Paths,
    pub vectors: &'a crate::vector::Vectors,
    pub http: ureq::Agent,
    /// Live MCP tool servers (Docker containers, stdio-only) spawned for
    /// this run by src/store.rs. Empty unless servers are registered
    /// (via @store), enabled, and Docker is available.
    pub mcp: Vec<crate::store::McpChild>,
    /// When a research profile is active (@research <name>.yml), web
    /// tools are locked to these sites (hosts or URLs). Empty = free.
    pub research_sites: Vec<String>,
}

pub fn specs(cfg: &Config) -> Vec<ToolSpec> {
    let mut out = vec![
        tool_spec("memory_search", "Search the agent's RAG memory (docs you gave it, past learnings, past chat logs) for relevant knowledge.", json_obj(&[("query", "string")])),
        tool_spec("remember", "Store a lasting learning into memory. Use whenever something new and useful was learned — keep it under 1200 chars, one focused fact/skill per call.", json_obj(&[("knowledge", "string")])),
        tool_spec("list_wiki", "List pages of the agent's memory wiki (markdown).", json_obj(&[])),
        tool_spec("read_wiki", "Read one page of the agent's memory wiki.", json_obj(&[("name", "string")])),
        tool_spec("write_wiki", "Write or update a page in the agent's memory wiki, e.g. memory-<topic>. Content is markdown.", json_obj(&[("name", "string"), ("content", "string")])),
    ];
    if cfg.tool_read_files || cfg.is_unleashed() {
        out.push(tool_spec(
            "list_dir",
            "List files in a directory.",
            json_obj(&[("path", "string")]),
        ));
        out.push(tool_spec(
            "read_file",
            "Read a text file.",
            json_obj(&[("path", "string")]),
        ));
    }
    if cfg.tool_write_files || cfg.is_unleashed() {
        out.push(tool_spec(
            "write_file",
            "Write a text file.",
            json_obj(&[("path", "string"), ("content", "string")]),
        ));
    }
    if cfg.tool_web_search || cfg.is_unleashed() {
        out.push(tool_spec(
            "web_search",
            "Search the web (DuckDuckGo results + Wikipedia + Hacker News) for research. Returns titles, URLs and snippets; use web_fetch on the best URL to read the page.",
            json_obj(&[("query", "string")]),
        ));
        out.push(tool_spec(
            "web_fetch",
            "Fetch a web page and return its readable text (for reading a search result in full).",
            json_obj(&[("url", "string")]),
        ));
    }
    if cfg.tool_run_command || cfg.is_unleashed() {
        out.push(tool_spec("run_command", "Run a shell command and return output. Use sparingly; read docs/man pages first when a tool is new.", json_obj(&[("command", "string")])));
    }
    out
}

fn tool_spec(
    name: &'static str,
    description: &'static str,
    parameters: serde_json::Value,
) -> ToolSpec {
    ToolSpec {
        typ: "function",
        function: ToolFn {
            name: name.into(),
            description: description.into(),
            parameters,
        },
    }
}

fn json_obj(props: &[(&str, &str)]) -> serde_json::Value {
    let props: serde_json::Map<String, serde_json::Value> = props
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!({ "type": v })))
        .collect();
    serde_json::json!({
        "type": "object",
        "properties": props,
        "required": props.keys().collect::<Vec<_>>(),
    })
}

// ---------------- permission checks ----------------

fn under_any(path: &PathBuf, roots: &[String]) -> bool {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
    for root in roots {
        let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
        if canon.starts_with(&root_canon) {
            return true;
        }
    }
    false
}

fn may_read(cfg: &Config, paths: &Paths, path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path);
    let inside_own = under_any(&p, &[paths.staging.to_string_lossy().to_string()]);
    let granted = cfg.is_unleashed() || inside_own || under_any(&p, &cfg.read_paths);
    if !granted {
        return Err(anyhow!(
            "DENIED: read access to '{path}' is not granted (leash: {}). Grant it with: /grants read {path}",
            cfg.leash
        ));
    }
    Ok(p)
}

fn may_write(cfg: &Config, paths: &Paths, path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path);
    let inside_own = under_any(&p, &[paths.staging.to_string_lossy().to_string()]);
    let granted = cfg.is_unleashed() || inside_own || under_any(&p, &cfg.write_paths);
    if !granted {
        return Err(anyhow!(
            "DENIED: write access to '{path}' is not granted (leash: {}). Grant it with: /grants write {path}",
            cfg.leash
        ));
    }
    Ok(p)
}

// ---------------- execution ----------------

pub fn run(ctx: &mut ToolCtx, log: &mut ChatLog, name: &str, args_json: &str) -> String {
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    let get = |k: &str| {
        args.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    log.log("tool_call", &format!("{name} {args_json}"));

    let result = match name {
        "memory_search" => {
            let q = get("query");
            let hits = ctx.vectors.search(&q, 5).unwrap_or_default();
            if hits.is_empty() {
                "No relevant memory found.".to_string()
            } else {
                hits.iter()
                    .map(|h| format!("[{} | {} | {:.2}] {}", h.kind, h.source, h.score, h.content))
                    .collect::<Vec<_>>()
                    .join("\n---\n")
            }
        }
        "remember" => {
            let knowledge = get("knowledge");
            match ctx.vectors.insert_text("learning", "learned", &knowledge) {
                Ok((total, added)) => format!("Stored to memory: {added}/{total} chunks."),
                Err(e) => format!("Failed to store learning: {e}"),
            }
        }
        "list_wiki" => match crate::wiki::list(ctx.paths) {
            Ok(pages) if pages.is_empty() => "(wiki is empty)".to_string(),
            Ok(pages) => pages.join(", "),
            Err(e) => format!("wiki error: {e}"),
        },
        "read_wiki" => match crate::wiki::read(ctx.paths, &get("name")) {
            Ok(body) => body,
            Err(e) => e.to_string(),
        },
        "write_wiki" => match crate::wiki::write(ctx.paths, &get("name"), &get("content")) {
            Ok(n) => format!("wiki page '{n}' written"),
            Err(e) => e.to_string(),
        },
        "list_dir" => match may_read(ctx.cfg, ctx.paths, &get("path")) {
            Ok(p) => list_dir(&p),
            Err(e) => e.to_string(),
        },
        "read_file" => match may_read(ctx.cfg, ctx.paths, &get("path")) {
            Ok(p) => read_file(&p),
            Err(e) => e.to_string(),
        },
        "write_file" => match may_write(ctx.cfg, ctx.paths, &get("path")) {
            Ok(p) => std::fs::create_dir_all(p.parent().unwrap_or(&p))
                .ok()
                .map(|_| ())
                .and_then(|_| std::fs::write(&p, get("content")).ok())
                .map(|_| format!("wrote {}", p.display()))
                .unwrap_or_else(|| "write failed".to_string()),
            Err(e) => e.to_string(),
        },
        "web_search" => web_search(ctx, &get("query")),
        "web_fetch" => web_fetch(ctx, &get("url")),
        "run_command" => {
            let already_granted = ctx.cfg.tool_run_command || ctx.cfg.is_unleashed();
            if !already_granted {
                "DENIED: command execution is off. Enable it in setup: /setup".to_string()
            } else if ctx.cfg.is_unleashed() {
                run_command(&get("command"))
            } else {
                // Leashed + run_command enabled: confirm each command.
                let theme = dialoguer::theme::ColorfulTheme::default();
                let ok = dialoguer::Confirm::with_theme(&theme)
                    .with_prompt(format!("run: {} ?", get("command")))
                    .default(false)
                    .interact()
                    .unwrap_or(false);
                if ok {
                    run_command(&get("command"))
                } else {
                    "User declined this command.".to_string()
                }
            }
        }
        _ => {
            // MCP tool servers registered via @store: `mcp_<server>_<tool>`.
            if name.starts_with(crate::store::MCP_TOOL_PREFIX) {
                crate::store::dispatch(ctx, name, args_json)
            } else {
                format!("UNKNOWN TOOL: {name}")
            }
        }
    };

    log.log(
        "tool_result",
        &format!("{name} -> {}", truncate(&result, 4000)),
    );
    result
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}…", &s[..max])
    } else {
        s.to_string()
    }
}

fn list_dir(p: &PathBuf) -> String {
    match std::fs::read_dir(p) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .flatten()
                .map(|e| {
                    let suffix = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        "/"
                    } else {
                        ""
                    };
                    format!("{}{}", e.file_name().to_string_lossy(), suffix)
                })
                .collect();
            names.sort();
            names.join("\n")
        }
        Err(e) => format!("cannot list {}: {e}", p.display()),
    }
}

fn read_file(p: &PathBuf) -> String {
    const MAX: u64 = 256 * 1024;
    match std::fs::metadata(p) {
        Ok(m) if m.len() > MAX => return format!("file too large ({} bytes)", m.len()),
        _ => {}
    }
    match std::fs::read(p) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => format!("cannot read {}: {e}", p.display()),
    }
}

pub fn run_command(cmd: &str) -> String {
    let mut child = match std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("failed to spawn shell: {e}"),
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                let mut text = format!("exit: {}\n{}{}", status, out, err);
                if text.len() > 8000 {
                    text.truncate(8000);
                    text.push_str("…(output truncated)");
                }
                return text;
            }
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    return "command timed out after 120s and was killed".to_string();
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return format!("wait failed: {e}"),
        }
    }
}

/// One web search hit.
struct WebHit {
    url: String,
    title: String,
    snippet: String,
}

/// Host of a URL ("https://a.b/x" -> "a.b"); bare domains pass through.
fn host_of(entry: &str) -> String {
    let e = entry.trim().to_lowercase();
    let e = e
        .strip_prefix("https://")
        .or_else(|| e.strip_prefix("http://"))
        .unwrap_or(&e);
    e.split('/').next().unwrap_or("").to_string()
}

/// True when `url` belongs to `site` (host match, subdomains included).
fn url_in_site(url: &str, site: &str) -> bool {
    let host = host_of(url);
    let site = host_of(site);
    !host.is_empty() && !site.is_empty() && (host == site || host.ends_with(&format!(".{site}")))
}

/// Percent-decode (for DDG redirect links).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Decode the common HTML entities (built at runtime so the source
/// stays free of literal entity text).
fn clean_text(s: &str) -> String {
    let e_amp = format!("{}amp;", '&');
    let e_quot = format!("{}quot;", '&');
    let e_lt = format!("{}lt;", '&');
    let e_gt = format!("{}gt;", '&');
    s.replace(&e_amp, "&")
        .replace(&e_quot, "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace(&e_lt, "<")
        .replace(&e_gt, ">")
        .trim()
        .to_string()
}

/// Extract (href, inner_text) pairs for anchors carrying `class="cls"`.
fn anchors_with_class(body: &str, cls: &str) -> Vec<(String, String)> {
    let needle = format!("class=\"{cls}\"");
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = body[pos..].find(&needle) {
        let at = pos + rel;
        let tag_start = body[..at].rfind('<').unwrap_or(0);
        let Some(tag_end_rel) = body[at..].find('>') else {
            break;
        };
        let tag_end = at + tag_end_rel;
        let tag = &body[tag_start..=tag_end];
        let href = tag
            .find("href=\"")
            .map(|h| {
                let rest = &tag[h + 6..];
                rest[..rest.find('"').unwrap_or(rest.len())].to_string()
            })
            .unwrap_or_default();
        let inner_end = body[tag_end + 1..]
            .find("</a>")
            .map(|e| tag_end + 1 + e)
            .unwrap_or(body.len());
        let inner = clean_text(&body[tag_end + 1..inner_end.min(body.len())]);
        out.push((href, inner));
        pos = inner_end.min(body.len());
    }
    out
}

/// Resolve a DDG href: unwrap the `uddg=` redirect, scheme-fix `//`.
fn resolve_href(href: &str) -> String {
    if let Some(u) = href.find("uddg=") {
        let rest = &href[u + 5..];
        let enc = rest.split('&').next().unwrap_or(rest);
        return url_decode(enc);
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

/// Read a response body into a String, capped at `cap` bytes.
fn read_body_capped(resp: ureq::Response, cap: u64) -> Option<String> {
    let mut s = String::new();
    resp.into_reader().take(cap).read_to_string(&mut s).ok()?;
    Some(s)
}

/// Real web results via DuckDuckGo's HTML endpoint (keyless). The old
/// Instant Answers API returns nothing for most queries — this returns
/// actual result links and snippets. Some datacenter IPs get blocked;
/// callers layer keyless native APIs on top for exactly that case.
fn ddg_html_search(agent: &ureq::Agent, query: &str) -> Vec<WebHit> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", url_encode(query));
    let resp = agent
        .get(&url)
        .timeout(Duration::from_secs(20))
        .set(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) DeCypherTek/0.1",
        )
        .call();
    let body = match resp {
        Ok(r) => read_body_capped(r, 2_000_000).unwrap_or_default(),
        Err(_) => return Vec::new(),
    };
    let titles = anchors_with_class(&body, "result__a");
    let snippets = anchors_with_class(&body, "result__snippet");
    titles
        .into_iter()
        .enumerate()
        .map(|(i, (href, title))| WebHit {
            url: resolve_href(&href),
            title,
            snippet: snippets.get(i).map(|(_, s)| s.clone()).unwrap_or_default(),
        })
        .filter(|h| h.url.starts_with("http"))
        .collect()
}

/// Wikipedia search (keyless, reliable everywhere).
fn wikipedia_search(agent: &ureq::Agent, query: &str, limit: usize) -> Vec<WebHit> {
    let wiki = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&format=json&srlimit={}",
        url_encode(query),
        limit
    );
    let resp = agent
        .get(&wiki)
        .timeout(Duration::from_secs(15))
        .set("User-Agent", "DeCypherTek/0.1")
        .call();
    let body = match resp {
        Ok(r) => read_body_capped(r, 500_000).unwrap_or_default(),
        Err(_) => return Vec::new(),
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Vec::new();
    };
    let Some(hits) = v.pointer("/query/search").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    hits.iter()
        .map(|h| {
            let title = h.get("title").and_then(|x| x.as_str()).unwrap_or("");
            let snippet = h
                .get("snippet")
                .and_then(|x| x.as_str())
                .map(|s| s.replace(['&', '<', '>'], ""))
                .unwrap_or_default();
            WebHit {
                url: format!(
                    "https://en.wikipedia.org/wiki/{}",
                    url_encode(&title.replace(' ', "_"))
                ),
                title: format!("wikipedia: {title}"),
                snippet,
            }
        })
        .take(limit)
        .collect()
}

/// Hacker News via the keyless Algolia API (news.ycombinator.com).
fn hn_search(agent: &ureq::Agent, query: &str, limit: usize) -> Vec<WebHit> {
    let url = format!(
        "https://hn.algolia.com/api/v1/search?query={}&hitsPerPage={}",
        url_encode(query),
        limit
    );
    let resp = agent.get(&url).timeout(Duration::from_secs(15)).call();
    let body = match resp {
        Ok(r) => read_body_capped(r, 500_000).unwrap_or_default(),
        Err(_) => return Vec::new(),
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Vec::new();
    };
    let Some(hits) = v.get("hits").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    hits.iter()
        .filter_map(|h| {
            let title = h.get("title").and_then(|x| x.as_str())?;
            let link = h
                .get("url")
                .and_then(|x| x.as_str())
                .filter(|u| u.starts_with("http"))
                .map(|u| u.to_string())
                .unwrap_or_else(|| {
                    format!(
                        "https://news.ycombinator.com/item?id={}",
                        h.get("objectID").and_then(|x| x.as_str()).unwrap_or("")
                    )
                });
            let snippet = format!(
                "{} points, {} comments — discussion on Hacker News",
                h.get("points").and_then(|x| x.as_i64()).unwrap_or(0),
                h.get("num_comments").and_then(|x| x.as_i64()).unwrap_or(0),
            );
            Some(WebHit {
                url: link,
                title: title.to_string(),
                snippet,
            })
        })
        .collect()
}

/// Text between the first `<tag>` and matching `</tag>` in an XML/Atom
/// blob (arXiv responses are small; hand-parsing keeps the binary
/// dependency-free).
fn xml_tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let a = xml.find(&open)?;
    let rest = &xml[a..];
    let content_start = rest.find('>')? + 1;
    let close = format!("</{tag}>");
    let end_rel = rest[content_start..].find(&close)?;
    Some(
        rest[content_start..content_start + end_rel]
            .trim()
            .to_string(),
    )
}

/// arXiv via its keyless export API (arxiv.org).
fn arxiv_search(agent: &ureq::Agent, query: &str, limit: usize) -> Vec<WebHit> {
    let url = format!(
        "https://export.arxiv.org/api/query?search_query=all:{}&max_results={}",
        url_encode(&query.replace(' ', "+")),
        limit
    );
    let resp = agent
        .get(&url)
        .timeout(Duration::from_secs(20))
        .set("User-Agent", "DeCypherTek/0.1")
        .call();
    let body = match resp {
        Ok(r) => read_body_capped(r, 1_000_000).unwrap_or_default(),
        Err(_) => return Vec::new(),
    };
    body.split("<entry>")
        .skip(1)
        .filter_map(|e| {
            let title = xml_tag_text(e, "title")?;
            let id = xml_tag_text(e, "id")?;
            let summary = xml_tag_text(e, "summary").unwrap_or_default();
            let snippet: String = summary.split_whitespace().collect::<Vec<_>>().join(" ");
            let snippet = if snippet.len() > 300 {
                let mut end = 300;
                while !snippet.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}…", &snippet[..end])
            } else {
                snippet
            };
            Some(WebHit {
                url: id,
                title,
                snippet,
            })
        })
        .collect()
}

/// Keyless web research, multi-source. With a research profile active
/// (@research <name>.yml), searching is locked to that profile's sites:
/// `site:`-scoped queries plus the native APIs of sites that have one
/// (arxiv.org, news.ycombinator.com, wikipedia.org), all filtered to
/// the profile's hosts. Without a profile it is a general search:
/// DuckDuckGo results + Wikipedia + Hacker News.
pub fn web_search(ctx: &ToolCtx, query: &str) -> String {
    let mut hits: Vec<WebHit> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut note = String::new();

    fn push(hits: &mut Vec<WebHit>, seen: &mut Vec<String>, allowed: &[String], h: WebHit) {
        if !allowed.is_empty() && !allowed.iter().any(|s| url_in_site(&h.url, s)) {
            return;
        }
        if h.url.starts_with("http") && !seen.contains(&h.url) {
            seen.push(h.url.clone());
            hits.push(h);
        }
    }

    if !ctx.research_sites.is_empty() {
        note = format!(
            "[research profile active — results restricted to: {}]\n\n",
            ctx.research_sites.join(", ")
        );
        // Native APIs first — keyless and reliable for their hosts.
        for site in &ctx.research_sites {
            let host = host_of(site);
            if host == "arxiv.org" || host.ends_with(".arxiv.org") {
                for h in arxiv_search(&ctx.http, query, 3) {
                    push(&mut hits, &mut seen, &ctx.research_sites, h);
                }
            } else if host.contains("ycombinator") {
                for h in hn_search(&ctx.http, query, 3) {
                    push(&mut hits, &mut seen, &ctx.research_sites, h);
                }
            } else if host == "wikipedia.org" || host.ends_with(".wikipedia.org") {
                for h in wikipedia_search(&ctx.http, query, 3) {
                    push(&mut hits, &mut seen, &ctx.research_sites, h);
                }
            }
        }
        // site:-scoped queries for the listed sites (best-effort; the
        // HTML endpoint refuses some datacenter IPs but works from
        // typical user connections).
        for site in ctx.research_sites.iter().take(4) {
            for h in ddg_html_search(&ctx.http, &format!("site:{} {}", host_of(site), query)) {
                push(&mut hits, &mut seen, &ctx.research_sites, h);
            }
        }
        if hits.is_empty() {
            return format!(
                "{note}(no results on the profile's sites — broaden the profile or the query)\n"
            );
        }
    } else {
        // General search: DuckDuckGo results, then the reliable keyless
        // sources. If DDG blocks this network, the others still answer.
        for h in ddg_html_search(&ctx.http, query) {
            push(&mut hits, &mut seen, &[], h);
        }
        let ddg_count = hits.len();
        for h in wikipedia_search(&ctx.http, query, 3) {
            push(&mut hits, &mut seen, &[], h);
        }
        for h in hn_search(&ctx.http, query, 3) {
            push(&mut hits, &mut seen, &[], h);
        }
        if hits.is_empty() {
            return "No results found (or network unreachable).".to_string();
        }
        if ddg_count == 0 {
            note = "(DuckDuckGo results unavailable from this network — answering from Wikipedia + Hacker News)\n\n".to_string();
        }
    }

    let mut results = note;
    for h in hits.iter().take(9) {
        results.push_str(&format!(
            "[{}] {}\n  {}\n  {}\n\n",
            host_of(&h.url),
            h.title,
            h.snippet,
            h.url
        ));
    }
    results
}

/// Readable text of a web page. Under a research profile, only the
/// profile's sites may be fetched — the search was restricted, so the
/// reading is too.
pub fn web_fetch(ctx: &ToolCtx, url: &str) -> String {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return "DENIED: only http(s) URLs can be fetched.".to_string();
    }
    if !ctx.research_sites.is_empty() && !ctx.research_sites.iter().any(|s| url_in_site(url, s)) {
        return format!(
            "DENIED: the active research profile only allows fetching from: {}",
            ctx.research_sites.join(", ")
        );
    }
    let resp = ctx
        .http
        .get(url)
        .timeout(Duration::from_secs(20))
        .set(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) DeCypherTek/0.1",
        )
        .call();
    let mut body = match resp {
        Ok(r) => read_body_capped(r, 1_000_000).unwrap_or_default(),
        Err(e) => return format!("fetch failed: {e}"),
    };
    // Strip non-content blocks, then tags, then collapse whitespace.
    for tag in ["script", "style", "nav", "header", "footer"] {
        loop {
            let open = format!("<{tag}");
            let close = format!("</{tag}>");
            let Some(a) = body.find(&open) else { break };
            let Some(b) = body[a..].find(&close) else {
                break;
            };
            body.replace_range(a..a + b + close.len(), " ");
        }
    }
    let mut text = String::with_capacity(body.len());
    let mut in_tag = false;
    for c in body.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = clean_text(&text);
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() > 8000 {
        let mut end = 8000;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n…(truncated)", &text[..end])
    } else if text.is_empty() {
        "page returned no readable text".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::paths::Paths;
    use crate::vector::Vectors;
    use serde_json::json;

    fn tmp(tag: &str) -> (Paths, Config) {
        let root = std::env::temp_dir().join(format!(
            "dct-tools-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths::new(&root);
        paths.ensure_staging().unwrap();
        let cfg = Config::default();
        (paths, cfg)
    }

    fn ctx<'a>(paths: &'a Paths, cfg: &'a Config, vectors: &'a Vectors) -> ToolCtx<'a> {
        ToolCtx {
            cfg,
            paths,
            vectors,
            http: ureq::AgentBuilder::new().build(),
            mcp: Vec::new(),
            research_sites: Vec::new(),
        }
    }

    #[test]
    fn leashed_read_outside_grants_is_denied() {
        let (paths, mut cfg) = tmp("leashed-deny");
        cfg.read_paths = vec![];
        let vectors = Vectors::open(&paths.vector_db).unwrap();
        let mut c = ctx(&paths, &cfg, &vectors);
        let mut log = ChatLog::new(&paths).unwrap();
        let out = run(
            &mut c,
            &mut log,
            "read_file",
            &json!({ "path": "/etc/hostname" }).to_string(),
        );
        assert!(out.contains("DENIED"), "leashed read must be DENIED: {out}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn own_data_is_always_readable() {
        let (paths, cfg) = tmp("own");
        let secret = paths.staging.join("memory").join("s.txt");
        std::fs::write(&secret, "own data").unwrap();
        let vectors = Vectors::open(&paths.vector_db).unwrap();
        let mut log = ChatLog::new(&paths).unwrap();
        let mut c = ctx(&paths, &cfg, &vectors);
        let out = run(
            &mut c,
            &mut log,
            "read_file",
            &json!({ "path": secret.to_string_lossy() }).to_string(),
        );
        assert_eq!(out, "own data");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn write_outside_grants_is_denied_unleashed_allowed() {
        let (paths, mut cfg) = tmp("write");
        let victim = paths.root.join("outside.txt");
        let vectors = Vectors::open(&paths.vector_db).unwrap();
        let mut log = ChatLog::new(&paths).unwrap();

        cfg.write_paths = vec![];
        let mut c = ctx(&paths, &cfg, &vectors);
        let out = run(
            &mut c,
            &mut log,
            "write_file",
            &json!({ "path": victim.to_string_lossy(), "content": "x" }).to_string(),
        );
        assert!(out.contains("DENIED"), "{out}");
        assert!(!victim.exists());

        cfg.leash = "unleashed".into();
        let mut c = ctx(&paths, &cfg, &vectors);
        let out = run(
            &mut c,
            &mut log,
            "write_file",
            &json!({ "path": victim.to_string_lossy(), "content": "x" }).to_string(),
        );
        assert!(out.contains("wrote"), "{out}");
        assert!(victim.exists());
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn run_command_tool_off_is_denied() {
        let (paths, cfg) = tmp("cmd");
        let vectors = Vectors::open(&paths.vector_db).unwrap();
        let mut log = ChatLog::new(&paths).unwrap();
        let mut c = ctx(&paths, &cfg, &vectors);
        let out = run(
            &mut c,
            &mut log,
            "run_command",
            &json!({ "command": "echo hi" }).to_string(),
        );
        assert!(out.contains("DENIED"), "{out}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn unleashed_offers_every_tool_spec() {
        let (paths, mut cfg) = tmp("unleash-specs");
        cfg.tool_web_search = false;
        cfg.tool_read_files = false;
        cfg.tool_write_files = false;
        cfg.tool_run_command = false;

        let leashed: Vec<String> = specs(&cfg)
            .iter()
            .map(|s| s.function.name.clone())
            .collect();
        assert!(
            !leashed
                .iter()
                .any(|n| n == "read_file" || n == "run_command"),
            "leashed must honor the switches: {leashed:?}"
        );

        cfg.leash = "unleashed".into();
        let off: Vec<String> = specs(&cfg)
            .iter()
            .map(|s| s.function.name.clone())
            .collect();
        for want in [
            "list_dir",
            "read_file",
            "write_file",
            "web_search",
            "web_fetch",
            "run_command",
        ] {
            assert!(
                off.contains(&want.to_string()),
                "unleashed must offer {want}: {off:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn unleashed_runs_command_even_when_the_switch_is_off() {
        let (paths, mut cfg) = tmp("unleash-cmd");
        cfg.tool_run_command = false;
        cfg.leash = "unleashed".into();
        let vectors = Vectors::open(&paths.vector_db).unwrap();
        let mut log = ChatLog::new(&paths).unwrap();
        let mut c = ctx(&paths, &cfg, &vectors);
        let out = run(
            &mut c,
            &mut log,
            "run_command",
            &json!({ "command": "echo dct-unleashed-ok" }).to_string(),
        );
        assert!(!out.contains("DENIED"), "{out}");
        assert!(out.contains("dct-unleashed-ok"), "{out}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn remember_and_search_roundtrip() {
        let (paths, cfg) = tmp("mem");
        let vectors = Vectors::open(&paths.vector_db).unwrap();
        let mut log = ChatLog::new(&paths).unwrap();
        let mut c = ctx(&paths, &cfg, &vectors);
        let out = run(
            &mut c,
            &mut log,
            "remember",
            &json!({ "knowledge": "the decoy server lives at 10.0.0.9" }).to_string(),
        );
        assert!(out.contains("Stored"), "{out}");
        let out = run(
            &mut c,
            &mut log,
            "memory_search",
            &json!({ "query": "where is the decoy server" }).to_string(),
        );
        assert!(out.contains("10.0.0.9"), "recall failed: {out}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn unknown_tool_reports_cleanly() {
        let (paths, cfg) = tmp("unknown");
        let vectors = Vectors::open(&paths.vector_db).unwrap();
        let mut log = ChatLog::new(&paths).unwrap();
        let mut c = ctx(&paths, &cfg, &vectors);
        let out = run(&mut c, &mut log, "teleport", "{}");
        assert!(out.contains("UNKNOWN TOOL"), "{out}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }
}
