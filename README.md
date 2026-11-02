# DeCypherTek.ai

An AI that runs your systems — installed with one command, sealed in an encrypted vault, and it remembers.

*Decoding technology, so you don't have to.*

## Install & First Run

Install: Works on Linux (PC, server, ARM box) and on Android inside the Podroid Linux VM.

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/install.sh | bash
```

Launch: after the install, run this from any terminal to wake the agent.

```bash
decyphertek.ai
```

The first launch walks you through setup: pick the **brain** (an OpenRouter API key for hosted AI, or a local Ollama for no-cloud), decide if it stays **leashed** (asks permission before it acts) or goes **unleashed**, and set the **vault password** that encrypts the agent's memory. Every launch after that asks for that password and drops you at the `decyphertek.ai:~$` prompt. Re-run the install command any time to update.

From that prompt, your terminal stays a terminal — anything without a `/` runs in your shell as typed. The slash-command basics:

| Command | What it does |
| --- | --- |
| `/chat <task>` | Talk to it, with its full memory behind the answer |
| `/code <task>` | Hands-on work — ends in a report of exactly what changed |
| `/research <topic>` | Web research — ends in a written report with sources |
| `/upload` | Pick files from Downloads — it reads and remembers them |
| `/ingest <folder>` | Teach it every document in a folder |
| `/store` | Browse and add tool servers to the agent |
| `/wiki list` / `/wiki read <name>` | Read the agent's own knowledge pages |
| `/status` | Current brain, permissions, memory size |
| `/setup` | Re-open the setup wizard (any setting, anytime) |
| `/password` | Change the vault password |
| `/help` | The full command list |
| `exit` (or Ctrl-D) | Seal the vault and quit |

While a task runs the screen shows one line; when it finishes you get one report — what was asked, what was done, what changed — and the prompt returns.

## Uninstall

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/uninstall.sh | bash
```

Removes the program; the encrypted memory vault is kept. Wipe the memory too with `--purge`:

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/uninstall.sh | bash -s -- --purge
```

## About — how it works

**What it is.** An AI operator that runs on your own machine — a PC, a server, or the Podroid Linux VM on Android. One program, no required cloud: it calls a brain you choose (hosted OpenRouter, or a local Ollama for a fully offline setup), and everything it knows — settings, documents, skills, every past run — stays inside a single encrypted vault on your device, sealed with your password.

**How it behaves.** It only does what you granted while leashed: folder access you approve, shell commands that ask y/N, web and tools you enable. Every past conversation, uploaded document, and skill it distills persists across runs, so it stays current on your systems instead of forgetting. Each run works like an operator's case: it acts, logs everything, reports the result once.
