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
    (
        "adminotaur.md",
        include_str!("../assets/wiki/adminotaur.md"),
    ),
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

/// The wiki's fixed subfolders. `research/` holds the YAML site lists
/// `@research <name>.yml` runs against (see src/research.rs); `info/`
/// holds the docs you upload with @upload. Both seal into the vault.
pub const FOLDERS: &[&str] = &["research", "info"];

/// Baseline research profiles shipped inside the binary: the example
/// `@research rag-chat.yml` runs against out of the box, plus the
/// hardcoded `sources.yml` — public archival research databases
/// (Internet Archive, NARA, Federal Register, Congress.gov, GovInfo,
/// FRUS, CIA Reading Room, UK National Archives, arXiv, Crossref,
/// Black Vault, …) so `@research sources.yml <topic>` works on day one.
pub const BASELINE_RESEARCH_PROFILES: &[(&str, &str)] = &[
    (
        "rag-chat.yml",
        include_str!("../assets/wiki/research/rag-chat.yml"),
    ),
    (
        "sources.yml",
        include_str!("../assets/wiki/research/sources.yml"),
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
    for folder in FOLDERS {
        std::fs::create_dir_all(p.wiki_dir.join(folder))?;
    }
    let research = p.wiki_dir.join("research");
    for (file, body) in BASELINE_RESEARCH_PROFILES {
        let profile = research.join(file);
        if !profile.exists() {
            std::fs::write(profile, body)?;
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
    // Subfolder entries show as "folder/file" (research profiles, info
    // uploads) — sorted after the root pages.
    let mut sub = Vec::new();
    for folder in FOLDERS {
        if let Ok(entries) = std::fs::read_dir(p.wiki_dir.join(folder)) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    sub.push(format!("{folder}/{name}"));
                }
            }
        }
    }
    sub.sort();
    out.extend(sub);
    Ok(out)
}

/// Sanitize a file name inside one of the fixed folders (any extension).
fn sanitize_folder_file(raw: &str) -> Result<String> {
    let lower = raw.trim().to_lowercase().replace(' ', "-");
    let ok = !lower.is_empty()
        && !lower.starts_with('.')
        && !lower.contains('/')
        && lower
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    if !ok || lower.len() > 100 {
        return Err(anyhow!("invalid file name: {raw:?}"));
    }
    Ok(lower)
}

/// Parse "folder/name" into (folder, file); plain names are root pages.
fn split_folder_entry(raw: &str) -> Option<(String, String)> {
    let (folder, name) = raw.split_once('/')?;
    if FOLDERS.contains(&folder) {
        Some((folder.to_string(), name.to_string()))
    } else {
        None
    }
}

/// Sanitized destination path for an uploaded doc inside info/.
pub fn info_file_path(p: &Paths, raw_name: &str) -> Result<std::path::PathBuf> {
    let name = sanitize_folder_file(raw_name)?;
    Ok(p.wiki_dir.join("info").join(name))
}

pub fn read(p: &Paths, raw_name: &str) -> Result<String> {
    if let Some((folder, file)) = split_folder_entry(raw_name) {
        let file = sanitize_folder_file(&file)?;
        let path = p.wiki_dir.join(&folder).join(&file);
        let body = std::fs::read_to_string(&path)
            .map_err(|_| anyhow!("wiki entry '{raw_name}' not found"))?;
        return Ok(body);
    }
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

    #[test]
    fn folder_entries_parse() {
        assert_eq!(
            split_folder_entry("research/rag-chat.yml"),
            Some(("research".into(), "rag-chat.yml".into()))
        );
        assert_eq!(
            split_folder_entry("info/manual.pdf"),
            Some(("info".into(), "manual.pdf".into()))
        );
        assert_eq!(split_folder_entry("evil/root.md"), None);
        assert_eq!(split_folder_entry("plain-page"), None);
        assert!(sanitize_folder_file("My Doc.PDF").unwrap() == "my-doc.pdf");
        assert!(sanitize_folder_file("../x").is_err());
    }

    #[test]
    fn baseline_ships_every_research_profile_idempotently() {
        let root = std::env::temp_dir().join(format!("dct-wiki-baseline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths::new(&root);
        write_baseline(&paths).unwrap();
        for (file, body) in BASELINE_RESEARCH_PROFILES {
            let path = paths.wiki_dir.join("research").join(file);
            let written = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("{file} missing after write_baseline"));
            assert_eq!(written, *body);
        }
        // second run must not touch what already exists (a user may edit)
        let suspect = paths.wiki_dir.join("research").join("sources.yml");
        std::fs::write(
            &suspect,
            "name: sources\ndescription: edited\nsites:\n  - https://archive.org\n",
        )
        .unwrap();
        write_baseline(&paths).unwrap();
        assert!(
            std::fs::read_to_string(&suspect)
                .unwrap()
                .contains("edited"),
            "write_baseline must never overwrite an existing profile"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
