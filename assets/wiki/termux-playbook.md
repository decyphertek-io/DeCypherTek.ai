# Termux Playbook

DeCypherTek runs natively in Termux on Android — a single static musl
binary, no root, no Docker, no interpreter.

## Install (one command, same as anywhere)

    curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/install.sh | bash

The script auto-detects aarch64/armv7, downloads the latest release binary,
installs light prerequisites, and drops you into the TUI walkthrough.

## Why it works on a phone

- One static binary (aarch64-unknown-linux-musl): no runtime to install.
- Memory is two files in the vault: SQLite + markdown. No external DB.
- The brain is OpenRouter by default — the phone only makes HTTPS calls.
- Tools run as local processes; no containers are required.

## Optional: local models with Ollama

Ollama ships in the Termux user repository (tur-repo):

    pkg install tur-repo
    pkg install ollama
    ollama serve &                # in a second session
    ollama pull qwen2.5:0.5b-instruct

Phone-sized models that actually work:

- qwen2.5:0.5b-instruct (~500 MB) — default pick, surprisingly capable
- llama3.2:1b (~1.3 GB) — best quality if the phone has RAM to spare
- smollm2:360m (~300 MB) — for very tight devices
- nomic-embed-text — embedding model for the (roadmap) neural embedder

Realistic expectations: these handle short tasks and chat, not deep
reasoning. OpenRouter stays the default brain for heavy thinking.

## Battery and storage

- The agent only runs while you have the shell open — nothing background.
- Chat logs rotate into tar.gz archives monthly to keep storage lean.
- Everything is inside the encrypted vault; sync the whole directory and
  you have moved the whole agent.

## Recovery

If a session crashes, `staging/` may survive on disk unsealed — the next
launch re-verifies the password and seals it back, nothing is lost.
