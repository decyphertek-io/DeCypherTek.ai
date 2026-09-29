//! Model layer — exactly ONE thin OpenAI-style HTTP client covers both
//! supported backends: OpenRouter (default, one key for every hosted
//! model) and Ollama (optional local models). Swap the base URL and the
//! same binary runs cloud or local.

use crate::config::Provider;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The curated OpenRouter picks offered at setup: strong current
/// models across providers — the default is the first entry.
pub const OPENROUTER_DEFAULT_MODELS: &[&str] = &[
    "z-ai/glm-latest",
    "moonshotai/kimi-k3",
    "deepseek/deepseek-v4.1-flash",
];

/// Slim models that actually run on a phone via Ollama in Termux.
pub const OLLAMA_SLIM_MODELS: &[&str] = &["qwen2.5:0.5b-instruct", "llama3.2:1b", "smollm2:360m"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnCall {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tc {
    pub id: String,
    #[serde(rename = "type")]
    pub typ: String,
    pub function: FnCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<Tc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub typ: &'static str,
    pub function: ToolFn,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFn {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl Msg {
    pub fn system(text: &str) -> Msg {
        Msg {
            role: "system".into(),
            content: Some(text.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    pub fn user(text: &str) -> Msg {
        Msg {
            role: "user".into(),
            content: Some(text.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    pub fn assistant(content: Option<String>, tool_calls: Option<Vec<Tc>>) -> Msg {
        Msg {
            role: "assistant".into(),
            content,
            tool_calls,
            tool_call_id: None,
        }
    }
    pub fn tool(id: String, text: String) -> Msg {
        Msg {
            role: "tool".into(),
            content: Some(text),
            tool_calls: None,
            tool_call_id: Some(id),
        }
    }
}

pub struct Client {
    http: ureq::Agent,
    base: String,
    key: Option<String>,
    pub model: String,
}

impl Client {
    pub fn from_config(
        provider: crate::config::Provider,
        or_key: &str,
        or_model: &str,
        ol_url: &str,
        ol_model: &str,
    ) -> Result<Client> {
        let http = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout(Duration::from_secs(180))
            .build();
        match provider {
            Provider::OpenRouter => {
                if or_key.trim().is_empty() {
                    return Err(anyhow!(
                        "no OpenRouter API key — run `decyphertek.ai setup` (or @setup) and add one"
                    ));
                }
                Ok(Client {
                    http,
                    base: "https://openrouter.ai/api/v1".into(),
                    key: Some(or_key.to_string()),
                    model: or_model.to_string(),
                })
            }
            Provider::Ollama => {
                let base = ol_url.trim_end_matches('/').to_string() + "/v1";
                Ok(Client {
                    http,
                    base,
                    key: None,
                    model: ol_model.to_string(),
                })
            }
        }
    }

    /// One chat-completions round trip. Returns (content, tool_calls).
    pub fn chat(
        &self,
        messages: &[Msg],
        tools: Option<&[ToolSpec]>,
    ) -> Result<(Option<String>, Vec<Tc>)> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            messages: &'a [Msg],
            stream: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            tools: Option<&'a [ToolSpec]>,
        }
        let req = Req {
            model: &self.model,
            messages,
            stream: false,
            tools,
        };

        let url = format!("{}/chat/completions", self.base);
        let body = serde_json::to_value(&req)?;
        let mut last_err: Option<String> = None;

        for attempt in 0..3 {
            let mut call = self.http.post(&url).set("Content-Type", "application/json");
            if let Some(k) = &self.key {
                call = call
                    .set("Authorization", &format!("Bearer {k}"))
                    .set("HTTP-Referer", "https://decyphertek.ai")
                    .set("X-Title", "DeCypherTek.ai");
            }
            let resp = call.send_json(body.clone());
            match resp {
                Ok(r) => {
                    let parsed: ChatResp = serde_json::from_reader(r.into_reader())
                        .context("model returned unparseable JSON")?;
                    let msg = parsed
                        .choices
                        .first()
                        .ok_or_else(|| anyhow!("model returned no choices"))?;
                    return Ok((
                        msg.message.content.clone(),
                        msg.message.tool_calls.clone().unwrap_or_default(),
                    ));
                }
                Err(ureq::Error::Status(code, r)) => {
                    let text = r.into_string().unwrap_or_default();
                    let short = if text.len() > 300 {
                        format!("{}…", &text[..300])
                    } else {
                        text
                    };
                    // Retry transient failures only.
                    if (code == 429 || code >= 500) && attempt < 2 {
                        std::thread::sleep(Duration::from_secs(2u64 * (attempt as u64 + 1)));
                        last_err = Some(format!("HTTP {code}: {short}"));
                        continue;
                    }
                    return Err(anyhow!("model request failed (HTTP {code}): {short}"));
                }
                Err(e) => {
                    // Network error: one retry, then fail with hint.
                    if attempt < 2 {
                        last_err = Some(e.to_string());
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                    return Err(anyhow!("cannot reach model backend: {e} — check your connection (OpenRouter) or that Ollama is running (ollama serve)"));
                }
            }
        }
        Err(anyhow!(
            "model request failed after retries: {}",
            last_err.unwrap_or_default()
        ))
    }

    /// Reachability probe used by setup and diagnostics.
    pub fn probe(&self) -> Result<()> {
        let url = format!("{}/models", self.base);
        let mut call = self.http.get(&url).timeout(Duration::from_secs(10));
        if let Some(k) = &self.key {
            call = call.set("Authorization", &format!("Bearer {k}"));
        }
        match call.call() {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, _)) => {
                Err(anyhow!("backend reachable, but returned HTTP {code}"))
            }
            Err(e) => Err(anyhow!("backend unreachable: {e}")),
        }
    }

    /// List local Ollama models (GET /api/tags), used at setup.
    pub fn ollama_tags(base_url: &str) -> Result<Vec<String>> {
        let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
        let resp = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(8))
            .build()
            .get(&url)
            .call()
            .map_err(|e| anyhow!("Ollama at {base_url} is not responding: {e}"))?;
        #[derive(Deserialize)]
        struct Tags {
            #[serde(default)]
            models: Vec<Named>,
        }
        #[derive(Deserialize)]
        struct Named {
            name: String,
        }
        let parsed: Tags = serde_json::from_reader(resp.into_reader())?;
        Ok(parsed.models.into_iter().map(|m| m.name).collect())
    }
}

#[derive(Debug, Deserialize)]
struct ChatResp {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: RespMsg,
}

#[derive(Debug, Deserialize)]
struct RespMsg {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<Tc>>,
}
