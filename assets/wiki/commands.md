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
example, viewable via `@wiki read research/rag-chat.yml`, and the
hardcoded `sources.yml` — public archival research databases (Internet
Archive, NARA, Federal Register, Data.gov, Congress.gov, GovInfo,
FRUS, the Pentagon UAP mirror, the CIA FOIA Reading Room, the UK
National Archives, arXiv, Crossref, Semantic Scholar, OpenAlex,
PubMed, NASA ADS, Europe PMC, the Library of Congress, Project
Gutenberg, Columbia History Lab, the National Security Archive, the
UCSF Industry Documents Library, ProPublica Nonprofit Explorer, the
FEC, the Black Vault and the OVNI Archive) — so
`@research sources.yml <topic>` digs into the world's archives out of
the box; Internet Archive answers natively through its keyless
metadata query API (archive.org/advancedsearch.php).

## @upload — your docs into the vault (info/)

- `@upload` — opens a folder picker (starting at Downloads; folders
  navigate, `(go up)` moves up, `(done)` finishes). Picked files are
  copied into the wiki's `info/` folder and chunked into RAG memory, so
  they seal into the vault and the agent can read them. Text files are
  chunked; binaries are stored as-is.

## @store — MCP tool servers

- `@store` — the MCP store: a fuzzy-searchable TUI over every MCP server
  found in Docker, A-Z — the in-binary catalog, a live Docker Hub query,
  and images already pulled on this machine.
- Pick one → pull → registered in the vault. Registered servers launch
  with the security template on every @-run: `--network=none`,
  `--cap-drop=ALL`, no-new-privileges, stdio-only. A server can only
  ever answer the agent — it cannot publish ports, reach the LAN, or
  call out. Their tools appear as `mcp_<server>_<tool>`.
- `@store` also flips servers enabled/disabled, updates images, and
  uninstalls. Some servers need API keys (e.g. GitHub MCP wants
  `GITHUB_PERSONAL_ACCESS_TOKEN`) — provide them per server image docs.

## Management

- `@ingest <folder>` — chunk a folder's docs into RAG memory, grants read.
- `@grants read <path>` / `@grants write <path>` — grant folder access.
- `@leash leashed|unleashed` — enforce or drop folder scopes; unleashed turns every ability on.
- `@wiki list` / `@wiki read <name>` — browse memory pages (also
  `research/<profile>.yml` and `info/<uploaded-doc>` entries).
- `@status` — brain, leash, grants, MCP servers, RAG size, wiki size.
- `@setup` — re-run the walkthrough (brain, folders, tools — including
  the MCP gate, registered servers, and creating research profiles).
- `@password` — re-key the encrypted vault.
- `@help` — the list.
- `exit` (or Ctrl-D) — seal the vault and quit.

Anything else runs in your shell, untouched — including `cd [path]`
(`~`, `..`, `-`), which moves the shell like in any terminal.

