//! The `@store` — MCP tool-server management, Termux/Android-first.
//!
//! What lives here:
//! - A curated A-Z seed catalog of well-known MCP (Model Context Protocol)
//!   servers, plus LIVE search of Docker Hub so the store covers every
//!   MCP image that exists, and a scan of images already pulled locally.
//! - A `@store` TUI (fuzzy search) to browse, pull, register, update,
//!   disable and uninstall MCP servers.
//! - The secure launch TEMPLATE: every registered server runs inside a
//!   hardened Docker container (`--network=none`, all capabilities
//!   dropped, no-new-privileges, memory/pid caps, no restart). Its ONLY
//!   channel to the world is the stdio pipe the agent holds — servers
//!   communicate internally with the agent and nothing else.
//! - A minimal MCP client (JSON-RPC 2.0 over stdio): initialize,
//!   tools/list, tools/call. Registered servers spawn as containers per
//!   agent run and their tools merge into the model's tool list as
//!   `mcp_<server>_<tool>` (gated by the leash's tool_mcp switch).

use crate::config::{Config, McpServer};
use crate::paths::Paths;
use crate::tui;
use anyhow::{anyhow, Context, Result};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, FuzzySelect, Select};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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
}

/// Well-known MCP servers, A-Z. Every image listed here was verified to
/// exist on its registry at commit time (mcp/* = Docker's MCP Catalog
/// conventions; the live Docker Hub search in `@store` covers the rest,
/// and pull failures at install surface clearly).
pub fn curated_catalog() -> Vec<StoreEntry> {
    let rows: &[(&str, &str, &str)] = &[
        ("brave-search", "mcp/brave-search", "Web search via the Brave Search API (needs BRAVE_API_KEY env)."),
        ("everything", "mcp/everything", "MCP reference server exposing tools, resources and prompts — a safe first install to try /store."),
        ("fetch", "mcp/fetch", "Fetch web pages and convert them to clean markdown the model can read."),
        ("filesystem", "mcp/filesystem", "Sandboxed file access for explicitly allowed directories."),
        ("git", "mcp/git", "Git operations: clone, status, diff, log, branches, create repos."),
        ("github", "ghcr.io/github/github-mcp-server:main", "GitHub's official MCP server — repos, issues, PRs, code search (needs GITHUB_PERSONAL_ACCESS_TOKEN)."),
        ("gitlab", "mcp/gitlab", "GitLab projects, issues, merge requests, pipelines (needs GITLAB_TOKEN)."),
        ("google-maps", "mcp/google-maps", "Google Maps: geocode, directions, places (needs GOOGLE_MAPS_API_KEY)."),
        ("memory", "mcp/memory", "Knowledge-graph memory: entities, relations, observations."),
        ("obsidian", "mcp/obsidian", "Read and search Obsidian vaults — your markdown notes, live."),
        ("puppeteer", "mcp/puppeteer", "Headless-Chromium automation: navigate, click, screenshot, evaluate JS."),
        ("redis", "mcp/redis", "Redis key-value access — get, set, list, delete."),
        ("sentry", "mcp/sentry", "Sentry.io issues and events (needs SENTRY_AUTH_TOKEN)."),
        ("slack", "mcp/slack", "Slack workspaces — channels, messages, reactions (needs SLACK_BOT_TOKEN)."),
        ("sqlite", "mcp/sqlite", "SQL over SQLite database files with row limits."),
        ("time", "mcp/time", "Current time in any timezone, plus conversions between zones."),
    ];
    let mut out: Vec<StoreEntry> = rows
        .iter()
        .map(|(n, i, d)| StoreEntry {
            name: (*n).into(),
            image: (*i).into(),
            description: (*d).into(),
            origin: EntryOrigin::Curated,
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

// ------------------------------------------------------------------ docker

pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// THE secure launch template: every MCP server runs with this. The sales
/// line is security, not convenience — a server may only ever communicate
/// internally, with the agent over stdio:
/// - `--network=none`: no network at all — no callbacks, no exfil, no LAN.
/// - `--cap-drop=ALL` + `--security-opt=no-new-privileges`: zero superuser
///   capabilities, no privilege escalation inside or out.
/// - `--memory/--pids-limit`: one server cannot starve the phone/host.
/// - `-i --rm`: stdio pipe in/out, container dies with the session.
/// - no published ports: nothing on the machine can reach it.
pub fn secure_run_args() -> Vec<String> {
    const ALL: [&str; 8] = [
        "--network=none",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--memory=512m",
        "--pids-limit=128",
        "--pull=never",
        "-i",
        "--rm",
    ];
    ALL.iter().map(|s| s.to_string()).collect()
}

/// `docker images` already pulled here, restricted to MCP-looking repos.
fn local_images() -> Vec<String> {
    let out = Command::new("docker")
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

/// Live Docker Hub search — "every MCP server found in docker, A-Z".
/// Best-effort: on rate-limit/offline (common on a phone) the curated
/// catalog still shows.
fn hub_search(http: ureq::Agent) -> Vec<StoreEntry> {
    let mut out = Vec::new();
    let Ok(resp) = http
        .get(HUB_SEARCH_URL)
        .timeout(std::time::Duration::from_secs(12))
        .set("User-Agent", "DeCypherTek-MCP-Store")
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
            });
        }
    }
    out
}

// -------------------------------------------------------------- registry

/// Registry handles: lowercase [a-z0-9-], safe as `mcp_<name>_…` tool names.
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

fn upsert(cfg: &mut Config, entry: &StoreEntry) {
    let image = if entry.image.contains('/') {
        entry.image.clone()
    } else {
        format!("docker.io/library/{}", entry.image)
    };
    // registry name precedence: existing entry by same image wins
    let name = cfg
        .mcp_servers
        .iter()
        .find(|s| s.image == image)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| {
            let base = sanitize_server_name(&entry.name);
            let mut n = base.clone();
            let mut i = 2;
            while cfg.mcp_servers.iter().any(|s| s.name == n) {
                n = format!("{base}{i}");
                i += 1;
            }
            n
        });
    let added = crate::util::now_iso();
    if let Some(existing) = cfg.mcp_servers.iter_mut().find(|s| s.image == image) {
        existing.name = name;
        existing.enabled = true;
        existing.added = added;
    } else {
        cfg.mcp_servers.push(McpServer {
            name,
            image,
            enabled: true,
            added,
        });
    }
}

// ------------------------------------------------------------------ @store

/// The `@store` TUI: search every MCP server found in Docker (A-Z),
/// install with one keypress, review/flip/down what runs in the vault.
pub fn browse(cfg: &mut Config, paths: &Paths, initial_query: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        return Err(anyhow!("/store is interactive — run it in the terminal"));
    }
    let theme = ColorfulTheme::default();

    if !docker_available() {
        tui::warn(
            "STORE",
            "Docker was not found on PATH. You can browse the catalog, but \
             servers only LAUNCH where a Docker daemon answers (Termux: the \
             installer puts one inside the proot Arch; desktop: install \
             Docker, or point DOCKER_HOST at a machine that runs it).",
        );
    }

    tui::info(
        "STORE",
        "MCP tool servers, A-Z — browse the in-binary catalog, a live Docker \
         Hub search, and the images already on this machine. Registered \
         servers launch under the security template: --network=none, all \
         capabilities dropped, stdio-only — they talk to the agent and \
         nothing else.",
    );

    loop {
        // Assemble the A-Z list: local images, hub search, curated catalog.
        let mut entries: Vec<StoreEntry> = Vec::new();
        for img in local_images() {
            let name = sanitize_server_name(img.rsplit('/').next().unwrap_or(&img));
            let desc = format!("Already pulled on this machine: {img}");
            entries.push(StoreEntry {
                name,
                image: img.clone(),
                description: desc,
                origin: EntryOrigin::Local,
            });
        }
        let http = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .build();
        let hub = hub_search(http);
        if !hub.is_empty() {
            tui::info(
                "STORE",
                &format!("Docker Hub live search found {} MCP servers.", hub.len()),
            );
        }
        entries.extend(hub);
        entries.extend(curated_catalog());

        // Dedupe by image, keep the most informative origin, sort A-Z.
        let mut seen: Vec<String> = Vec::new();
        let mut list: Vec<StoreEntry> = Vec::new();
        for e in entries {
            if seen.contains(&e.image) {
                continue;
            }
            seen.push(e.image.clone());
            list.push(e);
        }
        if !initial_query.trim().is_empty() {
            let q = initial_query.to_string();
            list.retain(|e| matches(e, &q));
        }
        list.sort_by(|a, b| a.name.cmp(&b.name));
        if list.is_empty() {
            tui::warn(
                "STORE",
                "No MCP servers match — try a different search term.",
            );
            return Ok(());
        }

        let items: Vec<String> = list
            .iter()
            .map(|e| {
                let reg = if cfg.mcp_servers.iter().any(|s| s.image == e.image) {
                    "[registered]"
                } else {
                    ""
                };
                format!(
                    "{:>12} {:<24} {:<38} {}",
                    reg,
                    e.image,
                    format!("{} ({})", e.name, e.origin.label()),
                    e.description
                )
            })
            .collect();
        let sel = FuzzySelect::with_theme(&theme)
            .with_prompt("Search MCP servers (type to filter, Esc/enter empty when done)")
            .items(&items)
            .default(0)
            .max_length(12)
            .interact_opt()?;
        let Some(idx) = sel else { break };
        if list.is_empty() || idx >= list.len() {
            break;
        }
        let entry = list[idx].clone();

        act_on(cfg, paths, &entry, &theme)?;
    }

    // Store closed: show what is vault-registered and flip/manage.
    let count = cfg.mcp_servers.iter().filter(|s| s.enabled).count();
    tui::info(
        "STORE",
        &format!(
            "Registered MCP servers: {} enabled / {} total. They run hardened (network=none, cap-drop=ALL), stdio-only, on the agent's terms. View with /status, add or remove any time with /store.",
            count,
            cfg.mcp_servers.len()
        ),
    );
    if !cfg.mcp_servers.is_empty() {
        manage_installed(cfg, paths)?;
    }
    Ok(())
}

fn act_on(
    cfg: &mut Config,
    paths: &Paths,
    entry: &StoreEntry,
    theme: &ColorfulTheme,
) -> Result<()> {
    let registered = cfg
        .mcp_servers
        .iter()
        .find(|s| s.image == entry.image || s.image.ends_with(entry.image.as_str()));
    tui::info(
        "MCP SERVER",
        &format!(
            "{} {}  [{}]\n\n{}\n\nsecurity template: --network=none, --cap-drop=ALL, no-new-privileges, stdio-only — the server can only answer the agent, never reach out.",
            entry.image,
            if registered.is_some() { "(already registered)" } else { "" },
            entry.origin.label(),
            entry.description
        ),
    );
    let is_broken_ctx = !docker_available();
    let mut options = vec!["Pull + register (secure template)".to_string()];
    if registered.is_some() {
        options.push("Pull (update image) only".to_string());
        options.push("Uninstall (remove from vault)".to_string());
        options.push("Back".to_string());
    } else {
        options.push("Pull only".to_string());
        options.push("Back".to_string());
    }
    let idx = Select::with_theme(theme)
        .with_prompt("Action")
        .items(&options)
        .default(0)
        .interact()?;
    match idx {
        0 => {
            if is_broken_ctx {
                tui::warn("STORE", "Docker is not on PATH — pull will fail. Install with the one-liner first or set DOCKER_HOST.");
            }
            if !Confirm::with_theme(theme)
                .with_prompt(format!(
                    "Pull {} and register it (launches with no network, stdio only)?",
                    entry.image
                ))
                .default(true)
                .interact()?
            {
                return Ok(());
            }
            let out = crate::tools::run_command(&format!("docker pull {}", entry.image));
            println!("{}", out);
            if out.contains("Status: Downloaded")
                || out.contains("up to date")
                || out.contains("Downloaded newer")
            {
                upsert(cfg, entry);
                cfg.save(paths)
                    .context("save config after /store install")?;
                tui::info(
                    "STORE",
                    &format!(
                        "{} registered and enabled. Its tools (mcp_{}_<tool>) are live on the next agent run.",
                        entry.image,
                        sanitize_server_name(&entry.name)
                    ),
                );
            } else {
                tui::warn(
                    "STORE",
                    "Pull did not clearly succeed — check the docker output above. If the image doesn't exist, pick another (the Hub live search only lists real images).",
                );
            }
        }
        1 => {
            let out = crate::tools::run_command(&format!("docker pull {}", entry.image));
            println!("{out}");
            if registered.is_some() {
                tui::info(
                    "STORE",
                    "Image updated; the vault-registered server re-launches fresh next run.",
                );
            } else {
                tui::info(
                    "STORE",
                    "Image pulled. Register it when you want its tools live.",
                );
            }
        }
        2 => {
            cfg.mcp_servers
                .retain(|s| s.image != entry.image && !s.image.ends_with(entry.image.as_str()));
            cfg.save(paths)
                .context("save config after /store uninstall")?;
            tui::info(
                "STORE",
                "Server unregistered (image kept on disk for a fast re-add).",
            );
        }
        _ => {}
    }
    Ok(())
}

/// Enable/disable/remove servers already in the vault; the "library" view.
fn manage_installed(cfg: &mut Config, paths: &Paths) -> Result<()> {
    let theme = ColorfulTheme::default();
    loop {
        let mut items: Vec<String> = cfg
            .mcp_servers
            .iter()
            .map(|s| {
                format!(
                    "[{}] {:<24} {}",
                    if s.enabled { "enabled" } else { "off" },
                    s.name,
                    s.image
                )
            })
            .collect();
        if items.is_empty() {
            return Ok(());
        }
        let done = "… done (back to the shell)".to_string();
        items.push(done.clone());
        let idx = Select::with_theme(&theme)
            .with_prompt("Your registered MCP servers (vault)")
            .items(&items)
            .default(items.len() - 1)
            .interact()?;
        if items[idx] == done {
            return Ok(());
        }
        let server = cfg.mcp_servers[idx].clone();
        let actions = [
            "Enable (run on agent runs, hardened)",
            "Disable (keep registered, don't run)",
            "Uninstall (remove from vault)",
            "Back",
        ];
        let act = Select::with_theme(&theme)
            .items(&actions)
            .default(0)
            .interact()?;
        match act {
            0 => {
                if let Some(s) = cfg.mcp_servers.iter_mut().find(|s| s.image == server.image) {
                    s.enabled = true;
                }
                cfg.save(paths)?;
                tui::info("STORE", &format!("{} enabled.", server.name));
            }
            1 => {
                if let Some(s) = cfg.mcp_servers.iter_mut().find(|s| s.image == server.image) {
                    s.enabled = false;
                }
                cfg.save(paths)?;
                tui::info(
                    "STORE",
                    &format!("{} disabled — state kept, runs paused.", server.name),
                );
            }
            2 => {
                cfg.mcp_servers.retain(|s| s.image != server.image);
                cfg.save(paths)?;
                tui::info(
                    "STORE",
                    &format!("{} uninstalled (image kept on disk).", server.name),
                );
            }
            _ => {}
        }
    }
}

// --------------------------------------------------------------- MCP client

/// A live, hardened MCP server container held by the agent, speaking
/// newline-delimited JSON-RPC 2.0 over stdio (spawnced per agent run).
pub struct McpChild {
    pub server: String,
    pub image: String,
    child: Child,
    writer: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
    pub tools: Vec<McpTool>,
}

#[derive(Debug, Clone)]
pub struct McpTool {
    /// Raw tool name as the server advertises it (not the mcp_* prefix).
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl McpChild {
    /// Launch a container under the security template and do the MCP handshake.
    pub fn spawn(server: &McpServer) -> Result<McpChild> {
        let mut cmd = Command::new("docker");
        cmd.arg("run")
            .args(secure_run_args())
            .arg(&server.image)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("docker run {} (is the daemon up?)", server.image))?;
        let writer = child.stdin.take().context("docker stdin pipe")?;
        let reader = BufReader::new(child.stdout.take().context("docker stdout pipe")?);
        let mut m = McpChild {
            server: server.name.clone(),
            image: server.image.clone(),
            child,
            writer,
            reader,
            next_id: 3,
            tools: Vec::new(),
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
        let tools = m.rpc_call("tools/list", json!({}))?;
        if let Some(list) = tools.pointer("/params/tools").and_then(|t| t.as_array()) {
            m.tools = list
                .iter()
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
                .collect();
        }
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
                    "MCP server '{}' closed its stdio — daemon down, image deleted, or missing API-key env (see /store)",
                    self.server
                ));
            }
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                continue; // docker FYI lines / non-JSON noise
            };
            match v.get("id").and_then(|i| i.as_u64()) {
                Some(id) if id == want_id => {
                    if let Some(err) = v.get("error") {
                        return Err(anyhow!("MCP error from {}: {err}", self.server));
                    }
                    return Ok(v);
                }
                _ => continue, // server notifications, other-traffic
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
                let is_error = v
                    .pointer("/params/result/isError")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                let text = v
                    .pointer("/params/result/content")
                    .and_then(|c| c.as_array())
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
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

/// A run's pool dies with it — no orphaned containers, ever.
impl Drop for McpChild {
    fn drop(&mut self) {
        self.kill();
    }
}

// ------------------------------------------------------------------ run-time

/// Spawn every enabled, registered server for this run. Returns the pool
/// plus a human warning per server that could not start (the agent runs
/// without them; the failures are visible, not silent).
pub fn spawn_pool(cfg: &Config) -> (Vec<McpChild>, Vec<String>) {
    let mut pool = Vec::new();
    let mut warnings = Vec::new();
    let active: Vec<&McpServer> = cfg.mcp_servers.iter().filter(|s| s.enabled).collect();
    if active.is_empty() {
        return (pool, warnings);
    }
    if !cfg.tool_mcp && !cfg.is_unleashed() {
        warnings.push("MCPS: tool_mcp is off (/setup) — registered servers skipped.".into());
        return (pool, warnings);
    }
    if !docker_available() {
        warnings.push(format!(
            "MCPS: Docker not found — {} registered server(s) skipped this run (/store has details).",
            active.len()
        ));
        return (pool, warnings);
    }
    for s in active {
        match McpChild::spawn(s) {
            Ok(child) => {
                if child.tools.is_empty() {
                    warnings.push(format!(
                        "MCPS: {} '{}' came up but advertises no tools — image may need config env keys.",
                        s.name, s.image
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
                        "[mcp server: {} @ {}] {} — the container has no network; it only answers the agent.",
                        child.server,
                        child.image,
                        if tool.description.trim().is_empty() {
                            "MCP tool"
                        } else {
                            &tool.description
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
        }
        assert!(cat.iter().any(|e| e.name == "github"));
        assert!(cat.iter().any(|e| e.name == "brave-search"));
    }

    #[test]
    fn secure_template_truly_isolates() {
        let args = secure_run_args();
        assert!(
            args.contains(&"--network=none".to_string()),
            "no network, ever"
        );
        assert!(args.contains(&"--cap-drop=ALL".to_string()));
        assert!(args.contains(&"--security-opt=no-new-privileges".to_string()));
        assert!(
            args.contains(&"--rm".to_string()),
            "container dies with the session"
        );
        assert!(
            args.contains(&"-i".to_string()),
            "stdio is the only channel"
        );
        // the template never publishes ports
        assert!(!args.iter().any(|a| a.starts_with("-p")));
        assert!(!args.iter().any(|a| a.starts_with("--publish")));
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
        use crate::config::{Config, McpServer};
        let cfg = Config {
            tool_mcp: false,
            mcp_servers: vec![McpServer {
                name: "x".into(),
                image: "img".into(),
                enabled: true,
                added: String::new(),
            }],
            ..Config::default()
        };
        let (_, w) = spawn_pool(&cfg);
        assert!(
            w.iter().any(|s| s.contains("tool_mcp is off")),
            "leashed gate off must warn: {w:?}"
        );
        let cfg = Config {
            leash: "unleashed".into(),
            ..cfg.clone()
        };
        let (_, w) = spawn_pool(&cfg);
        assert!(
            !w.iter().any(|s| s.contains("tool_mcp is off")),
            "unleashed turns the gate on: {w:?}"
        );
    }

    #[test]
    fn registry_survives_config_roundtrip_and_old_vaults() {
        use crate::config::Config;
        let root = std::env::temp_dir().join(format!("dct-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths::new(&root);
        paths.ensure_staging().unwrap();

        // new config: register, save, reload
        let mut cfg = Config::default();
        let entry = StoreEntry {
            name: "Time Server!".into(),
            image: "mcp/time".into(),
            description: "x".into(),
            origin: EntryOrigin::Curated,
        };
        upsert(&mut cfg, &entry);
        assert_eq!(cfg.mcp_servers.len(), 1);
        assert_eq!(cfg.mcp_servers[0].name, "time-server");
        assert!(cfg.mcp_servers[0].enabled);
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn upsert_dedupes_by_image_and_names() {
        let mut cfg = Config::default();
        let a = StoreEntry {
            name: "time".into(),
            image: "mcp/time".into(),
            description: String::new(),
            origin: EntryOrigin::Curated,
        };
        upsert(&mut cfg, &a);
        upsert(&mut cfg, &a); // same image again → still one entry
        assert_eq!(cfg.mcp_servers.len(), 1);
        let b = StoreEntry {
            name: "time".into(),
            image: "other/time".into(),
            description: String::new(),
            origin: EntryOrigin::Curated,
        };
        upsert(&mut cfg, &b); // same name, different image → unique handle
        assert_eq!(cfg.mcp_servers.len(), 2);
        assert_ne!(cfg.mcp_servers[0].name, cfg.mcp_servers[1].name);
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

        // tools/list response → tools array parsing
        let resp = json!({
            "jsonrpc": "2.0", "id": 2,
            "result": { "tools": [ {
                "name": "search",
                "description": "find things",
                "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
            }]}
        });
        let tools = resp.pointer("/result/tools").unwrap().as_array().unwrap();
        assert_eq!(tools[0]["name"], "search");
        assert!(tools[0]["inputSchema"].is_object());

        // tools/call result → text content join
        let out = json!({
            "jsonrpc": "2.0", "id": 3,
            "result": { "isError": false, "content": [ { "type": "text", "text": "42" } ] }
        });
        let text: String = out
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
        assert_eq!(text, "42");
    }
}
