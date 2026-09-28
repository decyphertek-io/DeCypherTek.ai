//! RAG vector store — one SQLite file, zero external services.
//!
//! Docs are chunked, each chunk gets a deterministic 256-dim feature-hashing
//! embedding (FNV-1a of word unigrams + bigrams, signed, L2-normalized), and
//! search is cosine similarity computed in Rust. Tiny corpora (phone scale)
//! scan in microseconds, the whole "vector DB" is one file in the vault,
//! and it works fully offline with no model download. When Ollama is
//! configured you can swap the embedder for `nomic-embed-text` later —
//! same schema, same cosine search.

use crate::util::{fnv1a32, fnv1a64, now_iso};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

pub const DIMS: usize = 256;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024; // skip huge/binary files
const TARGET_CHUNK: usize = 900;
const HARD_CHUNK: usize = 1200;

const INGESTIBLE_EXT: &[&str] = &[
    "md", "txt", "log", "json", "csv", "rs", "py", "js", "ts", "go", "c", "h", "cpp", "sh", "yaml",
    "yml", "toml", "html", "css",
];

pub struct Vectors {
    conn: Connection,
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
        let conn = Connection::open(db_path).context("open vector db")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chunks(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source TEXT NOT NULL,
                kind TEXT NOT NULL,
                content TEXT NOT NULL,
                chash INTEGER NOT NULL UNIQUE,
                embedding BLOB NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_kind ON chunks(kind);",
        )?;
        Ok(Vectors { conn })
    }

    pub fn insert_chunk(&self, source: &str, kind: &str, content: &str) -> Result<bool> {
        let hash = fnv1a64(content.as_bytes()) as i64;
        let emb = embed(content);
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO chunks(source, kind, content, chash, embedding, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![source, kind, content, hash, emb_to_blob(&emb), now_iso()],
        )? > 0;
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

    pub fn count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?)
    }

    pub fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>> {
        let q = embed(query);
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
            let score = cosine(&q, &blob_to_emb(&blob));
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

    /// Recursively chunk every readable text file in a folder into the store.
    /// Returns (files_seen, chunks_added).
    pub fn ingest_folder(&self, folder: &Path) -> Result<(usize, usize)> {
        let mut files = 0usize;
        let mut added = 0usize;
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
                } else if is_ingestible(&path) {
                    files += 1;
                    if let Ok(meta) = std::fs::metadata(&path) {
                        if meta.len() > MAX_FILE_BYTES {
                            continue;
                        }
                    }
                    if let Ok(text) = read_text_lossy(&path) {
                        let (_, a) = self.insert_text(&path.to_string_lossy(), "docs", &text)?;
                        added += a;
                    }
                }
            }
        }
        Ok((files, added))
    }
}

fn is_ingestible(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| INGESTIBLE_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn read_text_lossy(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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

    #[test]
    fn embedding_is_deterministic_and_normalized() {
        let a = embed("rust cargo build");
        let b = embed("rust cargo build");
        assert_eq!(a, b);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    #[test]
    fn similar_texts_score_higher() {
        let v = tmpdb("sim");
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
        let hits = v.search("how do I build a crate with cargo", 2).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].source, "a.md");
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
    fn dedupe_on_same_content() {
        let v = tmpdb("dedupe");
        v.insert_text("one.md", "docs", "identical content here")
            .unwrap();
        let (total, added) = v
            .insert_text("two.md", "docs", "identical content here")
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(added, 0, "same-content chunk must be deduped");
    }
}
