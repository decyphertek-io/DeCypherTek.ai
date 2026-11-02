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
| `/chat <task>` | Talk to it, with its full memory behind the answer — ask about anything you `/rag` or it has read |
| `/code <task>` | Hands-on work — ends in a report of exactly what changed |
| `/research <topic>` | Web research — ends in a written report with sources |
| `/rag <folder> *` | Index every document in the folder — PDF, Word (.docx/.doc), EPUB, text, code — and grant read |
| `/rag <folder> <file>` | Index one document by name |
| `/store` | Browse and add tool servers to the agent |
| `/status` | Current brain, permissions, memory size |
| `/setup` | Re-open the setup wizard (any setting, anytime) |
| `/password` | Change the vault password |
| `/help` | The full command list |
| `exit` (or Ctrl-D) | Seal the vault and quit |

`/rag` is differential: re-run it any time — unchanged documents cost nothing, changed ones update in place, deleted ones drop out of memory. Re-running the install command also updates the agent.

While a task runs the screen shows one line; when it finishes you get one report — what was asked, what was done, what changed — and the prompt returns.

## Uninstall

One command removes everything — the program, the encrypted memory vault, every trace in your home folder:

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/uninstall.sh | bash
```

## About — what it can do

**The core.** An AI operator that runs on your own machine — a PC, a server, or the Podroid Linux VM on Android — as one self-contained program. It calls a brain you choose (hosted OpenRouter, or a local Ollama for a fully offline setup), and everything it knows stays inside one encrypted vault on your device, sealed with your password.

**Its abilities, high level:**

- **RAG chat** — conversations are backed by a private vector database: everything you `/rag` (folders or single documents — PDF, Word, EPUB, text, code) and everything the agent learns (past runs, distilled skills) is chunked and indexed so the relevant knowledge is found instantly instead of re-reading everything. Answers stay fast — and it never forgets between runs.
- **LLM wiki** — durable knowledge goes into a markdown knowledge base the agent writes and maintains itself: a `memory-index` directory page maps where everything lives, and new pages are auto-indexed into its memory the moment they are written.
- **Web research** — searches the web and delivers a written report with sources; research can be locked to specific site lists you define.
- **Hands-on system work** — reads and writes files, runs shell commands, and verifies its own work, reporting exactly what changed.
- **MCP support** — a whole catalog of extra tool servers (fetch, git, GitHub, Slack, …) can be pulled from its `/store` and run network-isolated; their tools merge into the agent's toolbox.
- **Subagents** — for complex tasks it writes its own subagents: focused worker copies with their own instructions, working the task in parallel under the same leash.

**How it behaves.** While leashed it only does what you granted — folder access you approve, shell commands that ask y/N, tools you enable. Every run is logged in full and reports once: what was asked, what was done, what changed.
