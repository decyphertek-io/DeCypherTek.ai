//! The `@`-shell: a normal terminal that runs everything you type as typed,
//! and only wakes the agent for @-commands. The run stays silent; the TUI
//! shows one final report, then hands the prompt back.

use crate::config::Config;
use crate::paths::Paths;
use crate::tui;
use crate::vector::Vectors;
use anyhow::{Context, Result};
use std::io::{BufRead, Write};

pub const HELP: &str = "\
@-shell — commands run exactly as typed; @-commands wake the agent.

  @chat <task>        conversation backed by full memory
  @code <task>        hands-on: read, change, verify, report diffs
  @research <topic>   web research + memory, ends in a written report
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
  anything else       runs in your shell, untouched";

pub fn run(
    cfg: &mut Config,
    paths: &Paths,
    vectors: &mut Vectors,
    key: &mut [u8; 32],
    salt: [u8; 16],
) -> Result<()> {
    let stdin = std::io::stdin();
    tui::banner(env!("CARGO_PKG_VERSION"));
    tui::info(
        "VAULT UNSEALED",
        "Memory is live. Type @help for agent commands — anything else runs \
         in your shell exactly as typed. `exit` seals the vault on the way out.",
    );

    loop {
        let status = format!(
            "adminotaur | {} | {}",
            cfg.backend_summary(),
            cfg.leash
        );
        tui::draw_prompt(&status);
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

        let result = handle(cfg, paths, vectors, key, &salt, cmd);
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
        "@chat" | "@code" | "@research" => {
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
            run_agent(cfg, paths, vectors, mode, &task)
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
        // Anything else: passthrough — run in the user's shell, untouched.
        _ => {
            use std::process::Command;
            let status = Command::new("sh").arg("-c").arg(cmd).status();
            match status {
                Ok(s) if !s.success() => {
                    println!("{}", console::style(format!("(exit status: {s})")).dim());
                }
                Ok(_) => {}
                Err(e) => tui::error(&format!("could not spawn shell: {e}")),
            }
            Ok(())
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
) -> Result<()> {
    let spinner_note = console::style(format!(
        "@{mode} running — the screen stays quiet until the report…"
    ))
    .dim();
    println!("{spinner_note}");
    let _ = std::io::stdout().flush();

    match crate::agent::run(cfg, paths, vectors, mode, task) {
        Ok(result) => {
            tui::report(&format!("@{mode} — report"), &result.report);
            tui::report_stat(
                &format!("@{mode} — stats"),
                &format!(
                    "run: {}s | tool calls: {} | iterations: {}",
                    result.elapsed_secs, result.tool_calls, result.iterations
                ),
            );
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
