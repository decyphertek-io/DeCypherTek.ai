//! Forensic chat logs — every run leaves a case file.
//!
//! Each agent run writes a JSONL: every prompt, every model reply, every
//! tool call with arguments, every result, the final report. Old logs are
//! chunked into the vector store as `chatlog` knowledge, and after a month
//! a log rotates into a month tar.gz under archives/.

use crate::paths::Paths;
use crate::util::{now_iso, unix_ts};
use anyhow::Result;
use serde_json::json;
use std::io::Write;

pub struct ChatLog {
    path: std::path::PathBuf,
    handle: Option<std::fs::File>,
}

impl ChatLog {
    pub fn new(p: &Paths) -> Result<ChatLog> {
        std::fs::create_dir_all(&p.chatlog_dir)?;
        let path = p.chatlog_dir.join(format!("run-{}.jsonl", unix_ts()));
        let handle = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(ChatLog {
            path,
            handle: Some(handle),
        })
    }

    pub fn log(&mut self, kind: &str, text: &str) {
        let line = json!({ "ts": now_iso(), "kind": kind, "text": text }).to_string();
        if let Some(h) = self.handle.as_mut() {
            let _ = writeln!(h, "{line}");
        }
    }

    pub fn read_all(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }
}

/// Rotate logs older than 30 days into archives/chatlogs-YYYYMM.tar.gz
/// and remove them from chatlogs/. Cheap, runs once per launch.
pub fn rotate_old_logs(p: &Paths) -> Result<(usize, String)> {
    let cutoff = unix_ts() - 30 * 24 * 3600;
    let mut archived = 0usize;
    std::fs::create_dir_all(&p.archives_dir)?;

    let mut rotated: Vec<std::path::PathBuf> = Vec::new();
    let entries = std::fs::read_dir(&p.chatlog_dir)?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let ts = name
            .trim_start_matches("run-")
            .trim_end_matches(".jsonl")
            .parse::<i64>()
            .unwrap_or(i64::MAX);
        if ts < cutoff {
            rotated.push(entry.path());
        }
    }

    if !rotated.is_empty() {
        // Never truncate an existing archive — pick a fresh filename.
        let month = chrono::Local::now().format("%Y%m").to_string();
        let mut n = 1;
        let archive_path = loop {
            let cand = p.archives_dir.join(format!("chatlogs-{month}-{n}.tar.gz"));
            if !cand.exists() {
                break cand;
            }
            n += 1;
        };
        let out = std::fs::File::create(&archive_path)?;
        let enc = flate2::write::GzEncoder::new(out, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        for path in &rotated {
            if let Some(fname) = path.file_name() {
                let _ = tar.append_path_with_name(path, fname);
            }
            archived += 1;
        }
        tar.finish()?;
        let enc = tar.into_inner()?;
        enc.finish()?;
        for path in &rotated {
            let _ = std::fs::remove_file(path);
        }
        return Ok((archived, archive_path.to_string_lossy().to_string()));
    }
    Ok((0, String::new()))
}
