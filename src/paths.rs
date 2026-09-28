use std::path::{Path, PathBuf};

/// All on-disk locations. The root lives in the user's home directory,
/// outside the binary, so the binary can be replaced without losing the agent.
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub vault_file: PathBuf,
    pub staging: PathBuf,
    pub config_file: PathBuf,
    pub memory_dir: PathBuf,
    pub vector_db: PathBuf,
    pub wiki_dir: PathBuf,
    pub chatlog_dir: PathBuf,
    pub archives_dir: PathBuf,
}

impl Paths {
    pub fn new(root: &Path) -> Self {
        let staging = root.join("staging");
        Paths {
            root: root.to_path_buf(),
            vault_file: root.join("vault.dct"),
            staging: staging.clone(),
            config_file: staging.join("config.json"),
            memory_dir: staging.join("memory"),
            vector_db: staging.join("memory").join("vectors.db"),
            wiki_dir: staging.join("memory").join("wiki"),
            chatlog_dir: staging.join("chatlogs"),
            archives_dir: staging.join("archives"),
        }
    }

    pub fn from_home() -> anyhow::Result<Self> {
        let data_root = home_dir()?.join(".decyphertek.ai");
        Ok(Paths::new(&data_root))
    }

    /// Ensure every runtime subdirectory exists (called after unseal).
    pub fn ensure_staging(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.staging)?;
        std::fs::create_dir_all(&self.memory_dir)?;
        std::fs::create_dir_all(&self.wiki_dir)?;
        std::fs::create_dir_all(&self.chatlog_dir)?;
        std::fs::create_dir_all(&self.archives_dir)?;
        set_private(&self.staging);
        Ok(())
    }
}

pub fn home_dir() -> anyhow::Result<PathBuf> {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return Ok(PathBuf::from(h));
        }
    }
    Err(anyhow::anyhow!("HOME is not set; cannot locate ~"))
}

/// 0700 on the staging dir so decrypted data is user-only.
fn set_private(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(md) = std::fs::metadata(p) {
            let mut perm = md.permissions();
            perm.set_mode(0o700);
            let _ = std::fs::set_permissions(p, perm);
        }
    }
}
