# Termux Playbook

DeCypherTek runs on Android with a proot Debian Linux home — a single static
musl binary, no root, with Docker (and MCP tool servers from `@store`)
inside the proot container.

## Install (one command, same as anywhere)

    curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/install.sh | bash

The script auto-detects aarch64/armv7 and on Termux bootstraps a proot
Debian Linux home — nothing of the agent is downloaded into Termux itself.
Everything happens inside the Debian container, in order:

- the proot Debian rootfs is installed by `proot-distro` first;
- inside it: `apt update`, then
  `apt install -y docker.io docker-compose curl gnupg ca-certificates`
  (best-effort; some kernels refuse the daemon);
- LAST, inside the container: the latest release binary is downloaded and
  installed at `/usr/local/bin/decyphertek.ai`;
- a sourced alias lands in `~/.bashrc` — typing `decyphertek.ai` from a
  Termux prompt enters the proot Debian and starts the agent there, with
  dockerd started automatically using `--iptables=false --bridge=none`
  (proot-safe mode);
- the installer ends by running `decyphertek.ai` itself, so a fresh
  install drops straight into the first-run walkthrough.
- The vault stays in real Termux home (`~/.decyphertek.ai`) and is
  bind-mounted in, so it survives container reinstalls; `/sdcard` is
  mounted at `/sdcard` when present.

## Why it works on a phone

- One static binary (aarch64-unknown-linux-musl): no runtime to install.
- Memory is two files in the vault: SQLite + markdown. No external DB.
- The brain is OpenRouter by default — the phone only makes HTTPS calls.
- MCP tool servers from `@store` run in hardened Docker containers inside
  the proot Debian: `--network=none`, all capabilities dropped — they can
  only ever answer the agent over stdio.
- If this device's kernel refuses dockerd (proot limits), the agent runs
  everything else natively; point `DOCKER_HOST` at a LAN machine for
  @store servers, or re-run on a device where the daemon comes up.

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
