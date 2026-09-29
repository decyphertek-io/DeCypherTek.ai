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

    // 3. The Leash.
    let leash_opts = vec![
        "Leashed — default. Reads only granted folders, writes only its own data dir. Tool grants below still apply.",
        "Unleashed — folder scopes off: it may read/write anywhere your user can. Tool grants still apply.",
    ];
    let leash_idx = Select::with_theme(&theme)
        .with_prompt("The Leash — keep it on or take it off?")
        .items(&leash_opts)
        .default(0)
        .interact()?;
    cfg.leash = if leash_idx == 0 {
        "leashed".into()
    } else {
        "unleashed".into()
    };

    // 4. Tool grants.
    let tool_items = vec![
        "web_search — research the web (keyless DuckDuckGo + Wikipedia)",
        "read_files — read files in granted folders",
        "write_files — write files in granted folders",
        "run_command — execute shell commands (leashed asks before each; unleashed just runs)",
        "mcp_servers — run MCP tool servers from @store (docker; hardened, internal-only)",
    ];
    let defaults = vec![
        cfg.tool_web_search,
        cfg.tool_read_files,
        cfg.tool_write_files,
        cfg.tool_run_command,
        cfg.tool_mcp,
    ];
    let chosen = MultiSelect::with_theme(&theme)
        .with_prompt("Grant tools (space to toggle)")
        .items(&tool_items)
        .defaults(&defaults)
        .interact()?;
    cfg.tool_web_search = chosen.contains(&0);
    cfg.tool_read_files = chosen.contains(&1);
    cfg.tool_write_files = chosen.contains(&2);
    cfg.tool_run_command = chosen.contains(&3);
    cfg.tool_mcp = chosen.contains(&4);

    // 5. Persist: config, baseline wiki, RAG ingest.
    paths.ensure_staging()?;
    crate::wiki::write_baseline(paths)?;
    cfg.save(paths)?;

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
            "No folders given — use @ingest <folder> anytime to teach it new docs.",
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
            .with_prompt("Model id (e.g. z-ai/glm-latest)")
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
            &format!("Probe failed: {e}\nA wrong key can be fixed later with @setup."),
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
