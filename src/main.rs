//! DeCypherTek.ai — one Rust binary, runs anywhere: Termux on Android first,
//! every other Linux (or macOS) from the same release pipeline.
//!
//! Lifecycle: launch → password → vault unseals into staging/ → the @-shell
//! runs → exit seals everything back into the encrypted vault.

mod agent;
mod chatlog;
mod config;
mod models;
mod paths;
mod setup;
mod shell;
mod store;
mod tools;
mod tui;
mod util;
mod vault;
mod vector;
mod wiki;

use anyhow::{Context, Result};
use paths::Paths;
use std::io::IsTerminal;

const HELP: &str = "\
DeCypherTek.ai — a customizable, self-learning AI agent in one Rust binary.

USAGE:
  decyphertek.ai         launch the @-shell (asks vault password, decrypts memory)
  decyphertek.ai setup   run the first-time walkthrough wizard (brain, grants)
  decyphertek.ai --help  this help
  decyphertek.ai --version  print version

Everything lives encrypted in ~/.decyphertek.ai/vault.dct; it is decrypted
for the session on launch and sealed again on exit. Inside the shell, type
@help for agent commands — anything else runs in your shell as typed.";

fn main() {
    if let Err(e) = try_main() {
        tui::error(&format!("{e:#}"));
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help") | Some("-h") | Some("help") => {
            println!("{HELP}");
            Ok(())
        }
        Some("--version") | Some("-V") => {
            println!("decyphertek.ai {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("setup") | Some("wizard") => cmd_setup(),
        _ => cmd_run(),
    }
}

/// `decyphertek.ai setup`: wizard first, then optionally launch the shell.
fn cmd_setup() -> Result<()> {
    let paths = Paths::from_home().context("locate home directory")?;
    std::fs::create_dir_all(&paths.root)
        .with_context(|| format!("create {}", paths.root.display()))?;

    // Existing agent: unlock it first, then re-run the wizard over it.
    let mut key: Option<[u8; 32]> = None;
    let mut salt: Option<[u8; 16]> = None;
    if paths.vault_file.exists() {
        let (k, s) = unlock(&paths)?;
        key = Some(k);
        salt = Some(s);
        vault::unseal(&paths, &k)?;
    }

    if !std::io::stdin().is_terminal() {
        return Err(anyhow::anyhow!(
            "the setup wizard is interactive — run it in a real terminal"
        ));
    }

    let existing = if paths.config_file.exists() {
        Some(config::Config::load(&paths)?)
    } else {
        None
    };
    setup::wizard(&paths, existing.as_ref())?;

    // Fresh install: nothing sealed yet — pick a password and seal now.
    if key.is_none() {
        let password = setup::prompt_new_password()?;
        let s = new_salt()?;
        let k = vault::derive_key(&password, &s)?;
        std::fs::create_dir_all(&paths.root)
            .with_context(|| format!("create {}", paths.root.display()))?;
        vault::seal(&paths, &k, &s)?;
        tui::info(
            "VAULT",
            "Sealed and encrypted ~/.decyphertek.ai/vault.dct. Launch with: decyphertek.ai",
        );
        return Ok(());
    }

    // Existing install: re-seal with the key we already hold.
    let k = key.unwrap();
    let s = salt.unwrap();
    vault::seal(&paths, &k, &s)?;
    tui::info("VAULT", "Vault re-sealed. Launch with: decyphertek.ai");
    Ok(())
}

/// `decyphertek.ai` — the normal flow: detect setup state, run the walkthrough
/// automatically on a fresh install, and land in the @-shell either way.
fn cmd_run() -> Result<()> {
    let paths = Paths::from_home().context("locate home directory")?;
    std::fs::create_dir_all(&paths.root)
        .with_context(|| format!("create {}", paths.root.display()))?;

    // No agent at all → first-run wizard, then straight into the shell.
    if !paths.vault_file.exists() {
        let stale = paths.staging.exists() && paths.config_file.exists();
        if stale {
            tui::warn(
                "RECOVERY",
                "Found unsealed leftovers from a crashed first session — starting \
                 the walkthrough again (nothing sealed existed yet).",
            );
        }
        if !std::io::stdin().is_terminal() {
            return Err(anyhow::anyhow!(
                "no agent found — run `decyphertek.ai setup` in a real terminal first"
            ));
        }
        let _ = std::fs::remove_dir_all(&paths.staging);
        setup::wizard(&paths, None)?;
        let password = setup::prompt_new_password()?;
        let salt = new_salt()?;
        let key = vault::derive_key(&password, &salt)?;
        vault::seal(&paths, &key, &salt)?;
        tui::info(
            "VAULT",
            "Sealed. From now on, launching decrypts with your password. \
             Launching your @-shell now…",
        );
        shell_session(&paths, key, salt)
    } else {
        // Normal launch: password → unseal → shell → seal.
        let (key, salt) = unlock(&paths)?;
        shell_session(&paths, key, salt)
    }
}

/// Unseal → shell → seal. Shared by fresh setups (auto-continue) and
/// normal launches — one path, one lifecycle.
fn shell_session(paths: &Paths, key: [u8; 32], salt: [u8; 16]) -> Result<()> {
    vault::unseal(paths, &key)?;

    let mut cfg = config::Config::load(paths).context("load config from vault")?;
    let mut vectors = vector::Vectors::open(&paths.vector_db)?;
    let rotated = chatlog::rotate_old_logs(paths)?;
    if rotated.0 > 0 {
        tui::info(
            "ARCHIVE",
            &format!("{} old chat logs rotated into {}", rotated.0, rotated.1),
        );
    }

    let mut key = key;
    shell::run(&mut cfg, paths, &mut vectors, &mut key, salt)?;

    // Seal the vault back up.
    vault::seal(paths, &key, &salt)?;
    tui::info(
        "VAULT",
        "Sealed. Your agent sleeps encrypted at ~/.decyphertek.ai/vault.dct",
    );
    Ok(())
}

fn new_salt() -> Result<[u8; 16]> {
    let mut s = [0u8; 16];
    getrandom::getrandom(&mut s).map_err(|e| anyhow::anyhow!("generate salt: {e}"))?;
    Ok(s)
}

/// Ask for the vault password up to 3 times and verify it against the seal.
fn unlock(paths: &Paths) -> Result<([u8; 32], [u8; 16])> {
    if !std::io::stdin().is_terminal() {
        return Err(anyhow::anyhow!(
            "vault unlock needs an interactive terminal"
        ));
    }
    let salt = vault::vault_salt(paths)?;
    let mut tries = 0;
    loop {
        tries += 1;
        let password = setup::prompt_unlock()?;
        let k = vault::derive_key(&password, &salt)?;
        match vault::verify_password(paths, &k) {
            Ok(()) => return Ok((k, salt)),
            Err(_) if tries < 3 => {
                tui::warn("VAULT", "Wrong password — try again.");
            }
            Err(e) => return Err(e),
        }
    }
}
