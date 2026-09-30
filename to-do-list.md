# DeCypherTek.ai — To-Do List

Future work, one task per checkbox. Each task lands the way everything
else lands: branch → PR → merged to `main` → the `dev-*` channels get
rebased onto it. A task only moves to **Done** when its acceptance
criteria pass and the CI gates are green.

## Tasks

- [ ] **Task 1 — RAG: ingest any data source, store real vectors in
      SQLite** (makes @chat remember anything, not just text files)

---

## Task 1 — RAG: any data source → real vectors in SQLite

**Goal.** `/ingest` and the wizard's memory step must accept any data
source — not only the text-extension allow-list — extract its text,
chunk it, embed it with a **semantic** embedder, and store the vectors
in the existing `vectors.db` inside the sealed vault, searchable by
the cosine search the RAG chat already runs.

**Current state** (`src/vector.rs`):
- chunking: ~900-char paragraph split, hard-split with overlap,
  dedupe by FNV-1a64 content hash
- embedder: 256-dim signed feature hashing of unigrams + bigrams —
  lexical, not semantic (no "meaning" in the vectors)
- storage: one SQLite file, bundled rusqlite, embeddings as BLOBs
- search: full-scan cosine in Rust — fine at phone corpus size
- ingest: folder walk, text-extension allow-list, 2 MB cap per file

**Work items**
1. One `Ingestor` trait, adapters behind it:
   - text/code files (today's allow-list, unchanged)
   - structured: CSV/TSV/JSON/Parquet — rows rendered as
     `column: value` text; polars reads Parquet through Arrow without
     us owning a parser
   - extracted: PDF (`pdf-extract`/`lopdf`), DOCX/XLSX/PPTX and EPUB
     (zip + embedded xml/xhtml text)
   - database files: existing SQLite/DuckDB databases opened read-only,
     rows rendered as text — each source type capped like the 2 MB
     text guard, caps documented
2. Embedder becomes a config choice — offline default stays:
   - `feature-hash` — today's embedder (zero download, deterministic)
   - `ollama` — `nomic-embed-text` through the configured local
     Ollama; same schema, same cosine search (`vector.rs` reserved
     exactly this swap in its module doc)
   - optional local ONNX (`fastembed`/`candle`) — only if it actually
     cross-builds for our musl targets (open question below)
3. Vector storage pick — measure, then document the choice:
   - embeddings as BLOBs + cosine scan (today; fine to tens of
     thousands of chunks)
   - ANN index in Rust (`hnsw_rs`) persisted next to the vectors
   - `sqlite-vec` loadable extension — verify extension loading works
     inside bundled rusqlite in a static musl binary before betting

**Candidate crates** (landscape verified Dec 2025 — re-verify when
the task is picked up; this is the rust equivalents of the
pandas/sklearn question):

| Need | Crate | State |
| --- | --- | --- |
| DataFrames / CSV-Parquet-JSON | `polars` + `polars-lazy` | mature; the Rust engine behind Python polars |
| sklearn-style ML (only if models are ever needed) | `linfa` 0.8.1 sub-crates | alive, releases ~1/year; BLAS via feature flags |
| Local ONNX embeddings | `fastembed-rs` | Qdrant ecosystem; BGE-small ≥130 MB — phone? musl? |
| Low-level transformer inference | `candle` | Hugging Face's Rust ML framework |
| ANN index | `hnsw_rs` | pure Rust, no service |
| Vectors in SQLite | `sqlite-vec` | successor of sqlite-vss; extension loading to verify |
| LLM + embedding abstractions | `rig-core` | trait-based; a `rig-fastembed` bridge exists |

Too young to trust yet (re-check first): `sklears`, `rrag`,
`rust-rag-toolchain`, `rag_engine`.

**Constraints** (hard lines the task must respect)
- one static binary, cross-built musl for aarch64/armv7/x86_64 (+mac)
  — any new dep must build there; CI proves it
- phone-first: "no model download, works offline after install" stays
  the default path; heavier embedders are opt-in
- vectors live inside the sealed vault — never an outside DB service
- ingestion stays behind the leash: only granted/readable folders

**Acceptance criteria**
- a CSV and a Parquet file ingest, and @chat answers questions about
  their contents
- a PDF and a DOCX ingest (text extracted, header/footer noise
  tolerated)
- with Ollama configured, semantic recall beats `feature-hash` on a
  fixed test corpus (add that unit test to `vector.rs`)
- cross-build matrix green, no new runtime service
- gates: `cargo fmt --check`, `clippy -D warnings`, `cargo test`, the
  two e2e scripts

**Open questions**
- does `fastembed`'s ONNX runtime build for musl at all — or does the
  Ollama embedder cover "local + semantic" well enough?
- can `sqlite-vec` be loaded from bundled rusqlite in a static binary,
  or is BLOBs + `hnsw_rs` the pragmatic route?
- chunking for tables: bundle rows so related rows land in one chunk
  instead of one chunk per row?

## Done

(none yet — finished tasks move here with their PR number)
