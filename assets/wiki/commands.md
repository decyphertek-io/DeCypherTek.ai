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

## @mcp — MCP tool servers

- `@mcp` — a question-driven menu over every MCP server found in
  Docker, A-Z: the in-binary catalog, a live Docker Hub query, and
  images already pulled on this machine.
- **Search for a new MCP server** — pick one, pull it (success is the
  runtime's exit status plus an image inspect — docker and podman both
  report honestly), then a short setup walk:
  - **API keys**: the questions know what the image needs (e.g. GitHub
    MCP wants `GITHUB_PERSONAL_ACCESS_TOKEN`) — hidden input, values
    sealed in the encrypted vault, shown masked ever after, injected
    via a 0600 env-file (never in argv or `ps`).
  - **Mode permissions**: choose which runs may use the server —
    `/chat`, `/code`, `/research`, each independently (default: all).
  - **Verify**: launches the hardened container and lists its tools —
    before you leave the menu you know it works, and how its tools are
    named (`mcp_<server>_<tool>`).
- **Setup an MCP server** — manage what is registered: add/replace or
  remove keys, permission toggles, rename, enable/disable, update the
  image, re-verify, remove (the image stays on disk).
- Registered servers launch with the security template on every
  allowed run: `--cap-drop=ALL`, no-new-privileges, stdio-only,
  nothing published — plus outbound internet (a server must reach
  the web APIs it serves), unless the server opts into
  `--network=none` (never-network servers: filesystem, sqlite,
  memory, time). Nothing on the machine can reach it.

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

