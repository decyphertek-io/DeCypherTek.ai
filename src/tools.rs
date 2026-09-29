//! Built-in tools with permission enforcement — the leash.
//!
//! The agent can only do what you granted. Its own vault is always
//! readable/writable; other folders, the shell, the web, and command
//! execution are all gated by config (`read_paths`, `write_paths`, and
//! the per-tool switches). Leashed = defaults deny everything but its own
//! data dir; Unleashed = the leash comes off (folder scopes are dropped,
//! though tool switches still apply). Denied calls return "DENIED" as the
//! tool result — the run continues, the refusal is logged and reported.

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
}

pub fn specs(cfg: &Config) -> Vec<ToolSpec> {
    let mut out = vec![
        tool_spec("memory_search", "Search the agent's RAG memory (docs you gave it, past learnings, past chat logs) for relevant knowledge.", json_obj(&[("query", "string")])),
        tool_spec("remember", "Store a lasting learning into memory. Use whenever something new and useful was learned — keep it under 1200 chars, one focused fact/skill per call.", json_obj(&[("knowledge", "string")])),
        tool_spec("list_wiki", "List pages of the agent's memory wiki (markdown).", json_obj(&[])),
        tool_spec("read_wiki", "Read one page of the agent's memory wiki.", json_obj(&[("name", "string")])),
        tool_spec("write_wiki", "Write or update a page in the agent's memory wiki, e.g. memory-<topic>. Content is markdown.", json_obj(&[("name", "string"), ("content", "string")])),
    ];
    if cfg.tool_read_files {
        out.push(tool_spec(
            "list_dir",
            "List files in a directory (requires granted read access).",
            json_obj(&[("path", "string")]),
        ));
        out.push(tool_spec(
            "read_file",
            "Read a text file (requires granted read access).",
            json_obj(&[("path", "string")]),
        ));
    }
    if cfg.tool_write_files {
        out.push(tool_spec(
            "write_file",
            "Write a text file (only inside granted write folders).",
            json_obj(&[("path", "string"), ("content", "string")]),
        ));
    }
    if cfg.tool_web_search {
        out.push(tool_spec(
            "web_search",
            "Search the web (DuckDuckGo + Wikipedia) for research.",
            json_obj(&[("query", "string")]),
        ));
    }
    if cfg.tool_run_command {
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
            "DENIED: read access to '{path}' is not granted (leash: {}). Grant it with: @grants read {path}",
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
            "DENIED: write access to '{path}' is not granted (leash: {}). Grant it with: @grants write {path}",
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
        "web_search" => web_search(&ctx.http, &get("query")),
        "run_command" => {
            let already_granted = ctx.cfg.tool_run_command;
            if !already_granted {
                "DENIED: command execution is off. Enable it in setup: @setup".to_string()
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

/// Keyless web research: DuckDuckGo Instant Answers + Wikipedia search.
pub fn web_search(agent: &ureq::Agent, query: &str) -> String {
    let mut results = String::new();

    // DuckDuckGo Instant Answer.
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        url_encode(query)
    );
    let try_ddg = agent
        .get(&url)
        .timeout(Duration::from_secs(15))
        .set("User-Agent", "DeCypherTek/0.1")
        .call();
    if let Ok(resp) = try_ddg {
        if let Ok(v) = serde_json::from_reader::<_, serde_json::Value>(resp.into_reader()) {
            let heading = v.get("Heading").and_then(|x| x.as_str()).unwrap_or("");
            let abstract_text = v.get("AbstractText").and_then(|x| x.as_str()).unwrap_or("");
            let answer = v.get("Answer").and_then(|x| x.as_str()).unwrap_or("");
            if !abstract_text.is_empty() {
                results.push_str(&format!("[duckduckgo: {heading}] {abstract_text}\n\n"));
            }
            if !answer.is_empty() {
                results.push_str(&format!("[duckduckgo answer] {answer}\n\n"));
            }
            if let Some(topics) = v.get("RelatedTopics").and_then(|t| t.as_array()) {
                for t in topics.iter().take(3) {
                    if let Some(txt) = t.get("Text").and_then(|x| x.as_str()) {
                        results.push_str(&format!("[duckduckgo related] {txt}\n\n"));
                    }
                }
            }
        }
    }

    // Wikipedia search.
    let wiki = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&format=json&srlimit=5",
        url_encode(query)
    );
    let try_wiki = agent
        .get(&wiki)
        .timeout(Duration::from_secs(15))
        .set("User-Agent", "DeCypherTek/0.1")
        .call();
    if let Ok(resp) = try_wiki {
        if let Ok(v) = serde_json::from_reader::<_, serde_json::Value>(resp.into_reader()) {
            if let Some(hits) = v.pointer("/query/search").and_then(|x| x.as_array()) {
                for h in hits {
                    let title = h.get("title").and_then(|x| x.as_str()).unwrap_or("");
                    let let_snippet = h
                        .get("snippet")
                        .and_then(|x| x.as_str())
                        .map(|s| s.replace(['&', '<', '>'], ""))
                        .unwrap_or_default();
                    results.push_str(&format!("[wikipedia: {title}] {let_snippet}\n\n"));
                }
            }
        }
    }

    if results.is_empty() {
        "No results found (or network unreachable).".to_string()
    } else {
        results
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
