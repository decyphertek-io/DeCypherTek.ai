# DeCypherTek.ai

An AI that runs your systems — one command to install, entirely on your machine, and it remembers.

*Decoding technology, so you don't have to.*

## Install

One command. Works on Linux (PCs, servers, ARM boxes) and on Android inside the Podroid Linux VM:

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/install.sh | bash
```

Re-run the same command any time to update.

## Setup

Follow the setup instructions on first launch: choose a **brain** (an OpenRouter API key for hosted AI, or a local Ollama for a no-cloud setup), pick whether it stays **leashed** (asks permission before it acts), and set the **vault password** that encrypts the agent's memory — everything else just defaults. You land at a `decyphertek.ai:~$` prompt. Type `/help` for the command list; `/setup` re-opens the wizard anytime.

## Uninstall

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/uninstall.sh | bash
```

Removes the program; the encrypted memory vault is kept. Wipe the memory too with `--purge`:

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/uninstall.sh | bash -s -- --purge
```

## About — how it works

**What it is.** DeCypherTek.ai is an AI staff member that lives on your own machine — a phone running the Podroid VM, a PC, or any Linux box. It is one small program with no cloud of its own. You give it a brain of your choosing, and everything it knows and learns stays in a single private vault on your device, sealed with your password.

**How you use it.** Your terminal keeps working exactly as a terminal — everything you type runs as typed. Slash commands wake the AI: `/chat` to talk with full memory behind it, `/code` for hands-on work that ends in a report of exactly what changed, `/research` for a written report with sources, `/upload` or `/ingest` to teach it your documents. You never watch it think: the screen shows one line while it works, then it reports once, cleanly.

**How it remembers.** Every skill, document, and conversation becomes part of the agent's permanent memory: knowledge goes into its own private wiki, and all of it is searchable instantly — the agent looks things up in a filing cabinet instead of rereading everything — so answers stay quick, and it gets more useful about your systems over time instead of forgetting between runs.

**How it stays safe.** It is leashed by default: it only reads, writes, and runs what you grant, asks before running commands, and keeps a full activity log of every run. Nothing about you or your files leaves your machine unless you connect a hosted brain — and even then, only what your task needs. One password encrypts the entire vault — memory, settings, keys — with AES-256.
