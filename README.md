# DeCypherTek.ai

> **Status: Research & Development — Phase 1.**
> This is just the working architecture document — nothing is built yet, and everything here is subject to change.

A customizable, self-learning AI agent. It is taught to read the docs before it uses a tool, remember what it learns, search the web to do research, run its tools over MCP, and ship as one Rust executable that runs anywhere — including Termux on Android.

*Decoding technology, so you don't have to.*

## Overview

DeCypherTek.ai is a customizable agent framework that can be used for anything — the behavior, knowledge, and tools are yours to define. The agent has access to the technical knowledge and docs you provide, can search the web to do research, and carries a real memory: Wiki Memory together with a Vector DB lets the LLM learn, remember, and hold custom knowledge, so it gets smarter with every run.

The whole thing is one native Rust executable per platform — one codebase cross-compiled into two release folders: `pc/` for standard computers, `android-arm/` for Termux on Android. The architecture is deliberately the simplest one that works:

- Agent behavior is defined in customizable `AGENT.md` files.
- Memory lives in a local SQLite vector store plus a markdown Wiki Memory — no external database to stand up, which is exactly what lets a phone run it.
- Knowledge is custom: you provide the docs and technical knowledge it works from.
- Tools speak MCP — in Docker containers on desktop and server, as local processes under Termux on Android.
- Web search is built in for research.
- The terminal stays a normal terminal — commands pass through untouched, and only `@chat`, `@code`, or `@research` wake the agent, which reports back once, in a clean TUI. You never watch it think.
- Its brain is pluggable but stays simple: OpenRouter by default — one lightweight API key for every hosted model — or your own Ollama if you want local models. Nothing else is supported, on purpose.
- It can sync itself through sync MCP servers — Wiki Memory as a git repo on GitHub, the data directory backed up to Proton Drive via rclone.
- One Rust binary per release folder — `android-arm/` and `pc/` — which also keeps self-replication easy, and one command installs it on Termux.

## Core Ideas

- **Customizable.** It can be used for anything. Behavior comes from a customizable `AGENT.md`, knowledge comes from the docs you provide, and tools come from MCP servers — swap any of them and it is a different agent.
- **Models: OpenRouter or Ollama.** OpenRouter is the default — lightweight, nothing to install, one API key for any hosted model, which suits a phone perfectly. If you have Ollama, the agent can run your local models instead. Those two and no others, on purpose: both speak OpenAI-style HTTP, so one thin client covers the whole model layer.
- **Memory.** Wiki Memory and a Vector DB give the LLM a real memory — it can learn, retain what it learns, and carry custom knowledge across runs.
- **Custom knowledge.** The agent has access to the technical knowledge and docs you provide, chunked into memory, so it works on your stack and in your domain.
- **SQLite vector store.** A vector is just a DB — it chunks knowledge so docs can be searched quickly. Simple to build, no external database service required.
- **Hardcoded Wiki Memory.** Wiki Memory is markdown, so it is technically docs: curated baseline knowledge the agent ships with out of the box, and a place to write what it learns.
- **Teaching the LLM logic.** The agent is instructed: read the docs (man pages first) before using a tool, then chunk them into the vector store. When a tool call teaches it something new, chunk that again into the vector store and/or update Wiki Memory.
- **Web research.** When memory and local docs are not enough, the agent can search the web to do research — and chunk what it finds into memory.
- **Silent running.** You never watch it think. Reasoning, tool calls, retries, dead ends — none of it scrolls past the terminal; it all streams into chat logs while the screen stays quiet until the final TUI report.
- **Forensic chat logs.** Every run leaves a case file: every prompt, every thought, every tool invocation with the diffs it made, recorded so the AI can be studied like an investigation. Chat logs are knowledge too — they get chunked into the vector store, and after a month they rotate into a tar.gz archive.
- **Sync.** Since the agent is one binary plus one data directory, syncing itself is just moving that directory: a GitHub MCP server keeps Wiki Memory as a versioned git repo, and an rclone MCP server backs the data directory up to Proton Drive.
- **MCP tools.** Tools are MCP (Model Context Protocol) servers — Docker containers on desktop and server, local processes under Termux on Android.
- **One Rust executable.** Everything — the agent, its memory engine, the bundled baseline docs — compiles into a single native binary: no interpreter, no runtime, no framework. That is also what makes self-replication easy.
- **No LangChain — the loop is plain code.** The docs-first loop is deliberately small: prompt the model, call the tool, chunk the result, update the wiki. In Python that is a framework's job; in Rust it is a few hundred owned lines.
- **Termux / Android.** A first-class target: the exact same binary runs in Termux on a phone, with a one-command install script that sets everything up.

## Architecture

```mermaid
flowchart TB
    subgraph Core["Agent Core - single Rust executable"]
        Orchestrator["LLM Orchestrator"]
        AgentDef["AGENT.md - customizable agent definition"]
        Orchestrator --- AgentDef
    end

    subgraph Memory["Memory subsystem - local, offline-first"]
        Vector[("SQLite Vector Store<br/>chunked knowledge for quick search")]
        Wiki[("Wiki Memory<br/>markdown knowledge base")]
    end

    subgraph Models["Model backends - one OpenAI-style client"]
        OpenRouter["OpenRouter<br/>default - hosted models, one API key"]
        Ollama["Ollama<br/>optional - local models"]
    end

    subgraph Tools["Tool layer - MCP"]
        Docker["Docker MCP containers<br/>(desktop and server)"]
        Native["MCP processes in Termux<br/>(no Docker on Android)"]
        Web["Web search for research"]
    end

    subgraph Sync["Sync - MCP"]
        Ghsync["GitHub MCP server<br/>Wiki Memory as a git repo"]
        Rclone["rclone MCP server<br/>Proton Drive backup"]
    end

    subgraph Interface["Interface - a normal shell until you @ it"]
        Pass["Command passthrough<br/>typed commands run unchanged"]
        AtCmds["@chat @code @research<br/>activate a mode"]
        Tui["TUI<br/>the final report, only"]
    end

    subgraph Forensics["Forensic chat logs"]
        Chats[("Chat logs - every prompt,<br/>thought, tool call, diff")]
        Tar["Monthly tar.gz archive"]
    end

    You([You]) -->|@ command, mode dialogue| AtCmds
    You -->|anything else runs as typed| Pass
    AtCmds -->|task| Orchestrator
    Orchestrator -->|final report only| Tui
    Orchestrator -->|thoughts, tool calls, diffs| Chats
    Chats -->|chunked into memory| Memory
    Chats -->|monthly rotation| Tar
    You -->|your docs and knowledge| Memory
    Memory -->|retrieved context| Orchestrator
    Orchestrator -->|chunk learnings and update wiki| Memory
    Orchestrator -->|prompts| Models
    Models -->|completions| Orchestrator
    Orchestrator -->|tool calls| Tools
    Tools -->|results, research, learnings| Orchestrator
    Orchestrator -->|backup and sync| Sync
    Sync -->|restore memory on any device| Memory
    Core ==>|same self-replicating binary| Deploy["Runs everywhere:<br/>desktop, server, and Termux on Android"]
```

### Components

| Component | Role |
| --- | --- |
| **LLM Orchestrator** | Planning, reasoning, and driving the loop below. |
| **Model backend** | OpenRouter by default (any hosted model, one API key) or your own Ollama for local models — chosen per agent in `AGENT.md`. |
| **`AGENT.md`** | Customizable per-agent definition; behavior without code. |
| **SQLite Vector Store** | Chunked knowledge for fast semantic search. |
| **Wiki Memory** | Markdown knowledge base; hardcoded baseline plus everything the agent writes back. |
| **Your docs** | Technical knowledge and docs you provide, chunked into memory. |
| **Web search** | Online research; findings get chunked into memory. |
| **The `@`-shell** | Passthrough terminal — commands run exactly as typed; `@chat`, `@code`, `@research` activate agent modes. |
| **TUI report** | The agent's only visible output — the final report, from findings to diffs. |
| **Forensic chat logs** | Full run trail: prompts, thoughts, tool calls, diffs — chunked into memory, rotated into tar.gz monthly. |
| **MCP tool servers** | Tools over the Model Context Protocol — Docker containers on desktop and server, local processes under Termux. |
| **Sync MCP servers** | Backup and restore of the data directory — GitHub for versioned Wiki Memory, Proton Drive via rclone. |
| **Rust binary** | One native executable per release folder — `android-arm/` for Termux, `pc/` for standard computers; the self-replication vehicle. |

### How It Learns

1. **Before a tool call** — the agent reads the docs for what it is about to use (man pages first) and chunks them into the vector store.
2. **Research** — when memory and local docs don't hold the answer, it searches the web and chunks the findings into memory.
3. **Call the tool** — execution happens through its MCP tool server: containerized on desktop and server, a local process in Termux.
4. **After the tool call** — chunk what was learned into the vector store and/or update Wiki Memory.
5. **Next run starts smarter** — knowledge is recalled from memory instead of relearned from scratch.

### How It Syncs

Sync is just MCP tool calls that move the agent's one data directory, so the same mechanism works for backup and for replication:

- **GitHub (GitHub MCP server).** Wiki Memory *is* a git repo — every commit is versioned history of what the agent has ever learned. A new device clones it; the binary comes from GitHub Releases; the agent that arrives is the same one that left.
- **Proton Drive (rclone MCP server).** Proton Drive has no public API to build on, but rclone speaks it — an MCP server wrapping rclone syncs the whole data directory (Wiki Memory + vector store) as a backup.
- **Anywhere, container or not.** Both sync providers are MCP tool servers like any other: Docker containers on desktop and server, plain `git` and `rclone` under Termux — it works equally well on a phone.

### Models

Two backends, deliberately — anything more would break the simple rule:

- **OpenRouter is the default.** One API key, every hosted model, nothing to install or run. A phone cannot run a frontier model, but it can always ask one — which keeps the Termux build genuinely lightweight: an HTTPS call is all the phone has to make. Pick the model per agent in `AGENT.md` — something cheap and fast for routine passes, something heavyweight for hard reasoning.
- **Ollama is optional.** Install Ollama and the agent runs your own local models instead — fully local, no API key, no per-token cost. It sits on your desktop or server; a phone in Termux can reach an Ollama on your LAN when you want zero-cloud runs.
- **Why only these two.** OpenRouter and Ollama both expose OpenAI-style HTTP, so the orchestrator carries exactly one thin model client: swap the base URL and key and the same binary runs cloud or local. That is the entire model layer — kept small on purpose.

### The Stack

Rust end to end — one codebase, two release folders:

```
releases/
├── android-arm/    # aarch64 - runs in Termux on Android
└── pc/             # x86_64 - standard PCs and servers
```

- **No Python, no LangChain.** The docs-first learning loop above is deliberately small — prompt the model, run the MCP tool, chunk the result, update the wiki. A framework would outweigh the app; in Rust it is just code in one binary.
- **Crates, not ecosystems.** One HTTP client for the model backends and web search, one SQLite binding for the vector store, one markdown writer for Wiki Memory — each a small Rust crate, nothing dragging an ML stack along behind it.
- **Cross-compiled, not re-ported.** Both folders come from the identical codebase: the aarch64 Android target builds `android-arm/`, the regular build `pc/`. No maintained divergence, no separate fork for Termux.

## The `@`-Shell

The terminal stays a normal terminal. Everything you type passes straight through to the system and runs exactly as you typed it — only `@` commands trigger the AI:

- **`@chat`** — chat mode. The dialogue that follows is a conversation backed by the full memory.
- **`@code`** — code mode. The agent works the task and ends with a report of what changed: diffs, commands run, final state.
- **`@research`** — research mode. Web search plus memory, ending in a written report with sources.

The mode you pick frames everything that follows — and the interaction has strict rules:

- **You never watch it think.** Reasoning, tool calls, retries, dead ends — none of it hits the terminal while the task runs. It all streams into chat logs; the screen stays quiet.
- **One final report, in the TUI.** When the run finishes, the agent renders a proper TUI report — what was asked, what happened, which tools ran, what changed, conclusions — then hands the prompt back to the shell. Normal terminal operation before and after.
- **Forensic-grade logging — study it like a case file.** Every run leaves a complete trail: every prompt, every thought, every tool invocation with its arguments, every file touched. Digital-forensics level logging for the AI, so you can replay how it thought and what it ran whenever you want to audit, debug, or learn from it.
- **Logs are memory, then archives.** Chat logs are knowledge like any other: they get chunked into the vector store, so the agent remembers its past dialogues and learns from them. After a month on disk, a run's logs rotate into a tar.gz archive — out of the way, still restorable whenever a case needs reopening.

## Termux on Android

The phone is a first-class platform, not an afterthought. Everything is just one Rust executable from the `android-arm/` release folder, and it runs in Termux:

- **One binary.** The agent, its memory engine, and the bundled baseline docs are a single Rust binary built for aarch64 — no interpreter, no runtime, no Python toolchain, no pip. The install script below fetches it plus the few helpers Termux needs.
- **Local-first memory.** No external database or hosted service is required — the SQLite vector store and Wiki Memory are just files in the agent's data directory on the phone. That is the whole reason the memory design stays this simple: it has to run on a phone.
- **Tools over MCP.** Docker does not run natively on Android, so MCP tool servers run as local processes inside Termux — or in a proot-based Docker install where the device kernel allows it, or against your desktop's Docker over SSH. The same servers run containerized on desktop and server.
- **A phone-sized brain.** The phone never runs the model — OpenRouter is the default backend, so the phone just makes lightweight API calls to whatever hosted model you pick. Install Ollama on your desktop and the same binary goes fully local instead.
- **Research in your pocket.** Web search gives the phone everything it does not already carry; the findings get chunked into memory for next time.
- **Report, not scroll.** A phone screen sees even less of the process than a desktop: the run stays silent, one final TUI report renders in Termux, and the month's logs tar.gz away to keep phone storage lean.
- **Replication is a copy plus a sync.** Pull the binary from Releases, restore the memory from GitHub or Proton Drive — brand-new device, same learned agent.

### Install on Termux

One command:

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/install-termux.sh | bash
```

The script:

1. Updates Termux and installs the light prerequisites — `git`, `rclone`, `openssh`, and core tools.
2. Sets up Docker as well as Termux allows — a proot-based Docker install when the device kernel supports it, otherwise it points `DOCKER_HOST` at your desktop/server over SSH; if neither applies, it skips containers and MCP tool servers simply run natively.
3. Downloads the Rust agent binary from the `android-arm/` release folder and puts it on your `PATH`.
4. Optional but recommended — clones your Wiki Memory repo and configures Proton Drive in rclone, so the very first run starts with the agent's memory intact.

Everything the script does, the agent can also do for itself — the script just compresses the first five minutes into one command.

## Roadmap

- [ ] Build the SQLite vector store and hardcoded Wiki Memory (~1 week of effort)
- [ ] Implement the docs-first learning loop in plain Rust — no LangChain
- [ ] `@`-shell passthrough — normal commands run as typed; only `@chat`, `@code`, `@research` trigger the agent
- [ ] TUI final reports — the agent's only on-screen output
- [ ] Forensic chat logs — structured run logs, chunked into memory, monthly tar.gz rotation
- [ ] Add web search for research
- [ ] Integrate MCP tool servers — Docker on desktop and server, local processes under Termux
- [ ] Model layer — OpenRouter by default, optional Ollama for local models, one OpenAI-style client
- [ ] MCP sync servers — GitHub for versioned Wiki Memory, rclone for Proton Drive
- [ ] Support customizable agents via `AGENT.md`
- [ ] Compile the single Rust binary for both targets and publish two release folders — `android-arm/` and `pc/`
- [ ] Ship the Termux install script — prerequisites, best-effort Docker, binary from Releases
- [ ] Prove it end to end in Termux on Android: one-command install, learn a tool, sync, self-replicate
