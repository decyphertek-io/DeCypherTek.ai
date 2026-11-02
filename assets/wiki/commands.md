# @-Shell Commands

The terminal is a classic terminal, down to its prompt: `decyphertek.ai:~$`.
Everything you type passes through to the system and runs exactly as typed
(`cd` is a real built-in — the path in the prompt follows you). Only
@-commands wake the agent — they act like aliases that hand work to the
agent and report back.

## Modes

- `@chat <task>` — conversation backed by full memory.
- `@code <task>` — hands-on: read, change, verify; ends with diffs.
- `@research <topic>` — memory + web; ends in a written report with sources.
- `@research <name>.yml <topic>` — same, but web searching is locked to
  the sites of the research profile `name.yml` (see below) instead of a
  general web search.

While a run is live the screen shows one line —
`Processing Request............` — while the case file streams to chat
logs; when it finishes one TUI report renders and the shell returns.

## Research profiles (research/)

A research profile is a small YAML file in the wiki's `research/` folder:

    name: rag-chat
    description: RAG + chat research sources
    sites:
      - https://arxiv.org
      - https://en.wikipedia.org

`@research <name>.yml <topic>` runs the research with `web_search` and
`web_fetch` restricted to exactly those sites (site-scoped queries plus
the keyless native APIs of archive.org, arxiv.org, news.ycombinator.com
and wikipedia.org when they are in the profile). Create or change
profiles in `@setup` — a new one is saved into `research/`
automatically. Two baselines ship inside the binary: the `rag-chat.yml`
example, and the hardcoded `sources.yml` — public archival research
databases (Internet Archive, NARA, Federal Register, Data.gov,
Congress.gov, GovInfo, FRUS, the Pentagon UAP mirror, the CIA FOIA
Reading Room, the UK National Archives, arXiv, Crossref, Semantic
Scholar, OpenAlex, PubMed, NASA ADS, Europe PMC, the Library of
Congress, Project Gutenberg, Columbia History Lab, the National
Security Archive, the UCSF Industry Documents Library, ProPublica
Nonprofit Explorer, the FEC, the Black Vault and the OVNI Archive) —
so `@research sources.yml <topic>` digs into the world's archives out
of the box; Internet Archive answers natively through its keyless
metadata query API (archive.org/advancedsearch.php).

## @rag — your documents into RAG memory

- `@rag <folder> *` — recursively index every supported document in
  the folder: text, notes, code, **PDF, Word (docx and legacy doc),
  EPUB**. It is differential: changed documents are re-chunked,
  deleted ones pruned, unchanged ones cost nothing — re-run it as
  often as you like. READ on the folder is granted so the agent can
  also open the originals.
- `@rag <folder> <name.ext>` — index one document by name.
- Then just ask: `@chat <question>` — answers come straight from
  vector memory (one memory_search reaches the right chunk), never by
  the agent re-reading whole folders or wasting tokens.

## @mcp — tool servers for the AI

- `@mcp` — a short menu: find a new server, or set up one you already
  have. The search covers a built-in list, a live Docker Hub scan, and
  whatever is already downloaded on this machine.
- **Add a server** — pick one; /mcp downloads it (and tells you plainly
  whether the download worked — Docker and Podman both), then asks a
  few plain questions:
  - **Keys**: if the server needs a sign-in key — GitHub wants your
    `GITHUB_PERSONAL_ACCESS_TOKEN`, for example — you type it once,
    hidden. It's saved sealed in your encrypted vault, shown masked
    ever after, and handed to the server in a protected file (never
    visible in the process list).
  - **Which commands can use it**: `/chat`, `/code`, `/research`, each
    on or off (default: all three).
  - **A test run**: /mcp starts the server in its locked-down
    container and shows what it can do. You leave the menu knowing it
    works, and what its skills are called (`mcp_<server>_<tool>` — the
    names the AI answers to).
- **Set up an installed server** — add, change or remove keys; rename
  it; turn it on or off; update to the newest image; test it again; or
  remove it (the downloaded copy stays on your machine).
- Every installed server runs cut off from the internet: no network,
  no special rights — it can only answer the AI. It cannot publish
  ports, reach the LAN, or call out.

## Management

- `@grants read <path>` / `@grants write <path>` — grant folder access.
- `@leash leashed|unleashed` — enforce or drop folder scopes; unleashed turns every ability on.
- `@status` — brain, leash, grants, MCP servers, RAG size, wiki size.
- `@setup` — re-run the walkthrough (brain, folders, tools — including
  the MCP gate, registered servers, and creating research profiles).
- `@password` — re-key the encrypted vault.
- `@help` — the list.
- `exit` (or Ctrl-D) — seal the vault and quit.

Anything else runs in your shell, untouched — including `cd [path]`
(`~`, `..`, `-`), which moves the shell like in any terminal.

