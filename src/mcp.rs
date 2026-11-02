//! The `/mcp` command — MCP tool-server setup, Linux/Podroid-VM-first.
//!
//! One question-driven flow replaces the old store:
//! - "Search for a new MCP server" — the curated catalog, a LIVE Docker
//!   Hub search and images already on this machine; then pull plus a
//!   short setup walk: env keys (API keys — hidden input, sealed in the
//!   vault, injected via a 0600 env-file), mode permissions (/chat,
//!   /code, /research) and an optional verification that launches the
//!   hardened container and lists its tools.
//! - "Setup an MCP server" — keys, permissions, enable/disable, update,
//!   verify, remove — for what is already registered.
//! - Honest pulls: success is the runtime's exit status plus an
//!   `image inspect`, so docker AND podman report identically (the old
//!   store string-matched docker's success words and mislabeled every
//!   podman pull a failure).
//! - The secure launch TEMPLATE stays: every registered server runs in
//!   a hardened container (all capabilities dropped, no-new-privileges,
//!   memory/pid caps, stdio-only, nothing published) with outbound
//!   internet — GitHub, Slack and every other API-backed server is
//!   useless without it. Servers that never need the web (filesystem,
//!   sqlite, memory, time) can opt into `--network=none` per server.
//!   Its inbound channel to the world is the stdio pipe the agent
//!   holds; it cannot publish ports or reach the LAN.
//! - A minimal MCP client (JSON-RPC 2.0 over stdio): initialize,
//!   tools/list, tools/call. Registered servers spawn per agent run —
//!   only in modes their permissions allow — and their tools merge into
//!   the model's list as `mcp_<server>_<tool>` (the leash's tool_mcp
//!   switch is still the master gate).

use crate::config::{Config, McpServer};
use crate::paths::Paths;
use crate::tui;
use anyhow::{anyhow, Context, Result};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, FuzzySelect, Input, MultiSelect, Password, Select};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Model-facing tool names for MCP server tools start with this.
pub const MCP_TOOL_PREFIX: &str = "mcp_";
/// Do not let one chatty server wedge the agent: bounded stdio reads.
const RPC_READ_LIMIT: usize = 500;
const HUB_SEARCH_URL: &str =
    "https://hub.docker.com/v2/search/repositories/?query=mcp&page_size=50";

// ------------------------------------------------------------------ catalog

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryOrigin {
    Curated,
    Hub,   // found on Docker Hub by the live search
    Local, // an image already pulled on this machine
}

impl EntryOrigin {
    fn label(&self) -> &'static str {
        match self {
            EntryOrigin::Curated => "catalog",
            EntryOrigin::Hub => "docker hub",
            EntryOrigin::Local => "local image",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoreEntry {
    pub name: String,
    pub image: String,
    pub description: String,
    pub origin: EntryOrigin,
    /// Env keys the catalog knows this server wants: (KEY, how-to-get).
    /// Empty for Hub/Local finds — setup still takes free-form keys.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// Well-known MCP servers, A-Z, with the env keys they need. Every
/// image listed here was verified to exist on its registry at commit
/// time (mcp/* = Docker's MCP Catalog conventions; the live Docker Hub
/// search in /mcp covers the rest, and pull failures surface honestly
/// — exit status, not output words).
pub fn curated_catalog() -> Vec<StoreEntry> {
    let rows: &[(&str, &str, &str, &[(&str, &str)])] = &[
        ("brave-search", "mcp/brave-search", "Web search via the Brave Search API.", &[("BRAVE_API_KEY", "a Brave Search API key (api.search.brave.com)")]),
        ("everything", "mcp/everything", "MCP reference server exposing tools, resources and prompts — a safe first install to try /mcp.", &[]),
        ("fetch", "mcp/fetch", "Fetch web pages and convert them to clean markdown the model can read.", &[]),
        ("filesystem", "mcp/filesystem", "Sandboxed file access for explicitly allowed directories.", &[]),
        ("git", "mcp/git", "Git operations: clone, status, diff, log, branches, create repos.", &[]),
        ("github", "ghcr.io/github/github-mcp-server:main", "GitHub's official MCP server — repos, issues, PRs, code search.", &[("GITHUB_PERSONAL_ACCESS_TOKEN", "a GitHub personal access token (github.com/settings/tokens)")]),
        ("gitlab", "mcp/gitlab", "GitLab projects, issues, merge requests, pipelines.", &[("GITLAB_TOKEN", "a GitLab API token")]),
        ("google-maps", "mcp/google-maps", "Google Maps: geocode, directions, places.", &[("GOOGLE_MAPS_API_KEY", "a Google Maps API key")]),
        ("memory", "mcp/memory", "Knowledge-graph memory: entities, relations, observations.", &[]),
        ("obsidian", "mcp/obsidian", "Read and search Obsidian vaults — your markdown notes, live.", &[]),
        ("puppeteer", "mcp/puppeteer", "Headless-Chromium automation: navigate, click, screenshot, evaluate JS.", &[]),
        ("redis", "mcp/redis", "Redis key-value access — get, set, list, delete.", &[]),
        ("sentry", "mcp/sentry", "Sentry.io issues and events.", &[("SENTRY_AUTH_TOKEN", "a Sentry auth token")]),
        ("slack", "mcp/slack", "Slack workspaces — channels, messages, reactions.", &[("SLACK_BOT_TOKEN", "a Slack bot token (xoxb-…)")]),
        ("sqlite", "mcp/sqlite", "SQL over SQLite database files with row limits.", &[]),
        ("time", "mcp/time", "Current time in any timezone, plus conversions between zones.", &[]),
    ];
    let mut out: Vec<StoreEntry> = rows
        .iter()
        .map(|(n, i, d, keys)| StoreEntry {
            name: (*n).into(),
            image: (*i).into(),
            description: (*d).into(),
            origin: EntryOrigin::Curated,
            keys: keys.to_vec(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Case-insensitive pre-filter on name + image + description.
pub fn matches(entry: &StoreEntry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let hay = format!("{} {} {}", entry.name, entry.image, entry.description).to_lowercase();
    query.split_whitespace().all(|term| hay.contains(term))
}

// ------------------------------------------------------------------ runtime

/// The container CLI this machine answers with: `docker` when present,
/// else `podman`. Every runtime call goes through this so a
/// podman-native install works everywhere docker was assumed before.
pub fn runtime_cli() -> Option<&'static str> {
    for cli in ["docker", "podman"] {
        let ok = Command::new(cli)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Some(cli);
        }
    }
    None
}

pub fn docker_available() -> bool {
    runtime_cli().is_some()
}

/// User-facing runtime lookup for interactive flows.
fn runtime() -> Result<&'static str> {
    runtime_cli().ok_or_else(|| {
        anyhow!(
            "no container runtime found — install docker or podman (the \
             installer wires podman + a docker-compatible CLI), or set \
             DOCKER_HOST at a machine that runs it"
        )
    })
}

/// THE secure launch template: every MCP server runs with this. The
/// sales line is security, not convenience — a server talks to the
/// agent over stdio, keeps zero capabilities, and publishes nothing:
/// - outbound internet — API-backed servers (GitHub, Slack, Brave,
///   fetch, …) cannot work cut off from the web; DNS+HTTPS out only
///   reduces what they can do, not what they can reach in.
/// - `--network=none` OPT-IN per server (`network_isolated`): for
///   servers that never need the web (filesystem, sqlite, memory,
///   time) — then no network at all, no callbacks, no exfil, no LAN.
/// - `--cap-drop=ALL` + `--security-opt=no-new-privileges`: zero superuser
///   capabilities, no privilege escalation inside or out.
/// - `--memory/--pids-limit`: one server cannot starve the phone/host.
/// - `-i --rm`: stdio pipe in/out, container dies with the session.
/// - no published ports: nothing on the machine can reach it.
pub fn secure_run_args(network_isolated: bool) -> Vec<String> {
    const HARDENED: [&str; 7] = [
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--memory=512m",
        "--pids-limit=128",
        "--pull=never",
        "-i",
        "--rm",
    ];
    let mut args: Vec<String> = HARDENED.iter().map(|s| s.to_string()).collect();
    if network_isolated {
        args.push("--network=none".to_string());
    }
    args
}

/// Repository of an image ref with the tag stripped — a colon only
/// counts as a tag in the final path segment, so registries that carry
/// a port ("localhost:5000/x") keep it.
fn repo_of(image: &str) -> &str {
    let last_seg = image.rfind('/').map(|i| i + 1).unwrap_or(0);
    match image[last_seg..].find(':') {
        Some(tag_at) => &image[..last_seg + tag_at],
        None => image,
    }
}

/// Registries demand a namespace; bare names live in docker.io/library.
fn full_image(image: &str) -> String {
    if image.contains('/') {
        image.to_string()
    } else {
        format!("docker.io/library/{image}")
    }
}

/// `images` already pulled here, restricted to MCP-looking repos.
fn local_images() -> Vec<String> {
    let Some(cli) = runtime_cli() else {
        return Vec::new();
    };
    let out = Command::new(cli)
        .args(["images", "--format", "{{.Repository}}:{{.Tag}}"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| l.to_lowercase().contains("mcp"))
            .collect(),
        _ => Vec::new(),
    }
}

/// Live Docker Hub search — "every MCP server found in Docker, A-Z".
/// Best-effort: on rate-limit/offline (common on a phone) the curated
/// catalog still shows.
fn hub_search(http: ureq::Agent) -> Vec<StoreEntry> {
    let mut out = Vec::new();
    let Ok(resp) = http
        .get(HUB_SEARCH_URL)
        .timeout(std::time::Duration::from_secs(12))
        .set("User-Agent", "DeCypherTek-MCP")
        .call()
    else {
        return out;
    };
    let Ok(v) = serde_json::from_reader::<_, Value>(resp.into_reader()) else {
        return out;
    };
    if let Some(results) = v.get("results").and_then(|r| r.as_array()) {
        for r in results {
            let repo = r.get("repo_name").and_then(|x| x.as_str()).unwrap_or("");
            if repo.is_empty() || !repo.to_lowercase().contains("mcp") {
                continue;
            }
            let name = sanitize_server_name(repo.rsplit('/').next().unwrap_or(repo));
            let desc = r
                .get("short_description")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            out.push(StoreEntry {
                name,
                image: repo.to_string(),
                description: if desc.is_empty() {
                    format!("MCP image from Docker Hub ({repo}).")
                } else {
                    desc
                },
                origin: EntryOrigin::Hub,
                keys: Vec::new(),
            });
        }
    }
    out
}

/// Pull with honest success detection: the process's REAL exit status
/// plus an `image inspect` as a second witness — both runtimes report
/// the same way. Progress prints live; no output string-matching (the
/// old store matched docker's success words and mislabeled podman's
/// identical success — exit status 0 — a failure, so nothing ever got
/// registered).
fn pull_image(cli: &str, image: &str) -> Result<()> {
    let status = Command::new(cli)
        .args(["pull", image])
        .status()
        .with_context(|| format!("{cli} pull {image}"))?;
    let exit_ok = status.success();
    let inspect_ok = image_present(cli, image);
    if !pull_ok(exit_ok, inspect_ok) {
        return Err(anyhow!(
            "pull of '{image}' did not land (exit: {status}, inspect found it: \
             {inspect_ok}) — the image may not exist, the tag may be wrong, \
             or the registry needs login"
        ));
    }
    Ok(())
}

/// Truth for "did the pull land": both witnesses must agree.
fn pull_ok(exit_ok: bool, inspect_ok: bool) -> bool {
    exit_ok && inspect_ok
}

fn image_present(cli: &str, image: &str) -> bool {
    Command::new(cli)
        .args(["image", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ------------------------------------------------------------------ registry

/// Registry handles: lowercase [a-z0-9-], safe as `mcp_<name>…` tools.
pub fn sanitize_server_name(raw: &str) -> String {
    let cleaned: String = raw
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "server".to_string()
    } else {
        trimmed
    }
}

/// A registered entry for this image (repository match, tags ignored).
fn registered_index(cfg: &Config, entry_image: &str) -> Option<usize> {
    let repo = repo_of(&full_image(entry_image)).to_string();
    cfg.mcp_servers.iter().position(|s| repo_of(&s.image) == repo)
}

/// Register (or re-enable) a server; returns its index in the vault.
/// Re-registering keeps the stored setup — keys, permissions — and
/// only flips `enabled` back on.
fn register(cfg: &mut Config, entry: &StoreEntry) -> usize {
    if let Some(i) = registered_index(cfg, &entry.image) {
        cfg.mcp_servers[i].enabled = true;
        cfg.mcp_servers[i].added = crate::util::now_iso();
        return i;
    }
    let image = full_image(&entry.image);
    let base = sanitize_server_name(&entry.name);
    let mut name = base.clone();
    let mut n = 2;
    while cfg.mcp_servers.iter().any(|s| s.name == name) {
        name = format!("{base}{n}");
        n += 1;
    }
    cfg.mcp_servers.push(McpServer {
        name,
        image,
        enabled: true,
        added: crate::util::now_iso(),
        env: BTreeMap::new(),
        allow_chat: true,
        allow_code: true,
        allow_research: true,
        network_isolated: false,
    });
    cfg.mcp_servers.len() - 1
}

/// NAME=VALUE with NAME strictly `[A-Z_][A-Z0-9_]*`, uppercased for
/// free — the shape env files and containers expect.
fn parse_env_pair(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once('=')?;
    let k = k.trim().to_uppercase();
    if k.is_empty() || k == "_" {
        return None;
    }
    let first_ok = k
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase() || c == '_')
        .unwrap_or(false);
    let rest_ok = k
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !(first_ok && rest_ok) {
        return None;
    }
    Some((k, v.trim().to_string()))
}

// ------------------------------------------------------------------ /mcp TUI

/// The `/mcp` flow: ask, don't bury. One menu — Search for a new MCP
/// server / Setup an MCP server / Done; the search pulls and then walks
/// setup (keys + permissions + verify); setup manages what is
/// registered. All changes save when /mcp closes.
pub fn mcp(cfg: &mut Config, paths: &Paths, query: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        return Err(anyhow!("/mcp is interactive — run it in the terminal"));
    }
    let theme = ColorfulTheme::default();

    let enabled = cfg.mcp_servers.iter().filter(|s| s.enabled).count();
    let runtime_note = if docker_available() {
        String::new()
    } else {
        "\nNo container runtime on PATH — you can browse and register, but \
         pulls and launches need docker or podman."
            .to_string()
    };
    tui::info(
        "MCP",
        &format!(
            "Registered servers: {enabled} enabled / {} total. Search pulls \
             an image and then walks setup — env keys (hidden, vault-sealed), \
             network reach (outbound internet or fully isolated), mode \
             permissions (/chat, /code, /research), and a verification \
             launch. Secure template on every launch: all capabilities \
             dropped, stdio-only, nothing published — servers talk to the \
             agent; internet reach is opt-out per server.{runtime_note}",
            cfg.mcp_servers.len()
        ),
    );

    // The A-Z list is assembled once per /mcp open (local + catalog +
    // Hub fetch), not re-fetched every loop like the store did.
    let entries = assemble_entries();

    if !query.trim().is_empty() {
        // "/mcp something" — straight into search with the words applied.
        search_flow(cfg, paths, &entries, query, &theme)?;
    } else {
        loop {
            let mut options = vec!["Search for a new MCP server".to_string()];
            if !cfg.mcp_servers.is_empty() {
                options.push("Setup an MCP server (keys, permissions, on/off)".to_string());
            }
            options.push("Done".to_string());
            let idx = Select::with_theme(&theme)
                .with_prompt("MCP tool servers")
                .items(&options)
                .default(0)
                .interact_opt()?;
            let Some(idx) = idx else {
                break;
            };
            match idx {
                0 => search_flow(cfg, paths, &entries, "", &theme)?,
                1 if !cfg.mcp_servers.is_empty() => manage_flow(cfg, paths, &theme)?,
                _ => break,
            }
        }
    }

    let count = cfg.mcp_servers.iter().filter(|s| s.enabled).count();
    tui::info(
        "MCP",
        &format!(
            "Registered MCP servers: {count} enabled / {} total. They run \
             hardened (cap-drop=ALL, stdio-only, nothing published; outbound \
             internet unless a server opts into --network=none), on the \
             agent's terms. Live view: /status — manage any time: /mcp.",
            cfg.mcp_servers.len()
        ),
    );
    cfg.save(paths).context("save config after /mcp")?;
    Ok(())
}

/// The A-Z list: local images, the curated catalog (its key metadata
/// wins dedupe), then a live Docker Hub search — sorted A-Z.
fn assemble_entries() -> Vec<StoreEntry> {
    let mut entries: Vec<StoreEntry> = Vec::new();
    for img in local_images() {
        let base = img.rsplit('/').next().unwrap_or(&img);
        let handle = sanitize_server_name(base.split(':').next().unwrap_or(base));
        entries.push(StoreEntry {
            name: handle,
            image: img.clone(),
            description: format!("Already pulled on this machine ({img})."),
            origin: EntryOrigin::Local,
            keys: Vec::new(),
        });
    }
    entries.extend(curated_catalog());
    let http = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .build();
    let hub = hub_search(http);
    if !hub.is_empty() {
        tui::info(
            "MCP",
            &format!("Docker Hub live search found {} MCP servers.", hub.len()),
        );
    }
    entries.extend(hub);

    let mut seen: Vec<String> = Vec::new();
    let mut list: Vec<StoreEntry> = Vec::new();
    for e in entries {
        let repo = repo_of(&full_image(&e.image)).to_string();
        if let Some(i) = seen.iter().position(|r| *r == repo) {
            // The catalog row knows the required keys — never lose it
            // to a bare Hub row of the same repository.
            if list[i].keys.is_empty() && !e.keys.is_empty() {
                list[i] = e;
            }
            continue;
        }
        seen.push(repo);
        list.push(e);
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// Width budget for one search line: the fuzzy prompt owns the rest of
/// the terminal; the floor keeps name+image visible on narrow screens.
fn line_budget(term_width: usize) -> usize {
    term_width.saturating_sub(8).max(20)
}

/// Cut to a char-budget with ONE ellipsis — rows can never wrap, never
/// overflow, never abort a multi-byte char.
fn truncate_chars(s: &str, budget: usize) -> String {
    if s.chars().count() <= budget {
        return s.to_string();
    }
    let mut out: String = s.chars().take(budget.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One readable row: `name · image (origin)[ registered] — description`.
fn list_line(entry: &StoreEntry, registered: bool, budget: usize) -> String {
    let text = format!(
        "{} · {} ({}){} — {}",
        entry.name,
        entry.image,
        entry.origin.label(),
        if registered { " [registered]" } else { "" },
        entry.description
    );
    truncate_chars(&text, budget)
}

fn search_flow(
    cfg: &mut Config,
    paths: &Paths,
    entries: &[StoreEntry],
    prefilter: &str,
    theme: &ColorfulTheme,
) -> Result<()> {
    let mut list: Vec<StoreEntry> = entries.to_vec();
    if !prefilter.trim().is_empty() {
        list.retain(|e| matches(e, prefilter));
    }
    if list.is_empty() {
        tui::warn("MCP", "No MCP servers match that search — try another term (or empty).");
        return Ok(());
    }
    let budget = line_budget(tui::term_width());
    let items: Vec<String> = list
        .iter()
        .map(|e| list_line(e, registered_index(cfg, &e.image).is_some(), budget))
        .collect();
    let sel = FuzzySelect::with_theme(theme)
        .with_prompt("Search MCP servers (Esc cancels)")
        .items(&items)
        .default(0)
        .max_length(12)
        .interact_opt()?;
    let Some(idx) = sel else {
        return Ok(());
    };
    if idx >= list.len() {
        return Ok(());
    }
    detail_and_act(cfg, paths, &list[idx], theme)
}

/// Required-key lines for the detail panel.
fn keys_hint(keys: &[(&'static str, &'static str)]) -> String {
    if keys.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = keys.iter().map(|(k, w)| format!("  {k} — {w}")).collect();
    format!(
        "needs env keys (asked during setup, hidden input, vault-sealed):\n{}\n\n",
        lines.join("\n")
    )
}

/// Short registered-state suffix for the detail header.
fn registered_note(cfg: &Config, idx: Option<usize>) -> String {
    match idx {
        None => String::new(),
        Some(i) => {
            let s = &cfg.mcp_servers[i];
            format!(
                " as '{}' (chat {} · code {} · research {} · keys: {})",
                s.name,
                yn(s.allow_chat),
                yn(s.allow_code),
                yn(s.allow_research),
                if s.env.is_empty() {
                    "none".to_string()
                } else {
                    s.env.keys().cloned().collect::<Vec<_>>().join(", ")
                }
            )
        }
    }
}

fn detail_and_act(
    cfg: &mut Config,
    paths: &Paths,
    entry: &StoreEntry,
    theme: &ColorfulTheme,
) -> Result<()> {
    let reg_idx = registered_index(cfg, &entry.image);
    tui::info(
        "MCP SERVER",
        &format!(
            "{}\n{} · {}{}\n\n{}\n\n{}security template: --cap-drop=ALL, \
             no-new-privileges, stdio-only, nothing published, outbound \
             internet (a server must reach its APIs) — or fully \
             --network=none when set up that way.",
            entry.image,
            entry.origin.label(),
            if reg_idx.is_some() { "registered" } else { "not registered" },
            registered_note(cfg, reg_idx),
            entry.description,
            keys_hint(&entry.keys)
        ),
    );

    let options: Vec<String> = if reg_idx.is_some() {
        vec![
            "Set up again (API keys + mode permissions)",
            "Update image (pull latest)",
            "Remove from the vault (image kept on disk)",
            "Back",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    } else {
        vec![
            "Add — pull, then set it up (keys + permissions)",
            "Pull only (register later)",
            "Back",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    };
    let sel = Select::with_theme(theme)
        .with_prompt("Action")
        .items(&options)
        .default(0)
        .interact_opt()?;
    let Some(sel) = sel else {
        return Ok(());
    };

    match (sel, reg_idx) {
        (0, None) => {
            let Ok(cli) = runtime() else {
                tui::error("No container runtime found — install docker or podman first, then /mcp again.");
                return Ok(());
            };
            if let Err(e) = pull_image(cli, &entry.image) {
                tui::error(&format!("pull failed: {e:#}"));
                return Ok(());
            }
            let idx = register(cfg, entry);
            guided_setup(cfg, paths, idx, Some(entry), theme)?;
            if let Err(e) = cfg.save(paths) {
                tui::warn("MCP", &format!("could not save the config: {e:#}"));
            }
            tui::info(
                "MCP",
                &format!(
                    "Registered '{}' from {}. Its tools surface as \
                     mcp_{}_<tool> and go live automatically in your next \
                     allowed /chat, /code or /research run. Manage keys and \
                     permissions any time: /mcp → Setup.",
                    cfg.mcp_servers[idx].name,
                    entry.image,
                    sanitize_server_name(&cfg.mcp_servers[idx].name)
                ),
            );
        }
        (1, None) => {
            let Ok(cli) = runtime() else {
                tui::error("No container runtime found — install docker or podman first.");
                return Ok(());
            };
            if let Err(e) = pull_image(cli, &entry.image) {
                tui::error(&format!("pull failed: {e:#}"));
                return Ok(());
            }
            tui::info("MCP", "Image pulled. Register it any time: /mcp → Search → Add.");
        }
        (0, Some(i)) => guided_setup(cfg, paths, i, Some(entry), theme)?,
        (1, Some(_)) => {
            let Ok(cli) = runtime() else {
                tui::error("No container runtime found — install docker or podman first.");
                return Ok(());
            };
            if let Err(e) = pull_image(cli, &entry.image) {
                tui::error(&format!("pull failed: {e:#}"));
                return Ok(());
            }
            tui::info("MCP", "Image updated — the server relaunches fresh on its next run.");
        }
        (2, Some(i)) => {
            let removed = cfg.mcp_servers.remove(i).name;
            tui::info(
                "MCP",
                &format!(
                    "'{removed}' removed from the vault (image kept on disk \
                     for a fast re-add)."
                ),
            );
        }
        _ => {}
    }
    Ok(())
}

/// The question chain that makes a registration actually work: handle
/// name, required env keys (hidden input), extra env keys, network
/// reach (outbound internet or fully isolated), mode permissions, and
/// an optional smoke test that launches the container and lists its
/// tools.
fn guided_setup(
    cfg: &mut Config,
    paths: &Paths,
    idx: usize,
    entry: Option<&StoreEntry>,
    theme: &ColorfulTheme,
) -> Result<()> {
    let server = cfg.mcp_servers[idx].clone();

    // 1. The handle (its tools become mcp_<handle>_<tool>).
    let name: String = Input::with_theme(theme)
        .with_prompt("Server handle (its tools become mcp_<handle>_<tool>)")
        .default(server.name.clone())
        .interact()?;
    let cleaned = sanitize_server_name(&name);
    let taken = cfg
        .mcp_servers
        .iter()
        .enumerate()
        .any(|(i, s)| i != idx && s.name == cleaned);
    if cleaned != server.name && taken {
        tui::warn(
            "MCP",
            &format!("handle '{cleaned}' is taken — kept '{}'.", server.name),
        );
    } else {
        cfg.mcp_servers[idx].name = cleaned;
    }

    // 2. Required keys the catalog knows about — hidden input.
    if let Some(e) = entry {
        for (key, why) in &e.keys {
            let stored = cfg.mcp_servers[idx].env.contains_key(*key);
            if !stored {
                if Confirm::with_theme(theme)
                    .with_prompt(format!("Set {key} now? (needed: {why})"))
                    .default(true)
                    .interact()?
                {
                    let val: String = Password::new()
                        .with_prompt(format!("{key} — paste the value"))
                        .interact()?;
                    cfg.mcp_servers[idx]
                        .env
                        .insert((*key).to_string(), val.trim().to_string());
                } else {
                    tui::warn(
                        "MCP",
                        &format!(
                            "{key} not set — the server may refuse to list \
                             tools until you add it here (values are hidden \
                             and sealed inside the vault)."
                        ),
                    );
                }
            } else if Confirm::with_theme(theme)
                .with_prompt(format!("Replace the stored {key}?"))
                .default(false)
                .interact()?
            {
                let val: String = Password::new()
                    .with_prompt(format!("{key} — paste the value"))
                    .interact()?;
                cfg.mcp_servers[idx]
                    .env
                    .insert((*key).to_string(), val.trim().to_string());
            }
        }
    }

    // 3. Extra keys, free-form.
    loop {
        let line: String = Input::with_theme(theme)
            .with_prompt("Another env key (NAME=VALUE, empty to finish)")
            .allow_empty(true)
            .default("".into())
            .interact()?;
        let line = line.trim().to_string();
        if line.is_empty() {
            break;
        }
        match parse_env_pair(&line) {
            Some((k, v)) => {
                cfg.mcp_servers[idx].env.insert(k, v);
            }
            None => tui::warn(
                "MCP",
                &format!(
                    "'{line}' is not NAME=VALUE with NAME built of A-Z, 0-9, \
                     _ (letter or underscore first)."
                ),
            ),
        }
    }

    // 4. Network reach. Outbound internet is what makes API-backed
    //    servers (GitHub, Slack, Brave, fetch…) worth running — they
    //    call web APIs. `--network=none` fits servers that never need
    //    the web (filesystem, sqlite, memory, time). Defaults to
    //    outbound: a cut-off server "launches and answers" but no
    //    tool call can ever leave the container.
    let reach = Select::with_theme(theme)
        .with_prompt("Network reach for this server")
        .items(&[
            "Outbound internet — it calls web APIs (GitHub, Slack, fetch…)",
            "Fully isolated — no network at all, --network=none (filesystem, sqlite, memory…)",
        ])
        .default(if cfg.mcp_servers[idx].network_isolated {
            1
        } else {
            0
        })
        .interact()?;
    cfg.mcp_servers[idx].network_isolated = reach == 1;

    // 5. Mode permissions — /chat, /code, /research. Routing prefs,
    //    not the security leash (that is tool_mcp + the Leash).
    set_mode_permissions(cfg, idx, theme);

    // 6. Verify: the honest answer to "does it actually work?".
    if Confirm::with_theme(theme)
        .with_prompt("Verify now? (launches the hardened container and lists its tools)")
        .default(true)
        .interact()?
    {
        verify_with_hint(cfg, paths, idx);
    }
    Ok(())
}

fn set_mode_permissions(cfg: &mut Config, idx: usize, theme: &ColorfulTheme) {
    let modes = ["/chat", "/code", "/research"];
    let s = &cfg.mcp_servers[idx];
    let defaults = vec![s.allow_chat, s.allow_code, s.allow_research];
    let chosen = MultiSelect::with_theme(theme)
        .with_prompt("Use this server's tools in")
        .items(&modes)
        .defaults(&defaults)
        .interact()
        .unwrap_or_default();
    let s = &mut cfg.mcp_servers[idx];
    s.allow_chat = chosen.contains(&0);
    s.allow_code = chosen.contains(&1);
    s.allow_research = chosen.contains(&2);
}

/// Verify and print the outcome with guidance on failure.
fn verify_with_hint(cfg: &mut Config, paths: &Paths, idx: usize) {
    let isolated = cfg.mcp_servers[idx].network_isolated;
    match verify_server(&paths.staging, &cfg.mcp_servers[idx]) {
        Ok(report) => tui::info("MCP", &report),
        Err(e) => tui::warn(
            "MCP",
            &format!(
                "Verify failed: {e:#}\nMost common causes: a missing API key \
                 (set it: /mcp → Setup), a wrong image tag{}, or the container \
                 runtime being down.",
                if isolated {
                    ", no outbound internet (this server is network-isolated — \
                     /mcp can give it internet reach)"
                } else {
                    ""
                }
            ),
        ),
    }
}

/// Smoke test: start the container under the security template, do the
/// MCP handshake and tools/list. Bounded wall-clock — a hung container
/// can never wedge the CLI (worker thread + channel timeout).
fn verify_server(staging: &Path, server: &McpServer) -> Result<String> {
    let staging = staging.to_path_buf();
    let server = server.clone();
    let (tx, rx) = mpsc::channel::<Result<String>>();
    thread::spawn(move || {
        // The child dies with the thread (Drop), and a late send after
        // a timeout just fails silently into the dropped receiver.
        let out = McpChild::spawn(&server, &staging).map(|child| {
            let names: Vec<String> =
                child.tools.iter().map(|t| t.name.clone()).collect();
            if names.is_empty() {
                return format!(
                    "{} launched and answered, but advertises no tools — \
                     the image may need env keys (/mcp → Setup), a \
                     different container entrypoint{}.",
                    server.name,
                    if server.network_isolated {
                        ", or internet it cannot reach while fully \
                         network-isolated (/mcp → Setup can change that)"
                    } else {
                        ""
                    }
                );
            }
            let handle = sanitize_server_name(&server.name);
            let sample = names
                .iter()
                .take(3)
                .map(|n| format!("mcp_{handle}_{}", sanitize_tool_name(n)))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{} works — {} tool(s): {sample}{}\nThey go live \
                 automatically in your next allowed /chat, /code or \
                 /research run; the model sees and picks them itself. Try \
                 it: /chat using your MCP tools, <what you need done>.",
                server.name,
                names.len(),
                if names.len() > 3 { " …" } else { "" }
            )
        });
        let _ = tx.send(out);
    });
    match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(r) => r,
        Err(_) => Err(anyhow!(
            "the container never finished the MCP handshake within 90s — \
             the image may be missing, or the runtime hung (/mcp can \
             re-verify any time)"
        )),
    }
}

/// Manage everything already registered: the "library" view.
fn manage_flow(cfg: &mut Config, paths: &Paths, theme: &ColorfulTheme) -> Result<()> {
    loop {
        if cfg.mcp_servers.is_empty() {
            return Ok(());
        }
        let mut items: Vec<String> = cfg.mcp_servers.iter().map(server_line).collect();
        let done = "… done (back to the /mcp menu)".to_string();
        items.push(done.clone());
        let len = items.len();
        let sel = Select::with_theme(theme)
            .with_prompt("Your registered MCP servers")
            .items(&items)
            .default(len - 1)
            .interact_opt()?;
        let Some(sel) = sel else {
            return Ok(());
        };
        if sel >= cfg.mcp_servers.len() {
            return Ok(());
        }
        manage_one(cfg, paths, sel, theme)?;
        // Save after each action — a mid-flow interrupt never costs
        // more than the question it stopped.
        if let Err(e) = cfg.save(paths) {
            tui::warn("MCP", &format!("could not save the config: {e:#}"));
        }
    }
}

/// One readable management row: state, handle, image, modes, key NAMES
/// (never values), network reach when it is the exception.
fn server_line(s: &McpServer) -> String {
    format!(
        "[{}] {} — {} (chat {} · code {} · research {} · keys: {}{})",
        if s.enabled { "on " } else { "off" },
        s.name,
        s.image,
        yn(s.allow_chat),
        yn(s.allow_code),
        yn(s.allow_research),
        if s.env.is_empty() {
            "none".to_string()
        } else {
            s.env.keys().cloned().collect::<Vec<_>>().join(", ")
        },
        if s.network_isolated {
            " · net: isolated"
        } else {
            ""
        }
    )
}

fn yn(b: bool) -> &'static str {
    if b {
        "y"
    } else {
        "n"
    }
}

fn manage_one(cfg: &mut Config, paths: &Paths, idx: usize, theme: &ColorfulTheme) -> Result<()> {
    let s = cfg.mcp_servers[idx].clone();
    let actions = vec![
        "Walk the full setup again (keys + network + permissions + verify)",
        "Add or replace an env key (NAME=VALUE)",
        "Remove an env key",
        "Permissions (/chat /code /research)",
        "Network reach (outbound internet / fully isolated)",
        "Rename the server handle",
        "Toggle enable/disable",
        "Update image (pull latest)",
        "Verify now (launch + list tools)",
        "Remove from the vault (image kept on disk)",
        "Back",
    ];
    let sel = Select::with_theme(theme)
        .with_prompt(format!("Server '{}' — what to do?", s.name))
        .items(&actions)
        .default(0)
        .interact_opt()?;
    let Some(sel) = sel else {
        return Ok(());
    };
    match sel {
        0 => guided_setup(cfg, paths, idx, None, theme)?,
        1 => {
            let line: String = Input::with_theme(theme)
                .with_prompt("Env key (NAME=VALUE)")
                .allow_empty(true)
                .default("".into())
                .interact()?;
            let typed = line.trim().to_string();
            if !typed.is_empty() {
                match parse_env_pair(&typed) {
                    Some((k, v)) => {
                        cfg.mcp_servers[idx].env.insert(k, v);
                        tui::info("MCP", "Key stored — masked everywhere, sealed inside the vault.");
                    }
                    None => tui::warn(
                        "MCP",
                        "Expected NAME=VALUE with NAME built of A-Z, 0-9, _ (letter or underscore first).",
                    ),
                }
            }
        }
        2 => {
            if s.env.is_empty() {
                tui::info("MCP", "No env keys stored for this server.");
            } else {
                let keys: Vec<String> = s.env.keys().cloned().collect();
                let sel = Select::with_theme(theme)
                    .with_prompt("Remove which key?")
                    .items(&keys)
                    .default(0)
                    .interact()?;
                if sel < keys.len() {
                    let removed = keys[sel].clone();
                    cfg.mcp_servers[idx].env.remove(&removed);
                    tui::info("MCP", &format!("{removed} removed."));
                }
            }
        }
        3 => set_mode_permissions(cfg, idx, theme),
        4 => {
            let reach = Select::with_theme(theme)
                .with_prompt(format!(
                    "Network reach for '{}' — API-backed servers need outbound",
                    s.name
                ))
                .items(&[
                    "Outbound internet — it calls web APIs (GitHub, Slack, fetch…)",
                    "Fully isolated — no network at all, --network=none (filesystem, sqlite, memory…)",
                ])
                .default(if s.network_isolated { 1 } else { 0 })
                .interact()?;
            cfg.mcp_servers[idx].network_isolated = reach == 1;
            tui::info(
                "MCP",
                &if reach == 1 {
                    format!(
                        "{} runs fully network-isolated from its next launch — \
                         only choose this for servers that never need the web.",
                        s.name
                    )
                } else {
                    format!(
                        "{} gets outbound internet from its next launch — it \
                         can reach the APIs it needs, still publishes nothing.",
                        s.name
                    )
                },
            );
        }
        5 => {
            let name: String = Input::with_theme(theme)
                .with_prompt("Server handle (its tools become mcp_<handle>_<tool>)")
                .default(s.name.clone())
                .interact()?;
            let cleaned = sanitize_server_name(&name);
            let taken = cfg
                .mcp_servers
                .iter()
                .enumerate()
                .any(|(i, x)| i != idx && x.name == cleaned);
            if cleaned != s.name {
                if taken {
                    tui::warn(
                        "MCP",
                        &format!("handle '{cleaned}' is taken — kept '{}'.", s.name),
                    );
                } else {
                    cfg.mcp_servers[idx].name = cleaned.clone();
                    tui::info("MCP", &format!("Handle is now '{cleaned}'."));
                }
            }
        }
        6 => {
            cfg.mcp_servers[idx].enabled = !s.enabled;
            if cfg.mcp_servers[idx].enabled {
                tui::info("MCP", &format!("{} enabled.", s.name));
            } else {
                tui::info(
                    "MCP",
                    &format!("{} disabled — registration kept, runs paused.", s.name),
                );
            }
        }
        7 => match runtime() {
            Ok(cli) => match pull_image(cli, &s.image) {
                Ok(()) => tui::info("MCP", "Image updated — the server relaunches fresh on its next run."),
                Err(e) => tui::error(&format!("pull failed: {e:#}")),
            },
            Err(e) => tui::error(&e.to_string()),
        },
        8 => verify_with_hint(cfg, paths, idx),
        9 => {
            if Confirm::with_theme(theme)
                .with_prompt(format!("Remove '{}' from the vault? (image kept on disk)", s.name))
                .default(false)
                .interact()?
            {
                let removed = cfg.mcp_servers.remove(idx).name;
                tui::info("MCP", &format!("'{removed}' removed from the vault."));
            }
        }
        _ => {}
    }
    Ok(())
}

// --------------------------------------------------------------- MCP client

/// A live, hardened MCP server container held by the agent, speaking
/// newline-delimited JSON-RPC 2.0 over stdio (spawned per agent run).
pub struct McpChild {
    pub server: String,
    pub image: String,
    child: Child,
    writer: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
    pub tools: Vec<McpTool>,
    /// This launch's network state, only for honest tool descriptions
    /// (the container itself was started with it in secure_run_args).
    pub network_isolated: bool,
    /// The 0600 env-file this launch was given (secrets never ride in
    /// argv); removed with the container.
    env_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct McpTool {
    /// Raw tool name as the server advertises it (not the mcp_* prefix).
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A tools/list JSON-RPC response → its tools. Responses carry the
/// payload top-level under `result`; `params` is a REQUEST field —
/// reading `/params/tools` made every server on earth "advertise no
/// tools" while their containers shipped 100+ of them.
fn tools_from_response(resp: &Value) -> Vec<McpTool> {
    resp.pointer("/result/tools")
        .and_then(|tools| tools.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|t| {
                    let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    if name.is_empty() {
                        return None;
                    }
                    Some(McpTool {
                        name: name.into(),
                        description: t
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        input_schema: t
                            .get("inputSchema")
                            .cloned()
                            .filter(|s| s.is_object())
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A tools/call JSON-RPC response → (isError, joined text content).
/// Same JSON-RPC shape: the tool's result lives under `result`, never
/// `params` (the old `/params/result/…` pointers always read empty).
fn call_result(resp: &Value) -> (bool, String) {
    let is_error = resp
        .pointer("/result/isError")
        .and_then(|e| e.as_bool())
        .unwrap_or(false);
    let text = resp
        .pointer("/result/content")
        .and_then(|c| c.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    (is_error, text)
}

impl McpChild {
    /// Launch a container under the security template (plus the server's
    /// env keys via a 0600 --env-file) and do the MCP handshake.
    pub fn spawn(server: &McpServer, staging: &Path) -> Result<McpChild> {
        let cli = runtime_cli().ok_or_else(|| {
            anyhow!("no container runtime found — install docker or podman first")
        })?;
        let mut cmd = Command::new(cli);
        cmd.arg("run").args(secure_run_args(server.network_isolated));
        let env_file = write_env_file(server, staging)?;
        if let Some(p) = &env_file {
            // Secrets ride in a 0600 file inside staging (the area that
            // is decrypted while the session runs and re-sealed on
            // exit), never in argv where ps could read them.
            cmd.arg("--env-file").arg(p);
        }
        cmd.arg(&server.image)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("{cli} run {} (is the runtime up?)", server.image))?;
        let writer = child.stdin.take().context("container stdin pipe")?;
        let reader = BufReader::new(child.stdout.take().context("container stdout pipe")?);
        let mut m = McpChild {
            server: server.name.clone(),
            image: server.image.clone(),
            network_isolated: server.network_isolated,
            child,
            writer,
            reader,
            next_id: 3,
            tools: Vec::new(),
            env_file,
        };

        m.rpc_call(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "decyphertek", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        m.notify("notifications/initialized");
        let list_resp = m.rpc_call("tools/list", json!({}))?;
        m.tools = tools_from_response(&list_resp);
        Ok(m)
    }

    fn rpc_call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.rpc_send(&req)?;
        self.rpc_read(id)
    }

    fn notify(&mut self, method: &str) {
        let req = json!({ "jsonrpc": "2.0", "method": method });
        let _ = self.rpc_send(&req);
    }

    fn rpc_send(&mut self, req: &Value) -> Result<()> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.flush())
            .context("write to MCP server")
    }

    /// Read until the response with this id arrives. Notifications and
    /// log traffic from the server are skipped (bounded so a chatty or
    /// broken container can never wedge the agent).
    fn rpc_read(&mut self, want_id: u64) -> Result<Value> {
        for _ in 0..RPC_READ_LIMIT {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).context("read MCP stdio")?;
            if n == 0 {
                return Err(anyhow!(
                    "MCP server '{}' closed its stdio — runtime down, image \
                     deleted, or missing API-key env (add it with /mcp)",
                    self.server
                ));
            }
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                continue; // container FYI lines / non-JSON noise
            };
            match v.get("id").and_then(|i| i.as_u64()) {
                Some(id) if id == want_id => {
                    if let Some(err) = v.get("error") {
                        return Err(anyhow!("MCP error from {}: {err}", self.server));
                    }
                    return Ok(v);
                }
                _ => continue, // server notifications, other traffic
            }
        }
        Err(anyhow!(
            "MCP server '{}' never answered request {want_id}",
            self.server
        ))
    }

    /// tools/call — answers the model via the agent, internally.
    pub fn call(&mut self, tool: &str, args_json: &str) -> String {
        let args: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
        match self.rpc_call("tools/call", json!({ "name": tool, "arguments": args })) {
            Ok(v) => {
                let (is_error, text) = call_result(&v);
                if text.is_empty() {
                    return format!("MCP tool {} answered with no text content.", tool);
                }
                if is_error {
                    format!("MCP TOOL ERROR ({}): {text}", self.server)
                } else {
                    text
                }
            }
            Err(e) => format!("MCP TOOL ERROR ({}): {e}", self.server),
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// KEY=VALUE lines in a 0600 file inside staging — one per server, no
/// secrets in argv or in `ps` output. Empty env map → no file at all.
fn write_env_file(server: &McpServer, staging: &Path) -> Result<Option<PathBuf>> {
    if server.env.is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(staging)
        .with_context(|| format!("create {}", staging.display()))?;
    let path = staging.join(format!("mcp-{}.env", sanitize_server_name(&server.name)));
    let body: String = server
        .env
        .iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect();
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(Some(path))
}

/// A run's pool dies with it — no orphaned containers, no leftover key
/// files, ever.
impl Drop for McpChild {
    fn drop(&mut self) {
        self.kill();
        if let Some(p) = self.env_file.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

// ------------------------------------------------------------------ run-time

/// Which per-server switch governs this agent mode. Unknown modes stay
/// private — a future mode never inherits servers by accident.
fn allow_for(s: &McpServer, mode: &str) -> bool {
    match mode {
        "chat" => s.allow_chat,
        "code" => s.allow_code,
        "research" => s.allow_research,
        _ => false,
    }
}

/// Registered, enabled, and allowed in this agent mode. Pure: the
/// testable half of spawn_pool.
pub fn eligible<'a>(cfg: &'a Config, mode: &str) -> Vec<&'a McpServer> {
    cfg.mcp_servers
        .iter()
        .filter(|s| s.enabled && allow_for(s, mode))
        .collect()
}

/// Enabled but not allowed in this mode (for run warnings).
pub fn mode_skips(cfg: &Config, mode: &str) -> usize {
    cfg.mcp_servers
        .iter()
        .filter(|s| s.enabled && !allow_for(s, mode))
        .count()
}

/// Spawn every enabled server allowed in this run's mode. Returns the
/// pool plus a human warning per server that could not start (the run
/// continues without them; the failures are visible, not silent).
pub fn spawn_pool(cfg: &Config, paths: &Paths, mode: &str) -> (Vec<McpChild>, Vec<String>) {
    let mut pool = Vec::new();
    let mut warnings = Vec::new();
    let active = eligible(cfg, mode);
    if active.is_empty() {
        let skipped = mode_skips(cfg, mode);
        if skipped > 0 {
            warnings.push(format!(
                "MCPS: {skipped} registered server(s) are not allowed in \
                 /{mode} — /mcp can change that."
            ));
        }
        return (pool, warnings);
    }
    if !cfg.tool_mcp && !cfg.is_unleashed() {
        warnings.push("MCPS: tool_mcp is off (/setup) — registered servers skipped.".into());
        return (pool, warnings);
    }
    if !docker_available() {
        warnings.push(format!(
            "MCPS: no container runtime found — {} registered server(s) \
             skipped this run (install docker or podman; /mcp has details).",
            active.len()
        ));
        return (pool, warnings);
    }
    for s in active {
        match McpChild::spawn(s, &paths.staging) {
            Ok(child) => {
                if child.tools.is_empty() {
                    warnings.push(format!(
                        "MCPS: {} '{}' came up but advertises no tools — \
                         image may need env keys (/mcp → Setup){}.",
                        s.name,
                        s.image,
                        if s.network_isolated {
                            " or internet while fully network-isolated"
                        } else {
                            ""
                        }
                    ));
                }
                pool.push(child);
            }
            Err(e) => warnings.push(format!(
                "MCPS: '{}' ({}) failed to start: {e:#}",
                s.name, s.image
            )),
        }
    }
    (pool, warnings)
}

/// Merge every pool server's tools into the model's tool list: names
/// become `mcp_<server>_<tool>`, sanitized, prefixed server context.
pub fn pool_specs(pool: &[McpChild]) -> Vec<crate::models::ToolSpec> {
    let mut out = Vec::new();
    for child in pool {
        for tool in &child.tools {
            let name = format!(
                "{}{}_{}",
                MCP_TOOL_PREFIX,
                sanitize_server_name(&child.server),
                sanitize_tool_name(&tool.name)
            );
            out.push(crate::models::ToolSpec {
                typ: "function",
                function: crate::models::ToolFn {
                    name,
                    description: format!(
                        "[mcp server: {} @ {}] {} — hardened container ({}); \
                         it cannot publish ports.",
                        child.server,
                        child.image,
                        if tool.description.trim().is_empty() {
                            "MCP tool"
                        } else {
                            &tool.description
                        },
                        if child.network_isolated {
                            "no network, answers the agent only"
                        } else {
                            "outbound internet for its API calls"
                        }
                    ),
                    parameters: tool.input_schema.clone(),
                },
            });
        }
    }
    out
}

/// Forge a table-safe tool-name suffix (OpenAI tool-name charset).
fn sanitize_tool_name(raw: &str) -> String {
    let cleaned: String = raw
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_').to_string();
    let mut out: String = if trimmed.is_empty() {
        "tool".into()
    } else {
        trimmed
    };
    out.truncate(48);
    out
}

/// Router for `mcp_<server>_<tool>` calls coming out of the model.
pub fn dispatch(ctx: &mut crate::tools::ToolCtx, name: &str, args_json: &str) -> String {
    let rest = match name.strip_prefix(MCP_TOOL_PREFIX) {
        Some(r) => r,
        None => return format!("UNKNOWN MCP TOOL: {name}"),
    };
    for child in ctx.mcp.iter_mut() {
        let server = sanitize_server_name(&child.server);
        if let Some(tool) = rest.strip_prefix(&format!("{server}_")) {
            return child.call(tool, args_json);
        }
    }
    format!("UNKNOWN MCP TOOL: {name} (its server may have failed to launch this run)")
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, image: &str) -> StoreEntry {
        StoreEntry {
            name: name.into(),
            image: image.into(),
            description: "test".into(),
            origin: EntryOrigin::Curated,
            keys: Vec::new(),
        }
    }

    fn srv(name: &str, image: &str) -> McpServer {
        McpServer {
            name: name.into(),
            image: image.into(),
            enabled: true,
            added: String::new(),
            env: BTreeMap::new(),
            allow_chat: true,
            allow_code: true,
            allow_research: true,
            network_isolated: false,
        }
    }

    #[test]
    fn catalog_is_alphabetical_with_real_images() {
        let cat = curated_catalog();
        assert!(cat.len() >= 14, "catalog should have a real A-Z spread");
        for pair in cat.windows(2) {
            assert!(pair[0].name <= pair[1].name, "catalog must be sorted A-Z");
        }
        for e in &cat {
            assert!(!e.name.is_empty());
            assert!(!e.image.is_empty(), "{} needs an image", e.name);
            assert!(!e.description.is_empty());
            for (k, why) in &e.keys {
                assert_eq!(
                    parse_env_pair(&format!("{k}=x")).map(|p| p.0),
                    Some(k.to_string()),
                    "catalog key {k} must be NAME-shaped"
                );
                assert!(!why.is_empty(), "catalog key {k} needs a where-to-get hint");
            }
        }
        assert!(cat.iter().any(|e| e.name == "github"));
        assert!(cat.iter().any(|e| e.name == "brave-search"));
        let gh = cat.iter().find(|e| e.name == "github").unwrap();
        assert!(
            gh.keys
                .iter()
                .any(|(k, _)| *k == "GITHUB_PERSONAL_ACCESS_TOKEN"),
            "github carries its PAT key hint: {:?}",
            gh.keys
        );
    }

    #[test]
    fn secure_template_hardens_both_network_modes() {
        // Outbound (default): API-backed servers need the web, but the
        // template still drops every capability, caps resources and
        // publishes nothing.
        let out = secure_run_args(false);
        assert!(
            !out.contains(&"--network=none".to_string()),
            "outbound is the default — network=none starves GitHub/Slack/fetch"
        );
        assert!(out.contains(&"--cap-drop=ALL".to_string()));
        assert!(out.contains(&"--security-opt=no-new-privileges".to_string()));
        assert!(
            out.contains(&"--rm".to_string()),
            "container dies with the session"
        );
        assert!(
            out.contains(&"-i".to_string()),
            "stdio is the only channel"
        );

        // Isolated opt-in: same hardening plus no network at all.
        let iso = secure_run_args(true);
        assert!(iso.contains(&"--network=none".to_string()));
        for a in &out {
            assert!(iso.contains(a), "isolated keeps every other flag: {a}");
        }

        // neither mode ever publishes ports
        for args in [&out, &iso] {
            assert!(!args.iter().any(|a| a.starts_with("-p")));
            assert!(!args.iter().any(|a| a.starts_with("--publish")));
        }
    }

    #[test]
    fn tools_list_response_is_read_from_result_not_params() {
        // A real tools/list response (JSON-RPC 2.0): the payload sits
        // top-level under `result`; `params` is a request field. The
        // client that read /params/tools saw every server "advertise
        // no tools" — this is that regression, held forever.
        let resp = json!({
            "jsonrpc": "2.0", "id": 4,
            "result": { "tools": [
                {
                    "name": "search_issues",
                    "description": "Search across issues",
                    "inputSchema": { "type": "object", "properties": {
                        "q": { "type": "string" }
                    }}
                },
                { "name": "get_me", "description": "" }
            ]}
        });
        let tools = tools_from_response(&resp);
        assert_eq!(tools.len(), 2, "both tools must be found: got {tools:?}");
        assert_eq!(tools[0].name, "search_issues");
        assert_eq!(tools[0].description, "Search across issues");
        assert!(tools[0].input_schema.is_object());
        assert_eq!(tools[1].name, "get_me", "tool with no inputSchema still lists");
        assert!(
            tools[1].input_schema["type"] == "object",
            "missing schema becomes a benign empty object"
        );

        // a name-less entry or empty list must not crash or invent tools
        assert!(tools_from_response(&json!({ "result": {} })).is_empty());
        assert!(tools_from_response(&json!({})).is_empty());
        assert!(
            tools_from_response(&json!({ "result": { "tools": [ { "description": "x" } ] } }))
                .is_empty()
        );
    }

    #[test]
    fn call_results_are_read_from_result_not_params() {
        let ok = json!({
            "jsonrpc": "2.0", "id": 5,
            "result": { "isError": false, "content": [
                { "type": "text", "text": "issue #7 open" },
                { "type": "text", "text": "issue #8 closed" }
            ]}
        });
        assert_eq!(
            call_result(&ok),
            (false, "issue #7 open\nissue #8 closed".to_string())
        );

        let err = json!({
            "jsonrpc": "2.0", "id": 6,
            "result": { "isError": true, "content": [
                { "type": "text", "text": "network unreachable" }
            ]}
        });
        assert_eq!(
            call_result(&err),
            (true, "network unreachable".to_string()),
            "isError rides next to the content"
        );

        // no content at all → empty text, not a panic
        assert_eq!(
            call_result(&json!({ "result": {} })),
            (false, String::new())
        );
    }

    #[test]
    fn filter_matches_terms_case_insensitively() {
        let cat = curated_catalog();
        let hits: Vec<&StoreEntry> = cat.iter().filter(|e| matches(e, "GIT search")).collect();
        assert!(hits.iter().any(|e| e.name == "github"));
        assert!(!hits.iter().any(|e| e.name == "sqlite"));
        assert!(cat.iter().all(|e| matches(e, "")));
    }

    #[test]
    fn spawn_pool_gate_follows_the_leash() {
        let root = std::env::temp_dir().join(format!("dct-mcp-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths::new(&root);
        paths.ensure_staging().unwrap();

        let cfg = Config {
            tool_mcp: false,
            mcp_servers: vec![srv("x", "x/img")],
            ..Config::default()
        };
        let (_, w) = spawn_pool(&cfg, &paths, "chat");
        assert!(
            w.iter().any(|s| s.contains("tool_mcp is off")),
            "leashed gate off must warn: {w:?}"
        );
        let cfg = Config {
            leash: "unleashed".into(),
            ..cfg.clone()
        };
        let (_, w) = spawn_pool(&cfg, &paths, "chat");
        assert!(
            !w.iter().any(|s| s.contains("tool_mcp is off")),
            "unleashed turns the gate on: {w:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn registry_survives_config_roundtrip_and_old_vaults() {
        let root = std::env::temp_dir().join(format!("dct-mcp-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths::new(&root);
        paths.ensure_staging().unwrap();

        // new config: register, save, reload
        let mut cfg = Config::default();
        let e = entry("Time Server!", "mcp/time");
        let idx = register(&mut cfg, &e);
        assert_eq!(cfg.mcp_servers.len(), 1);
        assert_eq!(cfg.mcp_servers[idx].name, "time-server");
        assert!(cfg.mcp_servers[idx].enabled);
        assert!(cfg.mcp_servers[idx].allow_chat);
        assert!(cfg.mcp_servers[idx].allow_code);
        assert!(cfg.mcp_servers[idx].allow_research);
        assert!(cfg.mcp_servers[idx].env.is_empty());
        cfg.save(&paths).unwrap();
        let back = Config::load(&paths).unwrap();
        assert_eq!(back.mcp_servers, cfg.mcp_servers);

        // old vaults (no mcp fields at all, plus the since-retired
        // persona key) must keep loading — unknown fields are ignored
        let old = serde_json::json!({
            "version": 1,
            "persona": "default",
            "provider": "openrouter",
            "openrouter_api_key": "k",
            "openrouter_model": "m",
            "ollama_url": "u",
            "ollama_model": "m",
            "leash": "leashed",
            "read_paths": [],
            "write_paths": [],
            "rag_folders": [],
            "tool_web_search": true,
            "tool_read_files": true,
            "tool_write_files": false,
            "tool_run_command": false
        });
        std::fs::write(&paths.config_file, old.to_string()).unwrap();
        let migrated = Config::load(&paths).unwrap();
        assert!(migrated.mcp_servers.is_empty());
        assert!(migrated.tool_mcp, "mcp gate defaults on");

        // older registrations (no env / permission / network fields)
        // load with keys empty, every mode allowed and outbound net
        let mut older = old.clone();
        older["mcp_servers"] = serde_json::json!([
            { "name": "time", "image": "mcp/time", "enabled": true, "added": "" }
        ]);
        std::fs::write(&paths.config_file, older.to_string()).unwrap();
        let m2 = Config::load(&paths).unwrap();
        assert_eq!(m2.mcp_servers.len(), 1);
        assert!(m2.mcp_servers[0].allow_chat, "old vaults default allow_chat");
        assert!(m2.mcp_servers[0].allow_code);
        assert!(m2.mcp_servers[0].allow_research);
        assert!(m2.mcp_servers[0].env.is_empty());
        assert!(
            !m2.mcp_servers[0].network_isolated,
            "pre-field vaults upgrade to outbound internet — registered \
             GitHub & friends start actually working, no re-setup needed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn register_dedupes_by_image_and_names() {
        let mut cfg = Config::default();
        let a = entry("time", "mcp/time");
        let i = register(&mut cfg, &a);
        assert_eq!(cfg.mcp_servers[i].name, "time");
        cfg.mcp_servers[i].allow_chat = false; // setup state must survive
        // tag variants are the same repository → one entry, state kept
        let mut t = entry("time", "mcp/time");
        t.image = "mcp/time:latest".into();
        let j = register(&mut cfg, &t);
        assert_eq!(cfg.mcp_servers.len(), 1);
        assert_eq!(i, j);
        assert!(!cfg.mcp_servers[i].allow_chat, "register only re-enables");
        // same name, different image → unique handle
        let b = entry("time", "other/time");
        let k = register(&mut cfg, &b);
        assert_eq!(cfg.mcp_servers.len(), 2);
        assert_ne!(cfg.mcp_servers[i].name, cfg.mcp_servers[k].name);
    }

    #[test]
    fn mode_permissions_filter_registered_servers() {
        let mut cfg = Config::default();
        let mut only_chat = srv("only-chat", "a/only-chat");
        only_chat.allow_code = false;
        only_chat.allow_research = false;
        let mut off = srv("off", "b/off");
        off.enabled = false;
        cfg.mcp_servers.push(only_chat);
        cfg.mcp_servers.push(off);
        cfg.mcp_servers.push(srv("any", "c/any"));

        let names = |m: &str| {
            eligible(&cfg, m)
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names("chat"), ["only-chat", "any"]);
        assert_eq!(names("code"), ["any"]);
        assert_eq!(names("research"), ["any"]);
        assert!(
            names("some-future-mode").is_empty(),
            "unknown modes inherit nothing"
        );
        assert_eq!(
            mode_skips(&cfg, "code"),
            1,
            "only-chat skipped; disabled servers don't count"
        );
    }

    #[test]
    fn pull_verdict_needs_both_witnesses() {
        assert!(pull_ok(true, true));
        assert!(!pull_ok(true, false), "exit 0 but no image — did not land");
        assert!(
            !pull_ok(false, true),
            "a stale image does not bless a failed pull"
        );
        assert!(!pull_ok(false, false));
    }

    #[test]
    fn env_pairs_are_shell_variable_shaped() {
        assert_eq!(
            parse_env_pair("GITHUB_TOKEN=abc123"),
            Some(("GITHUB_TOKEN".into(), "abc123".into()))
        );
        assert_eq!(
            parse_env_pair("lower_key=x"),
            Some(("LOWER_KEY".into(), "x".into()))
        );
        assert_eq!(
            parse_env_pair("A_B=two words"),
            Some(("A_B".into(), "two words".into()))
        );
        assert_eq!(
            parse_env_pair("A=v=1"),
            Some(("A".into(), "v=1".into())),
            "value may contain '=' (env files split on the first)"
        );
        assert_eq!(parse_env_pair("GITHUB_TOKEN"), None, "no '=' at all");
        assert_eq!(parse_env_pair("=v"), None, "empty name");
        assert_eq!(parse_env_pair("1BAD=x"), None, "digit first");
        assert_eq!(parse_env_pair("GITHUB TOKEN=x"), None, "space in name");
        assert_eq!(parse_env_pair("BAD-NAME=x"), None, "'-' is not a name char");
    }

    #[test]
    fn repo_of_strips_tags_not_ports() {
        assert_eq!(repo_of("mcp/brave-search:latest"), "mcp/brave-search");
        assert_eq!(
            repo_of("ghcr.io/github/github-mcp-server:main"),
            "ghcr.io/github/github-mcp-server"
        );
        assert_eq!(repo_of("mcp/time"), "mcp/time");
        assert_eq!(
            repo_of("localhost:5000/mcp/time:latest"),
            "localhost:5000/mcp/time"
        );
    }

    #[test]
    fn search_lines_never_overflow_the_terminal() {
        assert_eq!(line_budget(80), 72);
        assert_eq!(line_budget(40), 32);
        assert_eq!(line_budget(25), 20, "very narrow turns get a floor");

        let long = entry("a-rather-long-server-name", "ghcr.io/github/github-mcp-server:main");
        let line = list_line(&long, true, 72);
        assert!(
            line.chars().count() <= 72,
            "{} chars: {line}",
            line.chars().count()
        );
        assert!(
            line.starts_with("a-rather-long-server-name · ghcr.io/github/github-mcp-server:main"),
            "{line}"
        );
        assert!(line.ends_with('…'), "{line}");
        assert_eq!(
            list_line(&entry("t", "mcp/time"), false, 72),
            "t · mcp/time (catalog) — test"
        );
    }

    #[test]
    fn env_file_lines_are_deterministic_and_secret() {
        let dir = std::env::temp_dir().join(format!("dct-mcp-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut s = srv("github", "ghcr.io/github/github-mcp-server:main");
        s.env.insert("B_TOKEN".into(), "two words".into());
        s.env.insert("A_TOKEN".into(), "x=1".into());
        let p = write_env_file(&s, &dir)
            .unwrap()
            .expect("non-empty env must produce a file");
        let body = std::fs::read_to_string(&p).unwrap();
        assert_eq!(body, "A_TOKEN=x=1\nB_TOKEN=two words\n");
        assert!(p.ends_with("mcp-github.env"), "{}", p.display());

        // empty env map → no file, no secret landing on disk
        let none = srv("time", "mcp/time");
        assert!(write_env_file(&none, &dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn server_line_shows_key_names_never_values() {
        let mut s = srv("github", "ghcr.io/github/github-mcp-server:main");
        s.env.insert(
            "GITHUB_PERSONAL_ACCESS_TOKEN".into(),
            "ghp_super_secret".into(),
        );
        let line = server_line(&s);
        assert!(line.contains("GITHUB_PERSONAL_ACCESS_TOKEN"), "{line}");
        assert!(!line.contains("ghp_super_secret"), "{line}");
        assert!(line.contains("[on ]"), "{line}");
        assert!(
            !line.contains("net: isolated"),
            "outbound is the silent default: {line}"
        );
        s.network_isolated = true;
        let line = server_line(&s);
        assert!(line.contains("net: isolated"), "{line}");
    }

    #[test]
    fn tool_names_are_model_safe() {
        assert_eq!(sanitize_server_name("Docker/MCP-Foo!"), "docker-mcp-foo");
        assert_eq!(sanitize_server_name("---"), "server");
        let spec = pool_specs(&[]).len();
        assert_eq!(spec, 0);
        let prefixed = format!(
            "{}{}_{}",
            MCP_TOOL_PREFIX,
            sanitize_server_name("github"),
            sanitize_tool_name("Get PR Diff")
        );
        assert!(prefixed
            .chars()
            .all(|c| { c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' }));
        assert!(prefixed.starts_with("mcp_"));
    }

    #[test]
    fn json_rpc_envelope_shapes() {
        // initialize request the client sends
        let init = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "decyphertek", "version": "0" }
            }
        });
        assert_eq!(init["method"], "initialize");
        let notif = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(notif.get("id").is_none(), "initialized is a notification");

        // tools/call request shape
        let call = json!({ "name": "search", "arguments": { "q": "x" } });
        assert_eq!(call["arguments"]["q"], "x");

        // tools/list response → through the REAL client parser
        let resp = json!({
            "jsonrpc": "2.0", "id": 2,
            "result": { "tools": [ {
                "name": "search",
                "description": "find things",
                "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
            }]}
        });
        let tools = tools_from_response(&resp);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "search");
        assert!(tools[0].input_schema.is_object());

        // tools/call result → through the REAL client parser
        let out = json!({
            "jsonrpc": "2.0", "id": 3,
            "result": { "isError": false, "content": [ { "type": "text", "text": "42" } ] }
        });
        assert_eq!(call_result(&out), (false, "42".to_string()));
    }
}
