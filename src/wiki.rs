//! Wiki Memory — the agent's markdown knowledge base.
//!
//! A curated baseline ships inside the binary (see assets/wiki/) and is
//! written out at setup; everything the agent learns is written back here
//! by the `wiki_write` tool. Names are sanitized to prevent traversal —
//! the agent can only write inside its own wiki dir.

use crate::paths::Paths;
use anyhow::{anyhow, Result};

pub const BASELINE_FILES: &[(&str, &str)] = &[
    (
        "operative-handbook.md",
        include_str!("../assets/wiki/operative-handbook.md"),
    ),
    ("personas.md", include_str!("../assets/wiki/personas.md")),
    (
        "termux-playbook.md",
        include_str!("../assets/wiki/termux-playbook.md"),
    ),
    (
        "secure-vault.md",
        include_str!("../assets/wiki/secure-vault.md"),
    ),
    ("commands.md", include_str!("../assets/wiki/commands.md")),
    (
        "rag-and-memory.md",
        include_str!("../assets/wiki/rag-and-memory.md"),
    ),
];

/// Write the shipped baseline docs into the vault's wiki dir (idempotent).
pub fn write_baseline(p: &Paths) -> Result<()> {
    std::fs::create_dir_all(&p.wiki_dir)?;
    for (name, body) in BASELINE_FILES {
        let path = p.wiki_dir.join(name);
        if !path.exists() {
            std::fs::write(path, body)?;
        }
    }
    Ok(())
}

/// Only [a-z0-9-_].md — no slashes, no dots-in-the-middle tricks.
pub fn sanitize_name(raw: &str) -> Result<String> {
    let lower = raw.trim().to_lowercase();
    let name = if lower.ends_with(".md") {
        lower
    } else {
        format!("{lower}.md")
    };
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    if !ok || name.len() > 80 {
        return Err(anyhow!(
            "invalid wiki name: {raw:?} (use letters, digits, -, _)"
        ));
    }
    Ok(name)
}

pub fn list(p: &Paths) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&p.wiki_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".md") {
            out.push(name.trim_end_matches(".md").to_string());
        }
    }
    out.sort();
    Ok(out)
}

pub fn read(p: &Paths, raw_name: &str) -> Result<String> {
    let name = sanitize_name(raw_name)?;
    let path = p.wiki_dir.join(&name);
    let body =
        std::fs::read_to_string(&path).map_err(|_| anyhow!("wiki page '{raw_name}' not found"))?;
    Ok(body)
}

pub fn write(p: &Paths, raw_name: &str, content: &str) -> Result<String> {
    let name = sanitize_name(raw_name)?;
    std::fs::write(p.wiki_dir.join(&name), content)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("Notes").unwrap(), "notes.md");
        assert!(sanitize_name("../../etc/passwd").is_err());
        assert!(sanitize_name("..hidden").is_err());
        assert!(sanitize_name("ok-page_v1").unwrap().ends_with(".md"));
    }
}
