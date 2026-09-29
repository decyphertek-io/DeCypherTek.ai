# DeCypherTek.ai

> **Status: v0.1.0 — the first runnable build ships in this repo.**
> One Rust binary. Termux on Android first — if it works there, it works on any Linux (or macOS) with an architecture build for it.

A customizable, self-learning AI agent. It runs as one native Rust executable, wakes from a normal shell only when you `@` it, remembers what it learns in an encrypted local memory, does research on the web, and reports once, cleanly, in a TUI. Everything it is — config, API keys, memory, forensic logs — lives encrypted in `~/.decyphertek.ai/` and is sealed with a password only you hold.

*Decoding technology, so you don't have to.*

## Quick Start

**Termux / Android** (from the Termux terminal):

```bash
curl -fsSL https://github.com/decyphertek-io/DeCypherTek.ai/raw/main/scripts/install.sh | bash
```

**Linux / macOS** — the exact same command. The installer:

1. **On Termux/Android**: Docker can't run in Termux proper (no root), and *nothing of the agent is downloaded into Termux itself*. The installer bootstraps a **proot Debian Linux** home first, and does everything *inside that Linux container, in order*: `apt update`, then `apt install -y docker.io docker-compose curl gnupg ca-certificates`, and **last** the **latest release binary** from GitHub Releases (verifying its SHA-256) is downloaded and installed as `/usr/local/bin/decyphertek.ai` inside the container. dockerd starts automatically with `--iptables=false --bridge=none` (proot-safe); kernels that refuse it get a clean warning instead of a broken agent. A **sourced alias lands in `~/.bashrc`**, so typing `decyphertek.ai` from a Termux prompt launches you straight into the proot Debian terminal. Everywhere else: detects your OS and CPU, downloads the latest release binary, and puts `decyphertek.ai` on your `PATH`, plus a **best-effort Docker setup** for `@store`'s MCP containers.
2. **Runs the agent itself at the end of the install** — no separate setup step and no "run this next" hand-off. It detects whether it has been configured (does the vault exist?); if not, the **TUI walkthrough wizard** runs right there (*Welcome to DeCypherTek.ai* → persona → brain → memory folders → the Leash → vault password), and you land in the `@`-shell afterwards. If no release is published yet, it tells you exactly which workflow to run.
3. Re-run the same command any time to **update** — it always pulls `releases/latest`.

```bash
decyphertek.ai
```

It asks for the **vault password**, decrypts `~/.decyphertek.ai/vault.dct` into memory, and drops you at the `@`-shell prompt. `@setup` re-runs the walkthrough any time you want to change the configuration.

## The `@`-Shell

The terminal stays a normal terminal. Everything you type passes straight through to your shell and runs exactly as typed — only `@` commands wake the agent:

| Command | What it does |
| --- | --- |
| `@chat <task>` | conversation backed by full memory |
| `@code <task>` | hands-on: read, change, verify — ends with a diff report |
| `@research <topic>` | memory + web research — ends in a written report with sources |
| `@store` | the MCP store: fuzzy-search TUI over every MCP server on Docker (A-Z) — pull, register, update, disable, uninstall |
| `@ingest <folder>` | chunk a folder's docs into RAG memory (grants read too) |
| `@grants read <path>` / `@grants write <path>` | grant folder access |
| `@leash leashed\|unleashed` | take the leash off (or put it back on) |
| `@status` | brain, leash, grants, RAG size, wiki size |
| `@wiki list` / `@wiki read <name>` | browse Wiki Memory |
| `@setup` | re-run the walkthrough wizard |
| `@password` | change the vault password |
| `@help` | all commands |
| `exit` (or Ctrl-D) | seal the vault and quit |

**You never watch it think.** While a run is live, reasoning, tool calls, retries and dead ends stream only into the forensic chat log; the screen stays quiet until the final TUI report renders: what was asked, what happened, which tools ran, what changed — then the shell prompt returns.

## What's Built (v0.1.0)

- **One Rust binary** — the agent, memory engine, tool suite, TUI walkthrough, crypto vault, and baseline docs all compile into a single static executable. No interpreter, no runtime, no framework.
- **The `@`-shell** — passthrough shell + `@chat` / `@code` / `@research` agent modes with per-mode system framing.
- **Pluggable personas** — `default`, plus **hal9000**, **neuromancer**, and **terminator** personalities chosen in the walkthrough; persona changes how it thinks and talks, never what it's permitted to do.
- **Model layer: OpenRouter or Ollama** — deliberately only two backends, both speaking OpenAI-style HTTP, so one thin client covers the entire model layer. OpenRouter is the default (one key, every hosted model — perfect for a phone: it only makes HTTPS calls). Ollama runs your local models.
- **Encrypted vault** — everything lives at `~/.decyphertek.ai/vault.dct`: AES-256-GCM over a gzip'd tar of the whole data directory, key derived with Argon2id from your password (salted, fresh nonce per seal). Launch asks for the password, decrypts to a private staging dir, and seals atomically on exit — AES-GCM's authentication means a wrong password simply fails. Crash mid-session? The plaintext staging survives, and the next launch re-verifies and re-seals it.
- **RAG vector store** — one SQLite file, chunked knowledge, deterministic 256-dim feature-hashing embeddings + cosine search; zero external services, zero model downloads, fully offline (a phone-sized brain has to run on the phone).
- **Wiki Memory** — markdown knowledge base with a baseline shipped *inside the binary* (operative handbook, Termux playbook, vault internals, commands, personas, RAG design), plus everything the agent writes back.
- **MCP tool servers via `@store`** — a fuzzy-search TUI over *every MCP server found in Docker*, A-Z: an in-binary seed catalog (fetch, git, github, slack, time, …), a live Docker Hub query, and images already pulled on the machine. Pick one, pull, register — its tools merge into the agent's tool list as `mcp_<server>_<tool>`. Servers launch under a **security template that keeps them internal-only**: `--network=none`, `--cap-drop=ALL`, `--security-opt=no-new-privileges`, memory/pid caps, no published ports — the only channel a server gets is the stdio pipe the agent holds, so it can answer the agent and nothing else. Enable/disable/uninstall from the same TUI; the registry seals into the vault.
- **Proot Debian Linux home on Termux** — Docker can't run in Termux proper, so the installer bootstraps a `proot-distro` Debian rootfs and does everything inside it, in order: `apt update`, `apt install -y docker.io docker-compose curl gnupg ca-certificates`, and the DeCypherTek.ai binary downloaded *last* into `/usr/local/bin/decyphertek.ai` — nothing is staged in Termux itself. Typing `decyphertek.ai` from Termux runs a **sourced alias (installed into `~/.bashrc`)** that launches you into the proot Debian terminal (a regular passthrough terminal — everything typed runs as typed, only `@` commands wake the agent). The vault stays bind-mounted from real Termux home so container reinstalls never touch your agent. dockerd auto-starts in proot-safe mode (`--iptables=false --bridge=none`) where the kernel allows it; devices that refuse it get a clear warning and `DOCKER_HOST`-to-LAN guidance.
- **Zero-step setup** — the installer finishes by running `decyphertek.ai` itself; the launcher detects whether the agent has been configured, on a fresh install the walkthrough runs immediately and drops you straight into the `@`-shell afterwards. `decyphertek.ai setup` (or `@setup` inside) re-runs it whenever you want to change the configuration.
- **The leash (permissions)** — the agent reads/writes folders and tools only as granted: read folders, write folders, per-tool switches (web_search, read_files, write_files, run_command, mcp_servers). Leashed, it can always touch its own data dir and nothing else; tool calls beyond grants come back `DENIED` — logged, respected, reported. Unleashed, folder scopes drop. Leashed + run_command enabled asks you to confirm each command interactively.
- **Forensic chat logs** — every run leaves a JSONL case file: prompts, model replies, every tool call with arguments, results, final report. Case files are chunked into RAG memory (so past dialogues are recallable), and rotate into monthly `tar.gz` archives after 30 days.
- **Keyless web research** — DuckDuckGo Instant Answers + Wikipedia search; findings land in the final report and get chunked into memory.
- **Learning loop** — recall memory before a run; instruct the model docs-first (read before using an unfamiliar tool); distill learnings via the `remember` and `write_wiki` tools; every report itself is chunked as `report`-kind knowledge, so next run starts smarter.

## Ollama on Termux — really slim phone models

The walkthrough offers to install Ollama straight from the Termux user repository when it runs on a phone (`pkg install tur-repo && pkg install ollama`), and can pull the default pick for you. The models actually sized for a phone:

| Model | Size | Notes |
| --- | --- | --- |
| `qwen2.5:0.5b-instruct` | ~500 MB | default — surprisingly capable |
| `llama3.2:1b` | ~1.3 GB | best quality if the phone has RAM to spare |
| `smollm2:360m` | ~300 MB | very tight devices |
| `nomic-embed-text` | ~274 MB | roadmap: neural embeddings for RAG |

Start the daemon in a second Termux session (`ollama serve`), and the agent reaches it at `http://127.0.0.1:11434`. Realistic expectations: these handle short tasks and chat, not deep reasoning; OpenRouter stays the default brain for heavy thinking.

## The Secure Vault

```
~/.decyphertek.ai/
├── vault.dct        ← everything, encrypted (AES-256-GCM + Argon2id)
└── staging/         ← exists only while the agent runs (0700, wiped on seal)
    ├── config.json          (persona, backend, API key, leash, grants)
    ├── memory/
    │   ├── vectors.db       (RAG SQLite vector store)
    │   └── wiki/*.md        (Wiki Memory)
    ├── chatlogs/*.jsonl     (forensic case files)
    └── archives/*.tar.gz    (rotated monthly)
```

- The key is never stored — derived on demand: `Argon2id(password, salt, m=32 MiB, t=2)`.
- Seal-on-exit is atomic (write `tmp`, rename); the staging copy is removed afterwards.
- Crash recovery: if the process dies mid-session, `staging/` survives and the next launch checks the password against the sealed vault, then re-seals. Worst case, nothing is lost.
- **There is no backdoor and no recovery.** Lose the password, lose the agent — by design.

## The Leash

| | Leashed (default) | Unleashed |
| --- | --- | --- |
| Read folders | only granted folders + own data dir | anywhere your user can |
| Write | only own data dir (+ grants) | anywhere your user can |
| `run_command` | asks y/N before each command (if enabled) | runs without asking |
| Tool switches | still apply (web_search, files, commands are separately gated) | still apply |

Grant and revoke live, in the shell: `@grants read ~/projects`, `@grants write ~/notes`, `@leash unleashed`, or during setup. The agent's own data dir is always readable/writable to itself; everything else must be given.

## Releases

The **Release** workflow is manual dispatch: *Actions → Release → Run workflow* — optionally set a tag (default is `v<version from Cargo.toml>`) and notes. It cross-compiles the single binary for every target, packages + checksums them, and publishes a GitHub Release. The install script pulls `releases/latest`, so every release instantly becomes what a phone or PC installs, and the installer doubles as the updater.

| Asset (from `releases/latest`) | Runs on | Conceptual folder |
| --- | --- | --- |
| `decyphertek-aarch64-unknown-linux-musl.tar.gz` | **Termux on Android (64-bit)**, ARM Linux servers | `android-arm/` |
| `decyphertek-armv7-unknown-linux-musleabihf.tar.gz` | older 32-bit Android phones | `android-arm/` |
| `decyphertek-x86_64-unknown-linux-musl.tar.gz` | PCs, servers | `pc/` |
| `decyphertek-aarch64-apple-darwin.tar.gz` | Apple Silicon Macs | `pc/` |
| `decyphertek-x86_64-apple-darwin.tar.gz` | Intel Macs | `pc/` |

Static musl binaries run on any Linux regardless of the host's libc — the same file that runs on a stock Debian server runs unchanged inside Termux.

## Architecture

```mermaid
flowchart TB
    subgraph Core["Agent Core - single Rust executable"]
        Orchestrator["LLM Orchestrator<br/>(plain loop, no framework)"]
        Personas["Personas<br/>default · hal9000 · neuromancer · terminator"]
        Orchestrator --- Personas
    end

    subgraph Vault["~/.decyphertek.ai/ - encrypted at rest"]
        Seal[("vault.dct<br/>AES-256-GCM + Argon2id")]
        Staging["staging/ (session only)"]
        Vector[("vectors.db — RAG<br/>SQLite + hashing embeddings")]
        Wiki[("memory/wiki/*.md<br/>Wiki Memory")]
        Config[("config.json<br/>backend, leash, grants")]
        Logs[("chatlogs/*.jsonl<br/>forensic case files")]
        Seal <-->|"password"| Staging
        Staging --- Vector
        Staging --- Wiki
        Staging --- Config
        Staging --- Logs
    end

    subgraph Models["Model backends - one OpenAI-style client"]
        OpenRouter["OpenRouter<br/>default · one key, every hosted model"]
        Ollama["Ollama<br/>optional · local · slim phone models on Termux"]
    end

    subgraph Tools["Tool layer - built-in, leash-enforced"]
        Files["read/write/list files<br/>(folder grants)"]
        Web["web_search<br/>DuckDuckGo + Wikipedia"]
        Shell["run_command<br/>(gated + confirmed when leashed)"]
        Memory["memory_search · remember"]
        WikiT["read_wiki · write_wiki"]
        Mcps["@store MCP servers in Docker<br/>--network=none · stdio-only"]
    end

    subgraph Interface["@-shell"]
        Pass["passthrough<br/>commands run as typed"]
        AtCmds["@chat @code @research"]
        Store["@store — MCP store<br/>pull Docker servers A-Z"]
        Tui["TUI reports"]
        Wizard["setup wizard<br/>persona → brain → folders → leash → password"]
    end

    You([You]) -->|@ command| AtCmds
    You -->|anything else| Pass
    You -->|browse, pull, register| Store
    You -->|setup / launch| Wizard
    Wizard -->|password| Seal
    Store -->|registered servers| Mcps
    Mcps -->|tools as mcp_server_tool| Orchestrator
    AtCmds -->|task + recalled memory| Orchestrator
    Orchestrator -->|one final report| Tui
    Orchestrator -->|thoughts, tool calls| Logs
    Logs -->|chunked into memory| Vector
    Logs -->|30-day rotation| Vault
    You -->|docs in your folders| Vector
    Vector -->|retrieved context| Orchestrator
    Orchestrator <-->|prompts / completions| OpenRouter
    Orchestrator <-->|prompts / completions| Ollama
    Orchestrator <-->|gated calls| Tools
    Tools -->|DENIED beyond grants| Orchestrator
```

### How a Run Works

1. **Recall** — the task text searches the RAG store; top chunks become context.
2. **Frame** — persona + mode rules + leash summary + recalled memory become the system prompt.
3. **Loop** (max 12 iterations) — the model replies with tool calls; the orchestrator executes each through the leash, appending results; when the model produces a final answer, the loop ends. Registered @store MCP servers spawn as hardened containers at run start (network=none, stdio-only) and die with the run.
4. **Learn** — the report and full case file are chunked into memory (`report` / `chatlog` kinds); the agent may have stored distillations via `remember` during the run.
5. **Report** — one TUI panel + a stats line; the shell returns.

### The Stack

Rust end to end, deliberately small: `ureq` (HTTPS with rustls), `rusqlite` (bundled SQLite), `aes-gcm` + `argon2` + `getrandom` (the vault), `tar` + `flate2` (seal format + log rotation), `dialoguer` + `console` (TUI). No tokio, no async, no LangChain, no ML stack — the loop is plain owned code.

- **No Python, no LangChain.** The docs-first learning loop is deliberately small — prompt the model, run the tool, chunk the result, update the wiki. A framework would outweigh the app; in Rust it is just code in one binary.
- **Crates, not ecosystems.** One HTTP client for the model backends and web search, one SQLite binding for the vector store, one markdown writer for Wiki Memory — small crates, nothing dragging an ML stack behind them.
- **Cross-compiled, not ported.** Every target comes from the identical codebase: aarch64 musl for Termux (the old `android-arm/` idea), x86_64 musl for PCs (`pc/`), darwin for Macs. No maintained divergence, no separate fork for Termux.

Everything cross-compiles static; CI is `cargo test` on Linux + macOS with clippy + fmt gating.

## Development

```bash
cargo build --release        # build the binary
cargo test                   # 25 unit tests (vault, embeddings, leash, wiki, chunking, MCP store)
cargo clippy -- -D warnings  # zero-warning policy
```

End-to-end drivers live in `tests/`:

- `tests/e2e_wizard.py` — pty driver: first-launch wizard, seal, unlock, wrong-password rejection, `@status`/`@wiki`/`@leash`, seal-on-exit.
- `tests/e2e_agent.py` — full agent loop with a fake Ollama (`tests/fake_ollama.py`): tool-call protocol, `remember` → RAG write, cross-run recall, leash DENIED mid-run, silent running, forensic logging.

Both run against the release binary with a scratch `$HOME`.

## Roadmap

Phase 1 shipped in this build:

- [x] SQLite RAG vector store + hardcoded Wiki Memory baseline
- [x] Docs-first learning loop in plain Rust — no LangChain
- [x] `@`-shell passthrough — `@chat`, `@code`, `@research`
- [x] TUI final reports — the agent's only on-screen output
- [x] Forensic chat logs — structured case files, chunked into memory, monthly rotation
- [x] Keyless web research (DuckDuckGo + Wikipedia)
- [x] Model layer — OpenRouter by default, optional Ollama, one client
- [x] The leash — folder grants, tool gates, leashed/unleashed
- [x] Encrypted vault — password on launch, sealed on exit
- [x] TUI walkthrough wizard + personas (hal9000, neuromancer, terminator)
- [x] Cross-compiled release pipeline (Termux aarch64 first-class) + manual release workflow
- [x] One-command installer pulling `releases/latest` (doubles as updater)

Next phases:

- [x] MCP tool servers — `@store` TUI: search Docker (A-Z, hub live search + seed catalog), pull, register; hardened containers (`--network=none`, `--cap-drop=ALL`) that talk only to the agent over stdio; registry sealed in the vault, gated by the leash's mcp_servers switch
- [ ] Neural embeddings via Ollama `nomic-embed-text` (swap the hashing embedder, same schema)
- [ ] MCP sync servers — GitHub MCP for Wiki Memory-as-git-repo, rclone MCP for Proton Drive backup of the whole data dir
- [ ] Custom agents via user-written `AGENT.md` on top of the built-in personas
- [ ] Self-replication: binary from Releases + memory from sync = same agent on any device

## License

MIT — see [LICENSE](LICENSE).
