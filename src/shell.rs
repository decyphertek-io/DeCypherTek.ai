//! The `@`-shell: a classic terminal — the prompt looks like
//! `decyphertek.ai:~$` — where everything you type runs exactly as typed
//! (with a persistent `cd`), and only @-commands wake the agent. A run
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
@-shell — commands run exactly as typed; @-commands wake the agent.

  @chat <task>        conversation backed by full memory
  @code <task>        hands-on: read, change, verify, report diffs
  @research <topic>   web research + memory, ends in a written report
  @research <n>.yml <topic>
                      same, but searching only the sites listed in the
                      research profile <n>.yml (create/edit via @setup)
  @upload             pick files from a folder browser (Downloads etc.) —
                      they land in the wiki's info/ folder and RAG memory
  @store              MCP tool-server store: search Docker A-Z (TUI), pull,
                      register — servers launch hardened, internal-only
  @ingest <folder>    chunk a folder's docs into RAG memory (+read grant)
  @grants read <p>    grant a folder to read
  @grants write <p>   grant a folder to write
  @leash <mode>       leashed | unleashed — take the leash on/off
  @status             current agent, brain, leash, grants, memory
  @wiki list          list wiki memory pages
  @wiki read <name>   print a wiki page
  @setup              re-run the walkthrough (brain, grants, tools)
  @password           change the vault password
  @help               this help
  exit                seal the vault and quit
  cd [path]           change directory (~, .., - ; the prompt follows)
  anything else       runs in your shell, untouched";

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
        if cmd == "exit" || cmd == "quit" || cmd == "@exit" || cmd == "@quit" {
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

    match head {
        "exit" | "quit" | "@exit" | "@quit" => Ok(()),
        "@help" | "help" => {
            println!("{HELP}");
            Ok(())
        }
        "@store" => {
            crate::store::browse(cfg, paths, rest)?;
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
            // Optional leading research profile: "@research <name>.yml <topic>"
            // locks web searching to that profile's sites. A bare first
            // token that matches a saved profile works too.
            let mut sites: Vec<String> = Vec::new();
            let mut topic = rest.to_string();
            let first = rest.split_whitespace().next().unwrap_or("").to_lowercase();
            let profiles = crate::research::list(paths).unwrap_or_default();
            let explicit = first.ends_with(".yml") || first.ends_with(".yaml");
            let known = profiles.iter().any(|p| {
                p.trim_end_matches(".yml").trim_end_matches(".yaml") == first
            });
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
                            tui::info("RESEARCH", "no profiles yet — @setup creates one, or just run @research <topic> for a general web search.");
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
        "@upload" => {
            upload_docs(paths, vectors)?;
            Ok(())
        }
        "@ingest" => {
            if rest.is_empty() {
                tui::info(
                    "USAGE",
                    "@ingest <folder> — chunk a folder's docs into RAG memory.",
                );
                return Ok(());
            }
            let folder = std::path::PathBuf::from(rest);
            if !folder.is_dir() {
                tui::error(&format!("'{}' is not a folder", rest));
                return Ok(());
            }
            let (files, added) = vectors.ingest_folder(&folder)?;
            let f = folder.to_string_lossy().to_string();
            if !cfg.rag_folders.contains(&f) {
                cfg.rag_folders.push(f.clone());
            }
            if !cfg.read_paths.contains(&f) {
                cfg.read_paths.push(f.clone());
            }
            cfg.save(paths).context("save config after ingest")?;
            tui::info(
                "RAG",
                &format!("{rest}: {files} files scanned, {added} chunks added. READ granted."),
            );
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
                    tui::info("USAGE", "@grants read <folder>  or  @grants write <folder>");
                    Ok(())
                }
            }
        }
        "@leash" => match rest {
            "leashed" | "unleashed" => {
                cfg.leash = rest.to_string();
                cfg.save(paths)?;
                let noun = if rest == "unleashed" {
                    "off — scope checks are gone; tool grants still apply."
                } else {
                    "on — folder scopes enforced."
                };
                tui::info("LEASH", noun);
                Ok(())
            }
            _ => {
                tui::info(
                    "LEASH",
                    &format!(
                        "current: {} — use @leash leashed | @leash unleashed",
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
                "(none — @store adds them)".to_string()
            } else {
                cfg.mcp_servers
                    .iter()
                    .map(|s| {
                        format!(
                            "{}{} {}",
                            s.name,
                            if s.enabled { "" } else { " (off)" },
                            s.image
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
                    if cfg.tool_mcp { "enabled" } else { "gate off (@setup)" },
                    if cfg.mcp_servers.is_empty() { String::new() } else { format!("\n               {mcp}") },
                    n,
                    pages,
                    paths.vault_file.display(),
                ),
            );
            Ok(())
        }
        "@wiki" => {
            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
            match parts.as_slice() {
                ["list"] => {
                    let pages = crate::wiki::list(paths)?;
                    tui::info(
                        "WIKI",
                        &if pages.is_empty() {
                            "(empty)".to_string()
                        } else {
                            pages.join(", ")
                        },
                    );
                    Ok(())
                }
                ["read", name] => {
                    let body = crate::wiki::read(paths, name)?;
                    println!("{body}");
                    Ok(())
                }
                _ => {
                    tui::info("USAGE", "@wiki list  |  @wiki read <name>");
                    Ok(())
                }
            }
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
            if rest.starts_with('/') {
                return home.join(&rest[1..]);
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
            tui::report(&format!("@{mode} — report"), &result.report);
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

/// @upload: interactive folder browser — the picker opens at Downloads
/// (or /sdcard/Download on Android) and walks the folder structure;
/// picked files are copied into the wiki's info/ folder and chunked
/// into RAG memory, so they seal into the vault and the agent can read
/// them (wiki + memory). Text files are chunked; binaries are stored
/// as-is with a note.
fn upload_docs(paths: &Paths, vectors: &mut Vectors) -> Result<()> {
    use dialoguer::theme::ColorfulTheme;
    use dialoguer::Select;

    let theme = ColorfulTheme::default();
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    let mut dir = [
        std::path::PathBuf::from("/sdcard/Download"),
        home.join("storage/downloads"),
        home.join("Downloads"),
    ]
    .into_iter()
    .find(|p| p.is_dir())
    .unwrap_or(home);

    tui::info(
        "UPLOAD",
        "Pick files to upload: they are copied into the agent's info/ wiki \
         folder and chunked into RAG memory — sealed into the vault, \
         readable by the agent. Folders navigate; '(done)' finishes.",
    );

    loop {
        let mut dirs: Vec<String> = Vec::new();
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }
                    if entry.path().is_dir() {
                        dirs.push(name);
                    } else {
                        files.push(entry.path());
                    }
                }
            }
            Err(e) => {
                tui::error(&format!("cannot read {}: {e}", dir.display()));
                break;
            }
        }
        dirs.sort();
        files.sort();

        let up = dir
            .parent()
            .filter(|p| *p != dir && p.is_dir())
            .map(|p| p.to_path_buf());
        let mut items: Vec<String> = vec!["(done — finish uploading)".into()];
        if up.is_some() {
            items.push("(go up one folder)".into());
        }
        let base = items.len();
        items.extend(dirs.iter().map(|d| format!("{d}/")));
        items.extend(
            files
                .iter()
                .map(|f| f.file_name().unwrap_or_default().to_string_lossy().to_string()),
        );

        let sel = Select::with_theme(&theme)
            .with_prompt(format!("Folder: {}", dir.display()))
            .items(&items)
            .default(0)
            .interact()?;
        if sel == 0 {
            break;
        }
        if up.is_some() && sel == 1 {
            dir = up.unwrap();
            continue;
        }
        let idx = sel - base;
        if idx < dirs.len() {
            dir = dir.join(&dirs[idx]);
            continue;
        }
        let file = &files[idx - dirs.len()];
        let raw_name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match crate::wiki::info_file_path(paths, &raw_name) {
            Ok(dest) => match std::fs::copy(file, &dest) {
                Ok(_) => match std::fs::read_to_string(&dest) {
                    Ok(text) => {
                        let (chunks, added) =
                            vectors.insert_text(&format!("info/{raw_name}"), "info-doc", &text)?;
                        tui::info(
                            "UPLOAD",
                            &format!(
                                "{raw_name} → info/ — {added} of {chunks} chunks into RAG \
                                 (readable via memory and @wiki read info/{raw_name})"
                            ),
                        );
                    }
                    Err(_) => tui::warn(
                        "UPLOAD",
                        &format!(
                            "{raw_name} copied into info/ — binary, stored but not \
                             text-chunked into RAG"
                        ),
                    ),
                },
                Err(e) => tui::error(&format!("copy failed: {e}")),
            },
            Err(e) => tui::error(&format!("bad file name '{raw_name}': {e}")),
        }
    }
    Ok(())
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
