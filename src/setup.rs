//! First-run TUI walkthrough — "Welcome to DeCypherTek.ai".
//! Backend (OpenRouter key/model, or Ollama with slim phone models on
//! Termux), memory folders (RAG + read grants), the Leash and tool
//! grants. main.rs handles the vault password + sealing around this.
//! The personality is fixed: ADMINOTAUR, the sysadmin operator.

use crate::config::Config;
use crate::models::{Client, OLLAMA_SLIM_MODELS, OPENROUTER_DEFAULT_MODELS};
use crate::paths::Paths;
use crate::tui;
use crate::vector::Vectors;
use anyhow::{Context, Result};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, MultiSelect, Password, Select};
use std::path::PathBuf;

/// Ask for a new vault password (twice, min 8 chars).
pub fn prompt_new_password() -> Result<String> {
    loop {
        let a = Password::new()
            .with_prompt("Vault password (encrypts everything in ~/.decyphertek.ai)")
            .interact()?;
        if a.len() < 8 {
            tui::warn(
                "PASSWORD",
                "At least 8 characters — this key encrypts your whole agent.",
            );
            continue;
        }
        let b = Password::new()
            .with_prompt("Repeat vault password")
            .interact()?;
        if a == b {
            return Ok(a);
        }
        tui::warn("PASSWORD", "Passwords did not match — try again.");
    }
}

/// Ask for the unlock password. Attempts limited by the caller.
pub fn prompt_unlock() -> Result<String> {
    Password::new()
        .with_prompt("Vault password (unlock ~/.decyphertek.ai)")
        .interact()
        .map_err(Into::into)
}

/// Run the full walkthrough; returns the resulting Config (already saved).
#[allow(clippy::too_many_lines)]
pub fn wizard(paths: &Paths, existing: Option<&Config>) -> Result<Config> {
    let theme = ColorfulTheme::default();
    let is_termux = crate::util::is_termux();

    tui::banner(env!("CARGO_PKG_VERSION"));
    tui::info(
        "WELCOME",
        "Your agent is ADMINOTAUR — the sysadmin AI that operates the whole \
          system (it can even build subagents). This walkthrough wires up \
          its brain (OpenRouter or Ollama), memory (folders it reads and \
          learns from), the Leash (what it may do) — and the vault password \
          that encrypts all of it under ~/.decyphertek.ai/.",
    );

    let mut cfg = existing.cloned().unwrap_or_default();

    // 1. Backend.
    let backends = vec![
        "OpenRouter — hosted models, one API key, nothing to install (recommended)",
        "Ollama — local models, no cloud; on Termux: slim phone models",
    ];
    let backend_idx = Select::with_theme(&theme)
        .with_prompt("Choose the brain (model backend)")
        .items(&backends)
        .default(0)
        .interact()?;
    if backend_idx == 0 {
        cfg.provider = "openrouter".into();
        choose_openrouter(&mut cfg, &theme)?;
    } else {
        cfg.provider = "ollama".into();
        choose_ollama(&mut cfg, &theme, is_termux)?;
    }

    // 2. Memory folders = RAG ingest (+ optional read grant).
    tui::info(
        "MEMORY",
        "Which folders hold the docs/knowledge the agent should chunk into \
         its RAG memory and read from? Comma-separated paths, or leave empty.",
    );
    let folders_raw: String = Input::with_theme(&theme)
        .with_prompt("Memory folders")
        .allow_empty(true)
        .default("".into())
        .interact()?;
    let mut folders: Vec<String> = Vec::new();
    for raw in folders_raw.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let p = PathBuf::from(raw);
        match p.exists() {
            true => folders.push(p.to_string_lossy().to_string()),
            false => tui::warn("MEMORY", &format!("'{raw}' does not exist — skipped.")),
        }
    }
    folders.dedup();
    let grant_read = !folders.is_empty()
        && Confirm::with_theme(&theme)
            .with_prompt("Grant READ permission on these folders too (yes = they also show up in read grants)")
            .default(true)
            .interact()?;
    cfg.rag_folders = folders.clone();
    if grant_read {
        for f in &folders {
            if !cfg.read_paths.contains(f) {
                cfg.read_paths.push(f.clone());
            }
        }
    }

    // 3. The Leash + the abilities it gets: leashed walks a clear
    //    can/can't list with space-to-toggle grants; unleashed shows
    //    everything it could do and takes one informed yes.
    choose_leash_and_tools(&mut cfg, &theme)?;

    // 4. Research profiles (optional) — a YAML site list saved into the
    //    wiki's research/ folder; `/research <name>.yml <topic>` then
    //    searches exactly those sites instead of the general web.
    let mut new_profile: Option<(String, String, Vec<String>)> = None;
    let existing = crate::research::list(paths).unwrap_or_default();
    if !existing.is_empty() {
        tui::info(
            "RESEARCH",
            &format!("existing profiles: {}", existing.join(", ")),
        );
    }
    if Confirm::with_theme(&theme)
        .with_prompt(
            "Create a research profile now? (a YAML site list — /research <name>.yml \
             searches only those sites)",
        )
        .default(false)
        .interact()?
    {
        let name: String = Input::with_theme(&theme)
            .with_prompt("Profile name (used as /research <name>.yml)")
            .with_initial_text("my-sources")
            .interact_text()?;
        let description: String = Input::with_theme(&theme)
            .with_prompt("Short description (optional)")
            .allow_empty(true)
            .default("".into())
            .interact_text()?;
        let sites_raw: String = Input::with_theme(&theme)
            .with_prompt(
                "Sites to search, comma-separated (e.g. arxiv.org, https://huggingface.co)",
            )
            .interact_text()?;
        let sites: Vec<String> = sites_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if sites.is_empty() {
            tui::warn("RESEARCH", "no sites given — profile not created.");
        } else {
            new_profile = Some((
                name.trim().to_lowercase().replace(' ', "-"),
                description.trim().to_string(),
                sites,
            ));
        }
    }

    // 5. Persist: config, baseline wiki, RAG ingest.
    paths.ensure_staging()?;
    crate::wiki::write_baseline(paths)?;
    cfg.save(paths)?;

    if let Some((name, description, sites)) = new_profile {
        match crate::research::save(paths, &format!("{name}.yml"), &name, &description, &sites) {
            Ok(()) => tui::info(
                "RESEARCH",
                &format!(
                    "profile saved: research/{name}.yml — run /research {name}.yml <topic> \
                     to search only: {}",
                    sites.join(", ")
                ),
            ),
            Err(e) => tui::warn("RESEARCH", &format!("could not save profile: {e}")),
        }
    }

    let vectors = Vectors::open(&paths.vector_db)?;
    for folder in &folders {
        let (files, added) = vectors
            .ingest_folder(&PathBuf::from(folder))
            .with_context(|| format!("ingesting {folder}"))?;
        tui::info(
            "RAG",
            &format!("{folder}: {files} files scanned, {added} chunks added."),
        );
    }
    if folders.is_empty() {
        tui::info(
            "RAG",
            "No folders given — use /ingest <folder> anytime to teach it new docs.",
        );
    }
    if grant_read {
        tui::info(
            "PERMISSIONS",
            &format!("READ granted on: {}", cfg.read_paths.join(", ")),
        );
    }

    Ok(cfg)
}

/// The Leash step plus the abilities that come with it.
/// Leashed: one panel lists what ADMINOTAUR can and cannot do, then
/// every ability is offered as a space-to-toggle grant. Unleashed:
/// one panel lists everything it could do, then a single informed
/// "Do you really accept?" — agreeing turns every ability on and
/// drops folder scopes; declining stays leashed with the grant menu.
fn choose_leash_and_tools(cfg: &mut Config, theme: &ColorfulTheme) -> Result<()> {
    let leash_opts = vec![
        "Leashed — granted folders + granted abilities only (can/can't menu next)",
        "Unleashed — everything allowed (the full list next, then one accept)",
    ];
    let idx = Select::with_theme(theme)
        .with_prompt("The Leash — keep it on or take it off?")
        .items(&leash_opts)
        .default(0)
        .interact()?;

    let mut unleashed = idx == 1;
    if unleashed {
        unleashed = confirm_unleashed(theme)?;
        if !unleashed {
            tui::info(
                "THE LEASH",
                "Not accepted — staying LEASHED. Grant abilities one by one below.",
            );
        }
    }
    cfg.leash = if unleashed {
        "unleashed".into()
    } else {
        "leashed".into()
    };

    if unleashed {
        // One accept covers everything — all abilities on, scopes off.
        cfg.tool_web_search = true;
        cfg.tool_read_files = true;
        cfg.tool_write_files = true;
        cfg.tool_run_command = true;
        cfg.tool_mcp = true;
        tui::info(
            "UNLEASHED",
            "Accepted — every ability is on, folder scopes are off. Back out anytime: \
             /leash leashed. Re-tune grants: /grants read|write <dir>, /setup.",
        );
    } else {
        tui::info(
            "LEASHED — CAN AND CANNOT",
            "CAN (always): chat with you · remember into its encrypted RAG memory \
             + wiki · use its own ~/.decyphertek.ai\n\
             CAN (only if you enable it below — SPACE toggles): search the web · \
             read files in granted folders · write files in granted folders · \
             run shell commands (each asks y/N first) · run MCP tool servers \
             from /store (docker; hardened, internal-only)\n\
             CANNOT: read or write outside granted folders (DENIED, logged) · \
             run a command without your y/N · use anything left off below\n\
             Widen later anytime: /grants read|write <dir> · redo this walk: /setup",
        );
        let tool_items = vec![
            "web_search — research the web (keyless DuckDuckGo + Wikipedia)",
            "read_files — read files in granted folders",
            "write_files — write files in granted folders",
            "run_command — execute shell commands (leashed asks y/N before each)",
            "mcp_servers — run MCP tool servers from /store (docker; hardened, internal-only)",
        ];
        let defaults = vec![
            cfg.tool_web_search,
            cfg.tool_read_files,
            cfg.tool_write_files,
            cfg.tool_run_command,
            cfg.tool_mcp,
        ];
        let chosen = MultiSelect::with_theme(theme)
            .with_prompt("Grant tools (space to toggle)")
            .items(&tool_items)
            .defaults(&defaults)
            .interact()?;
        cfg.tool_web_search = chosen.contains(&0);
        cfg.tool_read_files = chosen.contains(&1);
        cfg.tool_write_files = chosen.contains(&2);
        cfg.tool_run_command = chosen.contains(&3);
        cfg.tool_mcp = chosen.contains(&4);
    }
    Ok(())
}

/// The unleashed side of the bargain: everything the agent can do
/// with the leash off — and the one question that switches it all on.
fn confirm_unleashed(theme: &ColorfulTheme) -> Result<bool> {
    tui::warn(
        "UNLEASHED — WHAT ADMINOTAUR CAN DO",
        "• READ any file your user can reach — not just granted folders\n\
         • WRITE, change and delete files anywhere your user can write\n\
         • RUN any shell command — no per-command y/N ask anymore\n\
         • SEARCH the web (keyless DuckDuckGo + Wikipedia)\n\
         • RUN MCP tool servers from /store (docker; hardened, network=none, stdio-only)\n\
         • BUILD subagents that carry this same access\n\n\
         Nothing limits it beyond your user's own permissions — if it \
         misbehaves it can wreck your home directory. It is logged, \
         but only you can stop it.",
    );
    let yes = Confirm::with_theme(theme)
        .with_prompt(
            "Do you really accept? (Yes = everything above — all abilities on, folder scopes off)",
        )
        .default(false)
        .interact()?;
    Ok(yes)
}

fn choose_openrouter(cfg: &mut Config, theme: &ColorfulTheme) -> Result<()> {
    tui::info(
        "OPENROUTER",
        "Get a key at https://openrouter.ai/keys — one key, every hosted \
         model, nothing to install. Perfect for a phone: it only makes \
         HTTPS calls.",
    );
    let key = loop {
        let k: String = Password::new()
            .with_prompt("OpenRouter API key (sk-or-v1-…)")
            .interact()?;
        if k.is_empty() {
            tui::warn("OPENROUTER", "Key cannot be empty — or switch to Ollama.");
        } else {
            break k;
        }
    };
    cfg.openrouter_api_key = key;

    let mut display: Vec<String> = OPENROUTER_DEFAULT_MODELS
        .iter()
        .map(|s| s.to_string())
        .collect();
    let custom = "… type another model id";
    display.push(custom.to_string());
    let idx = Select::with_theme(theme)
        .with_prompt(
            "Default model (cheap+fast for routine passes, heavyweight for hard reasoning)",
        )
        .items(&display)
        .default(0)
        .interact()?;
    cfg.openrouter_model = if display[idx].eq(&custom.to_string()) {
        let m: String = Input::with_theme(theme)
            .with_prompt("Model id (e.g. ~z-ai/glm-flash-latest)")
            .interact_text()?;
        m.trim().to_string()
    } else {
        OPENROUTER_DEFAULT_MODELS[idx].to_string()
    };

    // Live probe — catches bad keys before the first run.
    match Client::from_config(
        crate::config::Provider::OpenRouter,
        &cfg.openrouter_api_key,
        &cfg.openrouter_model,
        "",
        "",
    )
    .and_then(|c| c.probe())
    {
        Ok(()) => tui::info("OPENROUTER", "Key verified — backend reachable."),
        Err(e) => tui::warn(
            "OPENROUTER",
            &format!("Probe failed: {e}\nA wrong key can be fixed later with /setup."),
        ),
    }
    Ok(())
}

fn choose_ollama(cfg: &mut Config, theme: &ColorfulTheme, is_termux: bool) -> Result<()> {
    if is_termux {
        tui::info(
            "OLLAMA ON TERMUX",
            "Phones need really slim models. Good ones: \
             qwen2.5:0.5b-instruct (~500 MB), llama3.2:1b (~1.3 GB), \
             smollm2:360m (~300 MB).",
        );
        if Confirm::with_theme(theme)
            .with_prompt("Install Ollama in Termux now (tur-repo + ollama)?")
            .default(false)
            .interact()?
        {
            println!(
                "{}",
                console::style("running: pkg install -y tur-repo && pkg install -y ollama").dim()
            );
            let r1 = crate::tools::run_command("pkg install -y tur-repo");
            println!("{r1}");
            let r2 = crate::tools::run_command("pkg install -y ollama");
            println!("{r2}");
            if Confirm::with_theme(theme)
                .with_prompt("Pull the default slim model now (qwen2.5:0.5b-instruct, ~500 MB)?")
                .default(true)
                .interact()?
            {
                let r3 = crate::tools::run_command("ollama pull qwen2.5:0.5b-instruct");
                println!("{r3}");
            }
            tui::info(
                "OLLAMA",
                "Start the daemon when you need it (second Termux session): \
                 ollama serve — then the agent reaches it at \
                 http://127.0.0.1:11434.",
            );
        } else {
            tui::info(
                "OLLAMA",
                "Manual install later:\n  pkg install tur-repo\n  pkg install ollama\n  ollama serve &\n  ollama pull qwen2.5:0.5b-instruct",
            );
        }
    }

    cfg.ollama_url = Input::with_theme(theme)
        .with_prompt("Ollama base URL")
        .default(cfg.ollama_url.clone())
        .interact_text()?;

    let mut model_options: Vec<String> = match Client::ollama_tags(&cfg.ollama_url) {
        Ok(tags) if !tags.is_empty() => {
            tui::info("OLLAMA", "Daemon reachable — these models are installed:");
            tags.iter().take(10).cloned().collect()
        }
        _ => {
            tui::warn(
                "OLLAMA",
                &format!(
                    "No installed models at {} — pulling from the slim phone list is a good start.",
                    cfg.ollama_url
                ),
            );
            OLLAMA_SLIM_MODELS.iter().map(|m| m.to_string()).collect()
        }
    };
    let custom = "… type another model id";
    model_options.push(custom.to_string());
    let idx = Select::with_theme(theme)
        .with_prompt("Local model")
        .items(&model_options)
        .default(0)
        .interact()?;
    cfg.ollama_model = if model_options[idx].eq(&custom.to_string()) {
        let m: String = Input::with_theme(theme)
            .with_prompt("Model (e.g. qwen2.5:0.5b-instruct)")
            .interact_text()?;
        m.trim().to_string()
    } else {
        model_options[idx].clone()
    };
    Ok(())
}
