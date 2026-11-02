//! Wiki Memory — the agent's markdown knowledge base.
//!
//! A curated baseline ships inside the binary (see assets/wiki/) and is
//! written out at setup; everything the agent learns is written back here
//! by the `wiki_write` tool. Names are sanitized to prevent traversal —
//! the agent can only write inside its own wiki dir.
//!
//! Every page is also a vector-memory document: `sync_index` differentially
//! chunks the whole wiki into the RAG store (atoms are `Vectors::sync_source`),
//! so one `memory_search` reaches wiki knowledge without list-then-read
//! token costs. `refresh_index_page` maintains `memory-index.md`, the
//! auto-generated directory that tells the agent where everything lives.

use crate::paths::Paths;
use crate::vector::{SyncOutcome, Vectors};
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
/// holds uploaded docs from earlier installs (the @upload command is
/// gone; /rag indexes documents in place). Both seal into the vault.
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
    // docs from earlier installs) — sorted after the root pages.
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

// ---------- differential vector indexing ----------

/// The auto-generated memory directory page — the map of where
/// knowledge lives. Regenerated by `refresh_index_page`.
pub const INDEX_PAGE: &str = "memory-index.md";

/// Vector source for a wiki file: `wiki/<name>`.
fn source_for(name: &str) -> String {
    format!("wiki/{name}")
}

/// Differential index of the whole wiki into vector memory:
/// new/edited pages are (re)chunked, vanished pages pruned. Returns
/// (pages_seen, pages_changed). Unchanged pages cost one hash compare.
pub fn sync_index(p: &Paths, vectors: &Vectors) -> Result<(usize, usize)> {
    let mut seen: Vec<String> = Vec::new();
    let mut changed = 0usize;
    if let Ok(entries) = std::fs::read_dir(&p.wiki_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".md") || name.starts_with('.') {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                let outcome = vectors.sync_source(&source_for(&name), "wiki", &text)?;
                if let SyncOutcome::Changed { .. } = outcome {
                    changed += 1;
                }
                seen.push(source_for(&name));
            }
        }
    }
    // Prune indexed pages that no longer exist on disk.
    for source in vectors.sources_with_kind("wiki")? {
        if !seen.contains(&source) {
            vectors.remove_source(&source)?;
        }
    }
    // Same for uploaded docs that were deleted by hand: their chunks
    // would otherwise outlive the file forever.
    for source in vectors.sources_with_kind("info-doc")? {
        let Some(file) = source.strip_prefix("info/") else {
            continue;
        };
        if !p.wiki_dir.join("info").join(file).exists() {
            vectors.remove_source(&source)?;
        }
    }
    vectors.ensure_index();
    Ok((seen.len(), changed))
}

/// Re-index one page right after it was written, so wiki knowledge is
/// searchable immediately — not at next launch.
pub fn sync_page(p: &Paths, vectors: &Vectors, name: &str) -> Result<SyncOutcome> {
    let path = p.wiki_dir.join(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| anyhow!("wiki page '{name}' not found for indexing"))?;
    let outcome = vectors.sync_source(&source_for(name), "wiki", &text)?;
    vectors.ensure_index();
    Ok(outcome)
}

/// First `# Heading` of a page, else (name, first prose line).
fn page_outline(text: &str, name: &str) -> (String, String) {
    let clean = text.replace("\r\n", "\n");
    let mut title = String::new();
    let mut snippet = String::new();
    for line in clean.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(h) = t.strip_prefix('#') {
            if title.is_empty() {
                title = h.trim().to_string();
            }
        } else if snippet.is_empty() && !t.starts_with("```") {
            snippet = t.to_string();
        }
        if !title.is_empty() && !snippet.is_empty() {
            break;
        }
    }
    if title.is_empty() {
        title = name.to_string();
    }
    (title, snippet)
}

/// Rebuild `memory-index.md` — the memory directory — from the wiki
/// listing. Only rewrites (and re-indexes) when the content actually
/// changed. Returns true when the page changed.
pub fn refresh_index_page(p: &Paths, vectors: &Vectors) -> Result<bool> {
    let entries = list(p)?;
    let mut roots = Vec::new();
    let mut info = Vec::new();
    let mut research = Vec::new();
    for e in &entries {
        if let Some((folder, file)) = e.split_once('/') {
            match folder {
                "info" => info.push(file.to_string()),
                "research" => research.push(file.to_string()),
                _ => {}
            }
        } else if e != "memory-index" {
            roots.push(e.clone());
        }
    }

    let mut body = String::from(
        "# Memory Directory\n\n\
         Auto-generated map of this agent's knowledge. Open a page with \
         `read_wiki <name>`; search everything with `memory_search`.\n\n\
         ## Wiki pages\n\n",
    );
    if roots.is_empty() {
        body.push_str("_(no pages yet)_\n");
    }
    for name in &roots {
        let (title, snippet) = std::fs::read_to_string(p.wiki_dir.join(format!("{name}.md")))
            .map(|t| page_outline(&t, name))
            .unwrap_or_else(|_| (name.clone(), String::new()));
        let snippet = clip(&snippet, 110);
        if snippet.is_empty() {
            body.push_str(&format!("- **{name}** — {title}\n"));
        } else {
            body.push_str(&format!("- **{name}** — {title}: {snippet}\n"));
        }
    }
    if !info.is_empty() {
        body.push_str("\n## Uploaded docs (info/)\n\n");
        for f in &info {
            body.push_str(&format!(
                "- info/{f} — uploaded document, chunked into memory\n"
            ));
        }
    }
    if !research.is_empty() {
        body.push_str("\n## Research profiles (research/)\n\n");
        for f in &research {
            let stem = f.trim_end_matches(".yml").trim_end_matches(".yaml");
            body.push_str(&format!(
                "- research/{f} — site list for `/research {stem}.yml <topic>`\n"
            ));
        }
    }
    body.push_str(&format!(
        "\n_Generated {} by `refresh_index_page`. Edits are overwritten — edit the \
         pages themselves, and write new knowledge with `write_wiki`._\n",
        crate::util::now_iso()
    ));

    let path = p.wiki_dir.join(INDEX_PAGE);
    if let Ok(prev) = std::fs::read_to_string(&path) {
        if prev == body {
            sync_page(p, vectors, INDEX_PAGE)?; // keep any prior copy indexed
            return Ok(false);
        }
    }
    std::fs::write(&path, &body)?;
    sync_page(p, vectors, INDEX_PAGE)?;
    Ok(true)
}

/// One line, whitespace-collapsed, clipped at a char boundary.
fn clip(s: &str, max: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.len() <= max {
        return t;
    }
    let mut end = max;
    while end > 0 && !t.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &t[..end])
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
    fn wiki_index_is_differential_and_searchable() {
        let root = std::env::temp_dir().join(format!("dct-wiki-vec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths::new(&root);
        paths.ensure_staging().unwrap();
        write_baseline(&paths).unwrap();
        let vectors = Vectors::open(&paths.vector_db).unwrap();

        // First sync: every baseline page + the directory page chunks in.
        refresh_index_page(&paths, &vectors).unwrap();
        let (pages, changed) = sync_index(&paths, &vectors).unwrap();
        assert!(pages >= 5, "baseline pages indexed: {pages}");
        assert!(changed > 0);
        let hits = vectors
            .search("encrypted vault password argon2", 6)
            .unwrap();
        assert!(
            hits.iter().any(|h| h.kind == "wiki"),
            "wiki knowledge must be recallable: {hits:?}"
        );

        // Idempotent: nothing changed since the last sync.
        refresh_index_page(&paths, &vectors).unwrap();
        let (_, changed) = sync_index(&paths, &vectors).unwrap();
        assert_eq!(changed, 0);

        // A hand edit is picked up; a deleted page is pruned.
        std::fs::write(
            paths.wiki_dir.join("deploy-notes.md"),
            "# Deploy Notes\n\nThe zigfruit repeater lives on port 6060.",
        )
        .unwrap();
        refresh_index_page(&paths, &vectors).unwrap();
        sync_index(&paths, &vectors).unwrap();
        let hits = vectors.search("zigfruit repeater port", 5).unwrap();
        assert_eq!(hits[0].source, "wiki/deploy-notes.md");

        std::fs::write(
            paths.wiki_dir.join("deploy-notes.md"),
            "# Deploy Notes\n\nRewritten: moved to port 7070.",
        )
        .unwrap();
        std::fs::remove_file(paths.wiki_dir.join("commands.md")).unwrap();
        refresh_index_page(&paths, &vectors).unwrap();
        sync_index(&paths, &vectors).unwrap();
        assert!(
            vectors
                .search("zigfruit repeater port", 5)
                .unwrap()
                .iter()
                .all(|h| h.source != "wiki/deploy-notes.md")
                || vectors
                    .search("zigfruit repeater port", 5)
                    .unwrap()
                    .iter()
                    .any(|h| h.content.contains("7070")),
            "stale page text must be replaced"
        );
        assert!(
            vectors
                .sources_with_kind("wiki")
                .unwrap()
                .iter()
                .all(|s| s != "wiki/commands.md"),
            "deleted page must be pruned from the index"
        );

        // The memory directory itself stays accurate.
        let idx = std::fs::read_to_string(paths.wiki_dir.join(INDEX_PAGE)).unwrap();
        assert!(idx.contains("deploy-notes"));
        assert!(
            !idx.contains("- **commands**"),
            "directory drops removed pages"
        );
        let _ = std::fs::remove_dir_all(&root);
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
