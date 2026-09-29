# @-Shell Commands

The terminal stays a normal terminal: everything you type passes through to
the system and runs exactly as typed. Only @-commands wake the agent —
they act like aliases that hand work to the agent and report back.

## Modes

- `@chat <task>` — conversation backed by full memory.
- `@code <task>` — hands-on: read, change, verify; ends with diffs.
- `@research <topic>` — memory + web; ends in a written report with sources.

While a run is live the screen stays quiet (the case file streams to chat
logs); when it finishes one TUI report renders and the shell returns.

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
- `@leash leashed|unleashed` — enforce or drop folder scopes.
- `@wiki list` / `@wiki read <name>` — browse memory pages.
- `@status` — brain, leash, grants, MCP servers, RAG size, wiki size.
- `@setup` — re-run the walkthrough (persona, brain, folders, tools —
  including the MCP gate and registered servers).
- `@password` — re-key the encrypted vault.
- `@help` — the list.
- `exit` (or Ctrl-D) — seal the vault and quit.

Anything else runs in your shell, untouched — exit status shown.

