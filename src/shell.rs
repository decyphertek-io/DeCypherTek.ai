//! The slash shell: a classic terminal — the prompt looks like
//! `decyphertek.ai:~$` — where everything you type runs exactly as typed
//! (with a persistent `cd`), and only agent commands (typed as `/command`;
//! the older `@command` prefix still works) wake the agent. A run
//! prints just `Processing Request............`, then one final TUI
//! report, and hands the prompt back.

use crate::config::Config;
use crate::paths::Paths;
use crate::tui;
use crate::vector::Vectors;
use anyhow::{Context, Result};
use std::io::BufRead;
use std::path::PathBuf;

pub const HELP: &str = "\
slash shell — commands run exactly as typed; /-commands wake the agent.

  /chat <task>        conversation backed by full memory — ask it about
                      anything it has read, learned or indexed
  /code <task>        hands-on: read, change, verify, report diffs
  /research <topic>   web research + memory, ends in a written report
  /research <n>.yml <topic>
                      same, but searching only the sites listed in the
                      research profile <n>.yml (create/edit via /setup)
  /rag <folder> *     index every document in the folder into RAG memory
                       (pdf, doc, docx, epub, text, code…; +read grant)
  /rag <folder> <f>   index one document by name
  /mcp                MCP tool servers: search & add (pull + keys +
                       permissions), or set up registered ones —
                       verifies the launch, sets /chat /code /research
                       access; servers run hardened, stdio-only
  /grants read <p>    grant a folder to read
  /grants write <p>   grant a folder to write
  /leash <mode>       leashed | unleashed — take the leash on/off
  /status             current agent, brain, leash, grants, memory
  /setup              customize: brain, memory folders, leash + grants,
                      research profiles (Enter keeps what you already have)
  /password           change the vault password
  /help               this help
  exit                seal the vault and quit
  cd [path]           change directory (~, .., - ; the prompt follows)
  anything else       runs in your shell, untouched

  /rag is differential: unchanged documents cost nothing on a re-run,
  changed documents replace their chunks, deleted documents are pruned.
  Ask /chat about anything you /rag — it answers straight from memory.

  every command also works with the older @-prefix (@chat = /chat)";

pub fn run(
    cfg: &mut Config,
    paths: &Paths,
    vectors: &mut Vectors,
    key: &mut [u8; 32],
    salt: [u8; 16],
) -> Result<()> {
    let stdin = std::io::stdin();
    let mut oldpwd: Option<PathBuf> = None;

    loop {
        tui::draw_prompt(&prompt_cwd());
        let mut line = String::new();
        let n = match stdin.lock().read_line(&mut line) {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break; // EOF (Ctrl-D)
        }
        let cmd = line.trim();
        if cmd.is_empty() {
            continue;
        }

        let result = handle(cfg, paths, vectors, key, &salt, cmd, &mut oldpwd);
        if let Err(e) = result {
            tui::error(&e.to_string());
        }
        if cmd == "exit"
            || cmd == "quit"
            || cmd == "@exit"
            || cmd == "@quit"
            || cmd == "/exit"
            || cmd == "/quit"
        {
            return Ok(());
        }
    }
    Ok(())
}

fn handle(
    cfg: &mut Config,
    paths: &Paths,
    vectors: &mut Vectors,
    key: &mut [u8; 32],
    salt: &[u8; 16],
    cmd: &str,
    oldpwd: &mut Option<PathBuf>,
) -> Result<()> {
    let (head, rest) = match cmd.split_once(' ') {
        Some((h, r)) => (h, r.trim()),
        None => (cmd, ""),
    };

    // Slash commands: `/name` dispatches identically to `@name` — one
    // command set, two sigils. Anything without a sigil still falls through
    // to the plain shell passthrough, untouched.
    let head_owned;
    let head = if let Some(name) = head.strip_prefix('/') {
        head_owned = format!("@{name}");
        &head_owned
    } else {
        head
    };

    match head {
        "exit" | "quit" | "@exit" | "@quit" => Ok(()),
        "@help" | "help" => {
            println!("{HELP}");
            Ok(())
        }
        "@mcp" => {
            crate::mcp::mcp(cfg, paths, rest)?;
            Ok(())
        }
        "@chat" | "@code" => {
            let mode = &head[1..];
            let task = if rest.is_empty() {
                ask_task(mode)?
            } else {
                rest.to_string()
            };
            if task.trim().is_empty() {
                println!("(nothing to do — task was empty)");
                return Ok(());
            }
            run_agent(cfg, paths, vectors, mode, &task, &[])
        }
        "@research" => {
            // Optional leading research profile: "/research <name>.yml <topic>"
            // locks web searching to that profile's sites. A bare first
            // token that matches a saved profile works too.
            let mut sites: Vec<String> = Vec::new();
            let mut topic = rest.to_string();
            let first = rest.split_whitespace().next().unwrap_or("").to_lowercase();
            let profiles = crate::research::list(paths).unwrap_or_default();
            let explicit = first.ends_with(".yml") || first.ends_with(".yaml");
            let known = profiles
                .iter()
                .any(|p| p.trim_end_matches(".yml").trim_end_matches(".yaml") == first);
            if !first.is_empty() && (explicit || known) {
                match crate::research::load(paths, &first) {
                    Ok(profile) => {
                        tui::info(
                            "RESEARCH",
                            &format!(
                                "profile '{}' — searching only: {}",
                                profile.name,
                                profile.sites.join(", ")
                            ),
                        );
                        sites = profile.sites.clone();
                        topic = rest[first.len()..].trim().to_string();
                    }
                    Err(e) => {
                        tui::error(&e.to_string());
                        let have = crate::research::list(paths).unwrap_or_default();
                        if have.is_empty() {
                            tui::info("RESEARCH", "no profiles yet — /setup creates one, or just run /research <topic> for a general web search.");
                        } else {
                            tui::info("RESEARCH", &format!("available: {}", have.join(", ")));
                        }
                        return Ok(());
                    }
                }
            }
            let task = if topic.is_empty() {
                ask_task("research")?
            } else {
                topic
            };
            if task.trim().is_empty() {
                println!("(nothing to do — task was empty)");
                return Ok(());
            }
            run_agent(cfg, paths, vectors, "research", &task, &sites)
        }
        "@rag" => {
            // /rag <folder> *           — index the whole folder
            // /rag <folder> <name.ext>  — index one document
            let mut args = rest.split_whitespace();
            let (Some(folder_arg), file_arg) = (args.next(), args.next().unwrap_or("*")) else {
                tui::info(
                    "USAGE",
                    "/rag <folder> *          — index every document in the folder\n\
                     /rag <folder> <name.ext> — index one document\n\
                     (pdf, doc, docx, epub, text, code — indexed differentially)",
                );
                return Ok(());
            };
            let folder = expand_tilde(folder_arg);
            if !folder.is_dir() {
                tui::error(&format!("'{}' is not a folder", folder_arg));
                return Ok(());
            }
            if file_arg == "*" {
                let report = vectors.ingest_folder(&folder)?;
                let f = folder.to_string_lossy().to_string();
                if !cfg.rag_folders.contains(&f) {
                    cfg.rag_folders.push(f.clone());
                }
                if !cfg.read_paths.contains(&f) {
                    cfg.read_paths.push(f.clone());
                }
                cfg.save(paths).context("save config after rag")?;
                tui::info("RAG", &rag_summary(rest, &report, true));
                return Ok(());
            }
            let path = folder.join(file_arg);
            if !path.is_file() {
                tui::error(&format!("no file '{}' in {}", file_arg, folder_arg));
                return Ok(());
            }
            let report = vectors.ingest_file(&path)?;
            let _ = crate::wiki::refresh_index_page(paths, vectors);
            tui::info("RAG", &rag_summary(file_arg, &report, false));
            Ok(())
        }
        "@grants" => {
            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
            match parts.as_slice() {
                [kind, path] if *kind == "read" => {
                    if !cfg.read_paths.iter().any(|p| p == path) {
                        cfg.read_paths.push(path.to_string());
                        cfg.save(paths)?;
                    }
                    tui::info("GRANTS", &format!("read granted: {path}"));
                    Ok(())
                }
                [kind, path] if *kind == "write" => {
                    if !cfg.write_paths.iter().any(|p| p == path) {
                        cfg.write_paths.push(path.to_string());
                        cfg.save(paths)?;
                    }
                    tui::info("GRANTS", &format!("write granted: {path}"));
                    Ok(())
                }
                _ => {
                    tui::info("USAGE", "/grants read <folder>  or  /grants write <folder>");
                    Ok(())
                }
            }
        }
        "@leash" => match rest {
            "leashed" | "unleashed" => {
                cfg.leash = rest.to_string();
                cfg.save(paths)?;
                let noun = if rest == "unleashed" {
                    "off — every ability is on, folder scopes are gone. \
                     Re-leash anytime: /leash leashed (the tool switches resume)."
                } else {
                    "on — folder scopes enforced; only granted abilities run."
                };
                tui::info("LEASH", noun);
                Ok(())
            }
            _ => {
                tui::info(
                    "LEASH",
                    &format!(
                        "current: {} — use /leash leashed | /leash unleashed",
                        cfg.leash
                    ),
                );
                Ok(())
            }
        },
        "@status" => {
            let n = vectors.count()?;
            let pages = crate::wiki::list(paths)?.len();
            let mcp = if cfg.mcp_servers.is_empty() {
                "(none — /mcp adds them)".to_string()
            } else {
                cfg.mcp_servers
                    .iter()
                    .map(|s| {
                        let keys = if s.env.is_empty() {
                            "keys: none".to_string()
                        } else {
                            format!(
                                "keys: {} (hidden)",
                                s.env.keys().cloned().collect::<Vec<_>>().join(", ")
                            )
                        };
                        format!(
                            "{}{} {} — chat {} · code {} · research {} · {keys}",
                            s.name,
                            if s.enabled { "" } else { " (off)" },
                            s.image,
                            if s.allow_chat { "y" } else { "n" },
                            if s.allow_code { "y" } else { "n" },
                            if s.allow_research { "y" } else { "n" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n               ")
            };
            tui::info(
                "STATUS",
                &format!(
                    "version        {}\nagent           adminotaur (sysadmin)\nbrain           {}\nleash           {}\nread grants     {}\nwrite grants    {}\ntools           {}\nMCP servers    {}{}\nRAG chunks      {}\nwiki pages      {}\nvault           {}",
                    env!("CARGO_PKG_VERSION"),
                    cfg.backend_summary(),
                    cfg.leash,
                    if cfg.read_paths.is_empty() { "(none)".into() } else { cfg.read_paths.join(", ") },
                    if cfg.write_paths.is_empty() { "(none)".into() } else { cfg.write_paths.join(", ") },
                    crate::agent::enabled_tools(cfg),
                    if cfg.tool_mcp { "enabled" } else { "gate off (/setup)" },
                    if cfg.mcp_servers.is_empty() { String::new() } else { format!("\n               {mcp}") },
                    n,
                    pages,
                    paths.vault_file.display(),
                ),
            );
            Ok(())
        }
        "@setup" => {
            let new_cfg = crate::setup::wizard(paths, Some(cfg))?;
            *cfg = new_cfg;
            Ok(())
        }
        "@password" => {
            change_password(paths, key, salt)?;
            Ok(())
        }
        // `cd`: a built-in — each passthrough runs `sh -c` from this
        // process's cwd, so `cd` moves *this* shell and the prompt
        // (and every later command) follows, like a real terminal.
        // Plain paths only: anything with shell syntax (&&, ;, pipes…)
        // falls through to the real shell and runs untouched.
        "cd" => {
            let mut target = rest;
            for q in ['"', '\''] {
                if target.len() >= 2 && target.starts_with(q) && target.ends_with(q) {
                    target = &target[1..target.len() - 1];
                    break;
                }
            }
            if target.chars().any(|c| ";&|<>`$".contains(c)) {
                passthrough(cmd);
            } else {
                builtin_cd(target, oldpwd);
            }
            Ok(())
        }
        // Anything else: passthrough — run in the user's shell, untouched.
        _ => {
            passthrough(cmd);
            Ok(())
        }
    }
}

/// Run a command in the user's shell exactly as typed.
fn passthrough(cmd: &str) {
    use std::process::Command;
    match Command::new("sh").arg("-c").arg(cmd).status() {
        Ok(_) => {}
        Err(e) => tui::error(&format!("could not spawn shell: {e}")),
    }
}

/// The prompt's path component: like `\w` — `~` at home, `~/…` under it,
/// the absolute path anywhere else.
fn prompt_cwd() -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let path = cwd.to_string_lossy().to_string();
    if let Ok(home) = crate::paths::home_dir() {
        let home = home.to_string_lossy().to_string();
        if let Some(rest) = path.strip_prefix(&home) {
            if rest.is_empty() {
                return "~".into();
            }
            if rest.starts_with('/') {
                return format!("~{rest}");
            }
        }
    }
    path
}

/// Expand a leading `~` / `~/…` to the home directory.
fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix('~') {
        if let Ok(home) = crate::paths::home_dir() {
            if rest.is_empty() {
                return home;
            }
            if let Some(rel) = rest.strip_prefix('/') {
                return home.join(rel);
            }
        }
    }
    PathBuf::from(p)
}

/// `cd` with bash semantics: bare `cd` → home, `cd -` → OLDPWD (printing
/// where it went), `cd <path>` with `~` expansion. Errors read like bash's.
fn builtin_cd(rest: &str, oldpwd: &mut Option<PathBuf>) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let target = if rest.is_empty() || rest == "~" {
        crate::paths::home_dir().ok()
    } else if rest == "-" {
        match oldpwd {
            Some(prev) => {
                println!("{}", prev.display());
                Some(prev.clone())
            }
            None => {
                println!("cd: OLDPWD not set");
                None
            }
        }
    } else {
        Some(expand_tilde(rest))
    };
    let Some(target) = target else {
        return;
    };
    match std::env::set_current_dir(&target) {
        Ok(()) => {
            *oldpwd = Some(cwd);
        }
        Err(e) => {
            let reason = match e.kind() {
                std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
                std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
                _ => e.to_string(),
            };
            println!("cd: {rest}: {reason}");
        }
    }
}

/// Multi-line tasks when none was given inline.
fn ask_task(mode: &str) -> Result<String> {
    println!(
        "{}",
        console::style(format!(
            "Give the {mode} task (multi-line; end with a single '.' line):"
        ))
        .dim()
    );
    let stdin = std::io::stdin();
    let mut out = String::new();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim() == "." {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

fn run_agent(
    cfg: &Config,
    paths: &Paths,
    vectors: &mut Vectors,
    mode: &str,
    task: &str,
    research_sites: &[String],
) -> Result<()> {
    println!("Processing Request............");

    match crate::agent::run(cfg, paths, vectors, mode, task, research_sites) {
        Ok(result) => {
            tui::report(&format!("/{mode} — report"), &result.report);
            for w in &result.warnings {
                tui::warn("RUN", w);
            }
            Ok(())
        }
        Err(e) => {
            tui::error(&e.to_string());
            Ok(())
        }
    }
}

/// One readable summary for a `/rag` run's totals.
fn rag_summary(target: &str, r: &crate::vector::IngestReport, granted: bool) -> String {
    if r.indexed == 0 && r.added == 0 && r.skipped.is_empty() {
        return format!("{target}: memory already current — nothing changed.");
    }
    let mut s = format!(
        "{target}: {} file(s) scanned, {} indexed, {} chunk(s) added",
        r.files, r.indexed, r.added
    );
    if r.unsupported > 0 {
        s.push_str(&format!(", {} non-doc skipped", r.unsupported));
    }
    s.push('.');
    if granted {
        s.push_str("\nREAD granted.");
    }
    for note in r.skipped.iter().take(3) {
        s.push_str(&format!("\n  skipped: {note}"));
    }
    s
}

fn change_password(paths: &Paths, key: &mut [u8; 32], salt: &[u8; 16]) -> Result<()> {
    let new_password = crate::setup::prompt_new_password()?;
    let new_key: [u8; 32] = crate::vault::derive_key(&new_password, salt)?;
    // No mid-session seal — staging stays live; the seal-on-exit (or next
    // @-run) writes the vault with the new key. Same salt, new key.
    *key = new_key;
    let _ = paths;
    tui::info("VAULT", "Vault re-keyed. It seals with your NEW password when you exit. Remember it — it is the only key.");
    Ok(())
}
