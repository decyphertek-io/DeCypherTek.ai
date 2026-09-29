//! Research profiles — the YAML site lists `@research <name>.yml` runs.
//!
//! A profile is a tiny YAML file living in the wiki's `research/` folder
//! (so it is sealed into the vault with everything else and readable via
//! `@wiki read research/<name>.yml`):
//!
//! ```yaml
//! # research profile
//! name: rag-chat
//! description: RAG + chat research sources
//! sites:
//!   - https://arxiv.org
//!   - https://news.ycombinator.com
//! ```
//!
//! Parsing is a deliberate minimal subset (top-level `key: value` pairs
//! plus one `sites:` bullet list) — no new dependency, no arbitrary YAML.
//! Files are written by the same serializer, so round-trips are exact.

use crate::paths::Paths;
use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchProfile {
    /// File name including the .yml suffix, e.g. "rag-chat.yml".
    pub file: String,
    /// Display name from the yaml (defaults to the file stem).
    pub name: String,
    #[allow(dead_code)]
    pub description: String,
    /// Sites the research run is locked to (host or URL forms).
    pub sites: Vec<String>,
}

/// Allowed suffixes for profile files.
pub fn is_profile_name(raw: &str) -> bool {
    let lower = raw.trim().to_lowercase();
    (lower.ends_with(".yml") || lower.ends_with(".yaml"))
        && !lower.starts_with('.')
        && lower.len() <= 80
        && lower
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !lower.contains('/')
}

/// Normalize a typed name to a `.yml` file name (accepts it with or
/// without the suffix).
pub fn normalize_name(raw: &str) -> Result<String> {
    let lower = raw.trim().to_lowercase();
    if lower.is_empty() {
        return Err(anyhow!("profile name cannot be empty"));
    }
    let name = if is_profile_name(&lower) {
        lower
    } else {
        let candidate = format!("{lower}.yml");
        if !is_profile_name(&candidate) {
            return Err(anyhow!(
                "invalid profile name: {raw:?} (use letters, digits, -, _)"
            ));
        }
        candidate
    };
    Ok(name)
}

/// The wiki folder research profiles live in (see src/wiki.rs folders).
fn research_dir(p: &Paths) -> std::path::PathBuf {
    p.wiki_dir.join("research")
}

/// List saved research profiles (file names, e.g. "rag-chat.yml").
pub fn list(p: &Paths) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let dir = research_dir(p);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(out),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_profile_name(&name) {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// Load one profile by typed name (with or without the .yml suffix).
pub fn load(p: &Paths, raw_name: &str) -> Result<ResearchProfile> {
    let file = normalize_name(raw_name)?;
    let path = research_dir(p).join(&file);
    let body = std::fs::read_to_string(&path).map_err(|_| {
        anyhow!("research profile '{raw_name}' not found — @wiki list shows what exists, @setup creates new ones")
    })?;
    let profile = parse(&body).with_context(|| format!("parsing research profile {file}"))?;
    Ok(ResearchProfile {
        file,
        name: if profile.0.is_empty() {
            file.trim_end_matches(".yml").to_string()
        } else {
            profile.0
        },
        description: profile.1,
        sites: profile.2,
    })
}

/// Save a profile into the wiki research folder (creates the folder).
pub fn save(p: &Paths, file: &str, name: &str, description: &str, sites: &[String]) -> Result<()> {
    let file = normalize_name(file)?;
    if sites.is_empty() {
        return Err(anyhow!("a research profile needs at least one site"));
    }
    std::fs::create_dir_all(research_dir(p))?;
    std::fs::write(research_dir(p).join(&file), serialize(name, description, sites))?;
    Ok(())
}

/// Serialize in the exact subset `parse` reads.
pub fn serialize(name: &str, description: &str, sites: &[String]) -> String {
    let mut out =
        String::from("# DeCypherTek research profile — used by @research <name>.yml <topic>\n");
    out.push_str(&format!("name: {}\n", name.trim()));
    if !description.trim().is_empty() {
        out.push_str(&format!("description: {}\n", description.trim()));
    }
    out.push_str("sites:\n");
    for s in sites {
        out.push_str(&format!("  - {}\n", s.trim()));
    }
    out
}

/// Parse the subset: `key: value` lines, one `sites:` list of `- item`
/// bullets, `#` comments, blank lines. Returns (name, description, sites).
pub fn parse(body: &str) -> Result<(String, String, Vec<String>)> {
    let mut name = String::new();
    let mut description = String::new();
    let mut sites: Vec<String> = Vec::new();
    let mut in_sites = false;
    for (i, raw) in body.lines().enumerate() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("- ") {
            if !in_sites {
                return Err(anyhow!("line {}: list item outside 'sites:'", i + 1));
            }
            let site = trimmed[2..].trim().trim_matches('"').trim_matches('\'');
            if !site.is_empty() {
                sites.push(site.to_string());
            }
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            return Err(anyhow!("line {}: expected 'key: value'", i + 1));
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
        match key {
            "name" => name = value,
            "description" => description = value,
            "sites" => {
                in_sites = true;
                if !value.is_empty() {
                    // single-line form: sites: [a, b] or sites: a
                    let list = value
                        .trim_start_matches('[')
                        .trim_end_matches(']')
                        .split(',')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    sites.extend(list);
                }
            }
            other => return Err(anyhow!("line {}: unknown key {other:?}", i + 1)),
        }
    }
    if sites.is_empty() {
        return Err(anyhow!("no sites listed — add 'sites:' with '- url' bullets"));
    }
    Ok((name, description, sites))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let y = serialize(
            "rag-chat",
            "RAG sources",
            &["arxiv.org".into(), "https://hn.com".into()],
        );
        let (name, desc, sites) = parse(&y).unwrap();
        assert_eq!(name, "rag-chat");
        assert_eq!(desc, "RAG sources");
        assert_eq!(
            sites,
            vec!["arxiv.org".to_string(), "https://hn.com".to_string()]
        );
    }

    #[test]
    fn parses_comments_and_bullets() {
        let y = "# profile\nname: ops\n\ndescription: daily ops reading\nsites:\n  - https://arxiv.org\n  - news.ycombinator.com\n";
        let (name, desc, sites) = parse(y).unwrap();
        assert_eq!(name, "ops");
        assert_eq!(desc, "daily ops reading");
        assert_eq!(sites.len(), 2);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("name: x\nsites:").is_err());
        assert!(parse("sites:\n  - a\nrandom_key: 1").is_err());
        assert!(parse("- orphan").is_err());
    }

    #[test]
    fn names_normalize() {
        assert_eq!(normalize_name("Rag-Chat").unwrap(), "rag-chat.yml");
        assert_eq!(normalize_name("rag-chat.yaml").unwrap(), "rag-chat.yaml");
        assert!(normalize_name("../evil").is_err());
        assert!(normalize_name("").is_err());
    }
}
