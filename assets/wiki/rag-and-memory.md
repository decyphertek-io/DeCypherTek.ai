# RAG and Memory

DeCypherTek's memory is deliberately simple: one SQLite file plus markdown,
no external database, no model download — which is why it runs on a phone.

## The vector store (vectors.db)

- Text is chunked (~900 chars, paragraph-aware) with overlap on hard splits.
- Each chunk gets a 256-dim feature-hashing embedding: word unigrams +
  bigrams, FNV-1a hashed into signed buckets, L2-normalized. Fully
  deterministic, fully offline, ~microsecond search at phone scale.
- Search = cosine similarity, in Rust, across chunks.
- Rows are content-deduplicated (FNV-64 hash), so re-ingesting a folder
  does not double the memory.

## Kinds of knowledge in the store

- `docs` — the folders you gave it (RAG ingest)
- `learned` — distilled learnings the agent stores with `remember`
- `report` — final reports of past runs
- `chatlog` — whole case files (prompt → thoughts → tool calls → report)

## The wiki (memory/wiki/*.md)

Durable, human-readable knowledge: the shipped baseline (this handbook),
plus `memory-<topic>.md` pages the agent writes when it learns something
structured. Read them, edit them, hand-evolve the agent.

## Roadmap: neural embeddings

With Ollama installed, `nomic-embed-text` can replace the hashing embedder
for better recall — same schema, same cosine search, just a different
`embed()` function. The hashing embedder keeps the default zero-dependency
phone build small.
