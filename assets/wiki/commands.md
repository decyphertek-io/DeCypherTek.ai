# @-Shell Commands

The terminal stays a normal terminal: everything you type passes through to
the system and runs exactly as typed. Only @-commands wake the agent.

## Modes

- `@chat <task>` — conversation backed by full memory.
- `@code <task>` — hands-on: read, change, verify; ends with diffs.
- `@research <topic>` — memory + web; ends in a written report with sources.

While a run is live the screen stays quiet (the case file streams to chat
logs); when it finishes one TUI report renders and the shell returns.

## Management

- `@ingest <folder>` — chunk a folder's docs into RAG memory, grants read.
- `@grants read <path>` / `@grants write <path>` — grant folder access.
- `@leash leashed|unleashed` — enforce or drop folder scopes.
- `@wiki list` / `@wiki read <name>` — browse memory pages.
- `@status` — brain, leash, grants, RAG size, wiki size.
- `@setup` — re-run the walkthrough (persona, brain, folders, tools).
- `@password` — re-key the encrypted vault.
- `@help` — the list.
- `exit` (or Ctrl-D) — seal the vault and quit.

Anything else runs in your shell, untouched — exit status shown.
