# RAG and Memory

DeCypherTek's memory is deliberately simple: one SQLite file plus markdown,
no external database, no model download — which is why it runs on a phone.

## The vector store (vectors.db)

- Text is chunked (~900 chars, paragraph-aware) with overlap on hard splits.
- Each chunk gets a 256-dim feature-hashing embedding: word unigrams +
  bigrams, FNV-1a hashed into signed buckets, L2-normalized. Fully
  deterministic, fully offline, microsecond embedding.
- Search runs on **SQLite-Vector** (sqlite.ai/sqlite-vector) — the C
  extension is compiled right into the binary (no loadable `.so`, so the
  one-static-musl-binary deployment is untouched). Vectors stay ordinary
  BLOBs in the `chunks` table; `vector_quantize` holds an INT8 index of
  them and queries go through the SIMD `vector_quantize_scan` — exact
  cosine results in flavor, ~13x faster than the row-by-row scan at
  99.5% recall. If the extension ever fails to load, a pure-Rust cosine
  scan answers the same queries (same results, slower).
- Kinds of knowledge, one table: chunks carry a `kind`.

## Differential indexing (the store never goes stale)

Every durable document is tracked in a `sources(source, fhash)` table:

- `sync_source` hashes the document content and **skips unchanged
  documents entirely**; changed documents have their chunk set replaced
  in full, so an edit can never leave stale chunks behind.
- Documents that vanish (deleted wiki page, removed file) are pruned.
- `/rag` (folders and single files) and every wiki write all go through
  it, so re-indexing an unchanged corpus is free.

## Kinds of knowledge in the store

- `wiki` — every wiki page, auto-indexed
- `docs` — everything you `/rag`: folders or single files — text, code,
  PDF, Word (docx, legacy doc), EPUB — extracted and chunked
- `learned` — distilled learnings the agent stores with `remember`
- `report` — final reports of past runs
- `chatlog` — whole case files (prompt → thoughts → tool calls → report)

## The wiki (memory/wiki/*.md)

Durable, human-readable knowledge: the shipped baseline (this handbook),
plus `memory-<topic>.md` pages the agent writes when it learns something
structured. Every page is chunked into vector memory the moment it is
written (and on launch, differentially) — one `memory_search` reaches all
of it without list-then-read token costs.

**`memory-index`** is the memory directory: an auto-generated page listing
every page, indexed document and research profile with one-line
summaries. Read it first when unsure where knowledge lives;
`memory_search` digs deeper.

Read the wiki, edit it, hand-evolve the agent.

## Roadmap: neural embeddings

With Ollama installed, `nomic-embed-text` can replace the hashing embedder
for more semantic recall — same schema, same SQLite-Vector search, just a
different `embed()` function. The hashing embedder keeps the default
zero-dependency phone build small.
