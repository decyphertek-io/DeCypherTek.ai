use crate::paths::Paths;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    OpenRouter,
    Ollama,
}
/// One registered MCP tool server (installed from @store). The agent runs
/// these in hardened Docker containers that talk ONLY to the agent over
/// stdio — no network, no capabilities (see src/store.rs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServer {
    /// Short handle, e.g. "github" — tools surface as `mcp_<name>_<tool>`.
    pub name: String,
    /// Docker image, e.g. "ghcr.io/github/github-mcp-server".
    pub image: String,
    pub enabled: bool,
    /// ISO timestamp of when it was added from @store (cosmetic).
    #[serde(default)]
    pub added: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub persona: String,
    pub provider: String, // "openrouter" | "ollama"
    pub openrouter_api_key: String,
    pub openrouter_model: String,
    pub ollama_url: String,
    pub ollama_model: String,
    pub leash: String, // "leashed" | "unleashed"
    /// Folders the agent may read from (granted read permission).
    pub read_paths: Vec<String>,
    /// Folders the agent may write to beyond its own data dir.
    pub write_paths: Vec<String>,
    /// Folders whose docs were chunked into the RAG vector store.
    pub rag_folders: Vec<String>,
    /// Tool gates. Memory/wiki tools are always allowed.
    pub tool_web_search: bool,
    pub tool_read_files: bool,
    pub tool_write_files: bool,
    pub tool_run_command: bool,
    /// MCP tool servers (from @store): master gate. Servers themselves are
    /// registered in `mcp_servers`; launching also requires Docker.
    #[serde(default = "default_true")]
    pub tool_mcp: bool,
    /// MCP servers registered through @store; stored in the vault with
    /// everything else. Old configs without the field load as empty.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: 1,
            persona: "default".into(),
            provider: "openrouter".into(),
            openrouter_api_key: String::new(),
            openrouter_model: "openai/gpt-4o-mini".into(),
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_model: "qwen2.5:0.5b-instruct".into(),
            leash: "leashed".into(),
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            rag_folders: Vec::new(),
            tool_web_search: true,
            tool_read_files: true,
            tool_write_files: false,
            tool_run_command: false,
            tool_mcp: true,
            mcp_servers: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(p: &Paths) -> Result<Config> {
        let raw = std::fs::read_to_string(&p.config_file)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, p: &Paths) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&p.config_file, json)?;
        Ok(())
    }

    pub fn is_unleashed(&self) -> bool {
        self.leash == "unleashed"
    }

    pub fn provider(&self) -> Provider {
        if self.provider == "ollama" {
            Provider::Ollama
        } else {
            Provider::OpenRouter
        }
    }

    /// Human summary for @status and the welcome banner.
    pub fn backend_summary(&self) -> String {
        match self.provider() {
            Provider::OpenRouter => format!("openrouter:{}", self.openrouter_model),
            Provider::Ollama => format!("ollama:{}", self.ollama_model),
        }
    }
}
