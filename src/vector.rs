//! RAG vector store — one SQLite file, zero external services.
//!
//! Search runs on SQLite-Vector (sqlite.ai/sqlite-vector, vendored in
//! vendor/sqlite-vector and compiled into the binary by build.rs under
//! -DSQLITE_CORE). Vectors stay ordinary BLOBs in the `chunks` table;
//! `vector_quantize` keeps an INT8 index of them and queries go through
//! the SIMD `vector_quantize_scan` (99.5% cosine recall at ~13x the
//! exact scan). If the extension ever fails to initialize the pure-Rust
//! cosine scan below is the fallback — same results, slower.
//!
//! Differential indexing: every durable document (wiki page, ingested
//! file, indexed document) is registered in `sources(source, fhash)`.
//! `sync_source` compares the content hash and rewrites the document's
//! chunk set only when the content changed; vanished documents are
//! pruned. Re-indexing an unchanged corpus is free, and the store never
//! keeps stale chunks from old file versions. When Ollama is configured
//! you can swap the embedder for `nomic-embed-text` later — same schema,
//! same search.

use crate::util::{fnv1a32, fnv1a64, now_iso};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::cell::Cell;
use std::path::Path;

pub const DIMS: usize = 256;
const TARGET_CHUNK: usize = 900;
const HARD_CHUNK: usize = 1200;

/// The extension's `vector_init` options for our column: little-endian
/// f32 BLOBs produced by `embed()`, which L2-normalizes every vector,
/// so `normalized=1` lets cosine collapse to `1 - dot`.
const VECTOR_INIT_OPTS: &str = "type=FLOAT32,dimension=256,distance=cosine,normalized=1";

// The extension's entry point, from vendor/sqlite-vector (build.rs
// compiles it into the binary). Its signature IS the generic
// auto-extension entry point sqlite3_auto_extension expects.
extern "C" {
    fn sqlite3_vector_init(
        db: *mut rusqlite::ffi::sqlite3,
        pz_errmsg: *mut *mut std::os::raw::c_char,
        api: *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::os::raw::c_int;
}

/// Register the embedded SQLite-Vector extension process-wide so every
/// connection opened afterwards has its functions. The C init is
/// idempotent per connection (it probes `vector_version()` first).
fn register_extension() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(sqlite3_vector_init));
    });
}

pub struct Vectors {
    conn: Connection,
    /// The extension's functions are live on this connection.
    ext: Cell<bool>,
    /// The INT8 quantized index exists and matches the live table.
    quantized: Cell<bool>,
    /// Rows changed since the last quantize — the index needs a rebuild.
    dirty: Cell<bool>,
}

/// What a `sync_source` did to the store.
#[derive(Debug, PartialEq)]
pub enum SyncOutcome {
    Unchanged,
    Changed { chunks: usize, added: usize },
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub source: String,
    pub kind: String,
    pub content: String,
    pub score: f32,
}

impl Vectors {
    pub fn open(db_path: &Path) -> Result<Vectors> {
        register_extension();
        let conn = Connection::open(db_path).context("open vector db")?;
        migrate(&conn)?;
        let v = Vectors {
            conn,
            ext: Cell::new(false),
            quantized: Cell::new(false),
            dirty: Cell::new(true),
        };
        let version: Option<String> = v
            .conn
            .query_row("SELECT vector_version()", [], |r| r.get(0))
            .ok();
        if version.is_none() {
            // Extension compiled in but refused to start: the pure-Rust
            // scan below still answers every query, just slower.
            return Ok(v);
        }
        let ok = v
            .conn
            .query_row(
                "SELECT vector_init('chunks', 'embedding', ?1)",
                rusqlite::params![VECTOR_INIT_OPTS],
                |_| Ok(()),
            )
            .is_ok();
        v.ext.set(ok);
        Ok(v)
    }

    /// Rebuild the INT8 quantized index if rows changed. Cheap at this
    /// corpus scale (one C table scan) and it keeps every later search
    /// on the fast SIMD path.
    pub fn ensure_index(&self) {
        if !self.dirty.get() {
            return;
        }
        self.dirty.set(false);
        if !self.ext.get() {
            return;
        }
        let empty = self.count().unwrap_or(0) == 0;
        if empty {
            self.quantized.set(false);
            return;
        }
        let quantized = self
            .conn
            .query_row(
                "SELECT vector_quantize('chunks', 'embedding', 'qtype=INT8')",
                [],
                |r| {
                    let _: i64 = r.get(0)?;
                    Ok(())
                },
            )
            .is_ok()
            && self
                .conn
                .query_row(
                    "SELECT vector_quantize_preload('chunks', 'embedding')",
                    [],
                    |_| Ok(()),
                )
                .is_ok();
        self.quantized.set(quantized);
    }

    pub fn insert_chunk(&self, source: &str, kind: &str, content: &str) -> Result<bool> {
        let hash = fnv1a64(content.as_bytes()) as i64;
        let emb = embed(content);
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO chunks(source, kind, content, chash, embedding, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![source, kind, content, hash, emb_to_blob(&emb), now_iso()],
        )? > 0;
        if changed {
            self.dirty.set(true);
        }
        Ok(changed)
    }

    pub fn insert_text(&self, source: &str, kind: &str, text: &str) -> Result<(usize, usize)> {
        let chunks = chunk_text(text);
        let mut added = 0;
        for c in &chunks {
            if self.insert_chunk(source, kind, c)? {
                added += 1;
            }
        }
        Ok((chunks.len(), added))
    }

    /// Differential index of one durable document. If the content hash
    /// matches the last sync the store is untouched; otherwise the
    /// document's chunk set is replaced in full, so edits never leave
    /// stale chunks behind.
    pub fn sync_source(&self, source: &str, kind: &str, text: &str) -> Result<SyncOutcome> {
        let fhash = fnv1a64(text.as_bytes()) as i64;
        let known: Option<i64> = self
            .conn
            .query_row(
                "SELECT fhash FROM sources WHERE source = ?1",
                rusqlite::params![source],
                |r| r.get(0),
            )
            .ok();
        if known == Some(fhash) {
            return Ok(SyncOutcome::Unchanged);
        }
        self.conn.execute(
            "DELETE FROM chunks WHERE source = ?1",
            rusqlite::params![source],
        )?;
        self.conn.execute(
            "DELETE FROM sources WHERE source = ?1",
            rusqlite::params![source],
        )?;
        let (chunks, added) = self.insert_text(source, kind, text)?;
        self.conn.execute(
            "INSERT INTO sources(source, kind, fhash, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![source, kind, fhash, now_iso()],
        )?;
        Ok(SyncOutcome::Changed { chunks, added })
    }

    /// Drop a document from the store (its file is gone).
    pub fn remove_source(&self, source: &str) -> Result<bool> {
        let chunks = self.conn.execute(
            "DELETE FROM chunks WHERE source = ?1",
            rusqlite::params![source],
        )?;
        let sources = self.conn.execute(
            "DELETE FROM sources WHERE source = ?1",
            rusqlite::params![source],
        )?;
        if chunks > 0 {
            self.dirty.set(true);
        }
        Ok(chunks > 0 || sources > 0)
    }

    /// All indexed document sources of one kind — used to prune
    /// documents that no longer exist on disk.
    pub fn sources_with_kind(&self, kind: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source FROM sources WHERE kind = ?1 ORDER BY source")?;
        let rows = stmt.query_map(rusqlite::params![kind], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?)
    }

    pub fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>> {
        let q = embed(query);
        if self.ext.get() {
            self.ensure_index();
            // Top-k mode: the scan returns the k nearest rows sorted by
            // distance. INT8 quantized when the index is fresh, exact
            // full-precision SIMD scan otherwise.
            let sql = if self.quantized.get() {
                "SELECT c.source, c.kind, c.content, v.distance \
                 FROM vector_quantize_scan('chunks', 'embedding', ?1, ?2) AS v \
                 JOIN chunks AS c ON c.id = v.rowid"
            } else {
                "SELECT c.source, c.kind, c.content, v.distance \
                 FROM vector_full_scan('chunks', 'embedding', ?1, ?2) AS v \
                 JOIN chunks AS c ON c.id = v.rowid"
            };
            if let Ok(mut stmt) = self.conn.prepare(sql) {
                let rows = stmt.query_map(rusqlite::params![emb_to_blob(&q), k as i64], |r| {
                    let dist: f64 = r.get(3)?;
                    Ok(Hit {
                        source: r.get(0)?,
                        kind: r.get(1)?,
                        content: r.get(2)?,
                        score: 1.0 - dist as f32,
                    })
                });
                if let Ok(rows) = rows {
                    let mut hits: Vec<Hit> = Vec::new();
                    for row in rows.flatten() {
                        hits.push(row);
                    }
                    hits.sort_by(|a, b| {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    hits.truncate(k);
                    return Ok(hits);
                }
            }
        }
        self.search_rust(&q, k)
    }

    /// Pure-Rust exact scan — the always-available fallback.
    fn search_rust(&self, q: &[f32; DIMS], k: usize) -> Result<Vec<Hit>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source, kind, content, embedding FROM chunks")?;
        let rows = stmt.query_map([], |r| {
            let source: String = r.get(0)?;
            let kind: String = r.get(1)?;
            let content: String = r.get(2)?;
            let blob: Vec<u8> = r.get(3)?;
            Ok((source, kind, content, blob))
        })?;
        let mut hits: Vec<Hit> = Vec::new();
        for row in rows {
            let (source, kind, content, blob) = row?;
            let score = cosine(q, &blob_to_emb(&blob));
            hits.push(Hit {
                source,
                kind,
                content,
                score,
            });
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// Index one document (any supported type) into the store — the
    /// single-file path of `/rag <folder> <file>`.
    pub fn ingest_file(&self, path: &Path) -> Result<IngestReport> {
        let mut report = IngestReport {
            files: 1,
            ..IngestReport::default()
        };
        self.ingest_one(path, &mut report)?;
        self.ensure_index();
        Ok(report)
    }

    /// Recursively index a folder into the store, differentially: files
    /// whose content hash changed are re-chunked, new files chunked,
    /// files that vanished since the last ingest are pruned.
    pub fn ingest_folder(&self, folder: &Path) -> Result<IngestReport> {
        let mut report = IngestReport::default();
        let mut seen: Vec<String> = Vec::new();
        let mut stack = vec![folder.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue; // hidden files/dirs/.git
                }
                if path.is_dir() {
                    stack.push(path);
                } else {
                    report.files += 1;
                    if !crate::extract::is_supported(&path) {
                        report.unsupported += 1;
                        continue;
                    }
                    if let Ok(source) = self.ingest_one(&path, &mut report) {
                        seen.push(source);
                    }
                }
            }
        }
        // Prune files that no longer exist under this folder.
        let prefix = format!("{}/", folder.to_string_lossy().trim_end_matches('/'));
        for source in self.sources_with_kind("docs")? {
            if source.starts_with(&prefix) && !seen.contains(&source) {
                self.remove_source(&source)?;
            }
        }
        self.ensure_index();
        Ok(report)
    }

    /// Extract + differential-sync one file; returns its source name
    /// when it produced indexable text.
    fn ingest_one(&self, path: &Path, report: &mut IngestReport) -> Result<String> {
        let source = path.to_string_lossy().to_string();
        match crate::extract::extract(path) {
            Ok(crate::extract::Extracted::Text(text)) => {
                if let SyncOutcome::Changed { added, .. } =
                    self.sync_source(&source, "docs", &text)?
                {
                    report.indexed += 1;
                    report.added += added;
                }
                Ok(source)
            }
            Ok(crate::extract::Extracted::Empty(reason)) => {
                report
                    .skipped
                    .push(format!("{} — {reason}", path.display()));
                Ok(source)
            }
            Ok(crate::extract::Extracted::Unsupported) => {
                report.unsupported += 1;
                Ok(source)
            }
            Err(e) => {
                report.skipped.push(format!("{} — {e}", path.display()));
                Ok(source)
            }
        }
    }
}

/// One `/rag` run's totals, for the user-facing summary.
#[derive(Default)]
pub struct IngestReport {
    pub files: usize,
    pub indexed: usize,
    pub added: usize,
    pub unsupported: usize,
    pub skipped: Vec<String>,
}

/// Create the current schema, then migrate legacy stores forward.
/// Legacy v1 held a GLOBAL `UNIQUE` on `chash` (chunk content hash):
/// dedupe across documents, which silently broke per-source
/// replacement — deleting one document's chunks could orphane a chunk
/// shared with another source, and re-syncs could under-insert. The
/// rebuilt key is `(source, chash)`: dedupe within one document,
/// independence across documents.
fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chunks(
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             source TEXT NOT NULL,
             kind TEXT NOT NULL,
             content TEXT NOT NULL,
             chash INTEGER NOT NULL,
             embedding BLOB NOT NULL,
             created_at TEXT NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_src_hash ON chunks(source, chash);
         CREATE INDEX IF NOT EXISTS idx_chunks_kind ON chunks(kind);
         CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source);
         CREATE TABLE IF NOT EXISTS sources(
             source TEXT PRIMARY KEY,
             kind TEXT NOT NULL,
             fhash INTEGER NOT NULL,
             updated_at TEXT NOT NULL
         );",
    )?;
    let legacy: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks'",
            [],
            |r| r.get(0),
        )
        .context("read chunks schema")?;
    if !matches!(&legacy, Some(ddl) if ddl.contains("chash INTEGER NOT NULL UNIQUE")) {
        return Ok(());
    }
    conn.execute_batch(
        "CREATE TABLE chunks_migrated(
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             source TEXT NOT NULL,
             kind TEXT NOT NULL,
             content TEXT NOT NULL,
             chash INTEGER NOT NULL,
             embedding BLOB NOT NULL,
             created_at TEXT NOT NULL
         );
         INSERT INTO chunks_migrated(id, source, kind, content, chash, embedding, created_at)
             SELECT id, source, kind, content, chash, embedding, created_at
             FROM chunks ORDER BY id;
         DROP TABLE chunks;
         ALTER TABLE chunks_migrated RENAME TO chunks;",
    )?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_src_hash ON chunks(source, chash);
         CREATE INDEX IF NOT EXISTS idx_chunks_kind ON chunks(kind);
         CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source);",
    )?;
    Ok(())
}

// ---------- embeddings ----------

/// Deterministic feature hashing: word unigrams + bigrams -> 256 dims.
pub fn embed(text: &str) -> [f32; DIMS] {
    let tokens = tokenize(text);
    let mut v = [0f32; DIMS];
    for i in 0..tokens.len() {
        hash_token(&tokens[i], &mut v);
        if i + 1 < tokens.len() {
            let big = format!("{} {}", tokens[i], tokens[i + 1]);
            hash_token(&big, &mut v);
        }
    }
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

fn hash_token(tok: &str, v: &mut [f32; DIMS]) {
    let h = fnv1a32(tok.as_bytes());
    let idx = (h % DIMS as u32) as usize;
    let sign = if h & 0x8000_0000 == 0 { 1.0 } else { -1.0 };
    v[idx] += sign;
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric()))
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_string())
        .collect()
}

fn emb_to_blob(v: &[f32; DIMS]) -> Vec<u8> {
    let mut out = Vec::with_capacity(DIMS * 4);
    for x in v.iter() {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn blob_to_emb(blob: &[u8]) -> [f32; DIMS] {
    let mut v = [0f32; DIMS];
    for i in 0..DIMS.min(blob.len() / 4) {
        v[i] = f32::from_le_bytes([
            blob[i * 4],
            blob[i * 4 + 1],
            blob[i * 4 + 2],
            blob[i * 4 + 3],
        ]);
    }
    v
}

fn cosine(a: &[f32; DIMS], b: &[f32; DIMS]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na > 0.0 && nb > 0.0 {
        dot / (na * nb)
    } else {
        0.0
    }
}

// ---------- chunking ----------

/// Split into ~900-char chunks on paragraph boundaries, hard-split
/// oversized paragraphs with overlap.
pub fn chunk_text(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n");
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();

    for para in normalized.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if para.len() > HARD_CHUNK {
            if !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
            for piece in hard_split(para) {
                chunks.push(piece);
            }
            continue;
        }
        if cur.len() + para.len() + 2 > TARGET_CHUNK && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(para);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

fn hard_split(para: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = para.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + TARGET_CHUNK).min(chars.len());
        let piece: String = chars[i..end].iter().collect();
        out.push(piece.trim().to_string());
        if end == chars.len() {
            break;
        }
        i = end.saturating_sub(100); // small overlap
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdb(tag: &str) -> Vectors {
        let dir = std::env::temp_dir().join(format!("dct-vec-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Vectors::open(&dir.join("v.db")).unwrap()
    }

    /// Sources of hits that actually match (the chat loop drops
    /// sub-threshold noise the same way — score > 0.05 there).
    fn hit_sources(v: &Vectors, query: &str) -> Vec<String> {
        v.search(query, 8)
            .unwrap()
            .into_iter()
            .filter(|h| h.score > 0.1)
            .map(|h| h.source)
            .collect()
    }

    #[test]
    fn embedding_is_deterministic_and_normalized() {
        let a = embed("rust cargo build");
        let b = embed("rust cargo build");
        assert_eq!(a, b);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    #[test]
    fn extension_is_live_and_searches() {
        let v = tmpdb("ext");
        v.insert_text(
            "a.md",
            "docs",
            "The rusty cargo crate builds wheels with cargo build release",
        )
        .unwrap();
        v.insert_text(
            "b.md",
            "docs",
            "Bananas are yellow fruit grown in tropical plantations",
        )
        .unwrap();
        v.ensure_index();
        assert!(
            v.ext.get(),
            "SQLite-Vector compiled in: vector_version/init must succeed"
        );
        let hits = v.search("how do I build a crate with cargo", 2).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].source, "a.md");
        assert!(hits[0].score > 0.3, "cosine of a near-match: {hits:?}");
    }

    #[test]
    fn chunking_respects_target() {
        let text = (0..80)
            .map(|i| format!("Paragraph number {i} with some words inside it."))
            .collect::<Vec<_>>()
            .join("\n\n");
        for c in chunk_text(&text) {
            assert!(c.len() <= HARD_CHUNK + 20, "chunk too big: {}", c.len());
        }
    }

    #[test]
    fn sync_source_is_differential() {
        let v = tmpdb("sync");
        let changed = v
            .sync_source(
                "wiki/page.md",
                "wiki",
                "alpha: the kettle schematics live here",
            )
            .unwrap();
        assert_eq!(
            changed,
            SyncOutcome::Changed {
                chunks: 1,
                added: 1
            }
        );
        let again = v
            .sync_source(
                "wiki/page.md",
                "wiki",
                "alpha: the kettle schematics live here",
            )
            .unwrap();
        assert_eq!(again, SyncOutcome::Unchanged);

        // An edit replaces the old chunks — the stale text must be gone.
        v.sync_source(
            "wiki/page.md",
            "wiki",
            "bravo: the espresso machine manual lives here now",
        )
        .unwrap();
        assert!(
            !v.sources_with_kind("wiki").unwrap().is_empty()
                && hit_sources(&v, "espresso machine manual").contains(&"wiki/page.md".into())
        );
        let hits = v.search("kettle schematics", 8).unwrap();
        assert!(
            hits.iter()
                .all(|h| h.source != "wiki/page.md" || h.score <= 0.1),
            "stale chunk survived the re-sync: {hits:?}"
        );

        // Removal prunes the source and its chunks.
        assert!(v.remove_source("wiki/page.md").unwrap());
        assert!(v.sources_with_kind("wiki").unwrap().is_empty());
        assert!(v
            .search("espresso machine manual", 8)
            .unwrap()
            .iter()
            .all(|h| h.source != "wiki/page.md"));
    }

    #[test]
    fn chunk_dedupe_is_per_source() {
        let v = tmpdb("dedupe");
        v.insert_text("one.md", "docs", "identical content here")
            .unwrap();
        let (total, added) = v
            .insert_text("one.md", "docs", "identical content here")
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(added, 0, "same source + same content must dedupe");
        // A different document owning the same text stores its own row,
        // so deleting one document never orphans the other.
        let (_, added) = v
            .insert_text("two.md", "docs", "identical content here")
            .unwrap();
        assert_eq!(added, 1);
        v.remove_source("one.md").unwrap();
        assert_eq!(hit_sources(&v, "identical content here"), vec!["two.md"]);
    }

    #[test]
    fn ingest_is_differential_and_prunes() {
        let dir = std::env::temp_dir().join(format!("dct-ingest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.md"),
            "zedran relay calibration notes for the distillery",
        )
        .unwrap();
        std::fs::write(dir.join("nope.iso"), vec![0u8; 32]).unwrap();
        let v = tmpdb("ingest");
        let r = v.ingest_folder(&dir).unwrap();
        assert_eq!(r.files, 2);
        assert!(r.added > 0);
        assert_eq!(r.unsupported, 1);
        let r = v.ingest_folder(&dir).unwrap();
        assert_eq!(r.added, 0, "unchanged folder must add nothing");
        assert_eq!(r.indexed, 0, "and nothing counts as re-indexed");

        // Single-file mode: re-rag the same file is a no-op too.
        let r = v.ingest_file(&dir.join("a.md")).unwrap();
        assert_eq!(r.added, 0);

        // Changed file: re-chunked, old wording no longer searchable.
        std::fs::write(
            dir.join("a.md"),
            "updated: heliostat bearing maintenance log",
        )
        .unwrap();
        let r = v.ingest_folder(&dir).unwrap();
        assert!(r.added > 0);
        assert!(hit_sources(&v, "zedran relay").is_empty());

        // Deleted file: pruned from the store.
        std::fs::remove_file(dir.join("a.md")).unwrap();
        let _ = v.ingest_folder(&dir).unwrap();
        assert!(v.sources_with_kind("docs").unwrap().is_empty());
        assert!(hit_sources(&v, "heliostat bearing").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rich_documents_index_and_recall() {
        // A folder of "normal people" documents — Word + EPUB — must
        // land in the store and be recallable by content.
        let dir = std::env::temp_dir().join(format!("dct-rich-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            use std::io::Write;
            let mut buf = Vec::new();
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            w.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            w.write_all(br#"<w:document><w:body><w:p><w:r><w:t>The heliostat invoice totals 4400 ducats</w:t></w:r></w:p></w:body></w:document>"#)
                .unwrap();
            w.finish().unwrap();
            std::fs::write(dir.join("invoice.docx"), buf).unwrap();
        }
        {
            use std::io::Write;
            let mut buf = Vec::new();
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            w.start_file("mimetype", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"application/epub+zip").unwrap();
            w.start_file("c1.xhtml", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(
                br#"<html><body><p>The duchess keeps zigfruit in the cellar</p></body></html>"#,
            )
            .unwrap();
            w.finish().unwrap();
            std::fs::write(dir.join("book.epub"), buf).unwrap();
        }

        let v = tmpdb("rich");
        let r = v.ingest_folder(&dir).unwrap();
        assert_eq!(r.unsupported, 0);
        assert!(r.added > 0, "both documents must chunk in");
        let hits = v.search("who keeps the zigfruit", 5).unwrap();
        assert!(hits[0].source.ends_with("book.epub"));
        let hits = v.search("heliostat invoice total", 5).unwrap();
        assert!(hits[0].source.ends_with("invoice.docx"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_global_unique_chash_migrates() {
        let dir = std::env::temp_dir().join(format!("dct-mig-{}", std::process::id()));
        let db = dir.join("legacy.db");
        std::fs::create_dir_all(&dir).unwrap();
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS chunks(
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     source TEXT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL,
                     chash INTEGER NOT NULL UNIQUE, embedding BLOB NOT NULL,
                     created_at TEXT NOT NULL
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks(source, kind, content, chash, embedding, created_at)
                 VALUES ('old.md', 'docs', 'rugged memory core', 42, X'00000000', '2026-01-01')",
                [],
            )
            .unwrap();
        }
        // Open with the new schema: migration must keep the row and move
        // the uniqueness to (source, chash).
        let v = Vectors::open(&db).unwrap();
        assert_eq!(v.count().unwrap(), 1);
        let ddl: String = v
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !ddl.contains("UNIQUE"),
            "global chash UNIQUE dropped: {ddl}"
        );
        // Same text under a new source now stores its own row.
        v.insert_text("new.md", "docs", "rugged memory core")
            .unwrap();
        assert_eq!(v.count().unwrap(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
