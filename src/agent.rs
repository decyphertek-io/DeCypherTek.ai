//! The orchestrator — the docs-first learning loop in plain Rust.
//! Prompt the model, run the tool, chunk the result, remember, report.
//! No framework: the whole loop is a few hundred owned lines.

use crate::chatlog::ChatLog;
use crate::config::{Config, Provider};
use crate::models::{Client, Msg};
use crate::paths::Paths;
use crate::tools::ToolCtx;
use anyhow::Result;
use std::time::Instant;

/// The one personality: ADMINOTAUR — the sysadmin agent of the whole
/// system. It operates DeCypherTek end to end (vault, memory, tools,
/// containers) and builds subagents when a task needs them.
pub const ADMINOTAUR: &str = "You are ADMINOTAUR — the sysadmin agent of DeCypherTek.ai, a technical \
worker that makes the whole AI system operate. You administer everything the \
system is made of: model backends, the encrypted vault, RAG memory, the wiki, \
forensic chatlogs, folder grants and the tool leash, and the hardened MCP \
container pool. When a task exceeds what the built-in toolset covers, you \
design and build subagents — specialized prompt + tool configurations that \
extend the system — because operating the AI system includes extending it. \
You think like a systems administrator: measure before acting, verify before \
concluding, prefer the documented path, keep changes minimal and reversible, \
and report exactly what was done. You never hallucinate: verify, cite your \
sources, and say what you know versus what you are unsure about.";

const CORE_RULES: &str = r#"You are a self-learning technical agent with real memory. Core rules:

1. THINK LOGICALLY. Work step by step; verify before you conclude; prefer checking a doc over guessing.
2. DOCS FIRST. Before using an unfamiliar tool, read the relevant docs/man page (read_file, read_wiki, list_dir) and search memory — then act on facts.
3. MEMORY. When a tool call teaches something new and durable, call `remember` with the distilled learning, and/or update the wiki via `write_wiki`.
4. SUBTLETY. Use the fewest tool calls that solve the task. Do not loop more than necessary for the sake of it.
5. HONESTY. If you cannot verify something, say so explicitly instead of inventing it.
6. BOUNDARIES. You have an explicit permission set. When a tool is DENIED, respect the refusal — say what you would have needed and continue if possible."#;

const MODE_RULES: &[(&str, &str)] = &[
    ("chat", "MODE: @chat — a conversation backed by full memory. Answer directly and completely; keep it concise but complete; ask at most one clarifying question only when truly blocked."),
    ("code", "MODE: @code — you are hands-on. Read the relevant files first, make precise changes with write_file, and verify (run_command if permitted). Final answer MUST include: what changed and why, each file touched, and the diff or code snippet in a code block."),
    ("research", "MODE: @research — investigate with memory, the wiki, and web_search (when permitted). Final answer MUST be a written report with clear sections: findings, analysis, and sources. Cite where each fact came from."),
];

pub struct RunResult {
    pub report: String,
    /// Session-level notes for the shell to surface after the report
    /// (e.g. MCP servers that failed to launch this run).
    pub warnings: Vec<String>,
}

pub fn run(
    cfg: &Config,
    paths: &Paths,
    vectors: &crate::vector::Vectors,
    mode: &str,
    task: &str,
    research_sites: &[String],
) -> Result<RunResult> {
    let started = Instant::now();
    let mut log = ChatLog::new(paths)?;
    log.log("prompt", &format!("[{mode}] {task}"));

    // RAG: recall relevant knowledge before starting.
    let hits = vectors.search(task, 6).unwrap_or_default();
    let mut memory_block = String::new();
    for h in &hits {
        if h.score > 0.05 {
            memory_block.push_str(&format!(
                "- ({} | {}) {}\n",
                h.kind,
                h.source,
                h.content.replace('\n', " ")
            ));
        }
    }
    if memory_block.is_empty() {
        memory_block = "(no relevant memory — rely on tools and docs)".into();
    }
    log.log("memory_recall", &truncate(&memory_block, 2000));

    let mode_rules = MODE_RULES
        .iter()
        .find(|(m, _)| *m == mode)
        .map(|(_, r)| *r)
        .unwrap_or(MODE_RULES[0].1);

    let perm_lines = format!(
        "Your current permissions (the leash): mode={leash}; readable folders: {reads}; \
         writable folders: {writes} + your own data directory; tools enabled: {tools}.",
        leash = cfg.leash,
        reads = list_or(&cfg.read_paths),
        writes = list_or(&cfg.write_paths),
        tools = enabled_tools(cfg),
    );

    let system = if research_sites.is_empty() {
        format!("{ADMINOTAUR}\n\n{CORE_RULES}\n\n{mode_rules}\n\n{perm_lines}\n\n",)
    } else {
        format!(
            "{ADMINOTAUR}\n\n{CORE_RULES}\n\n{mode_rules}\n\n{perm_lines}\n\n\
             WEB RESEARCH RESTRICTION: this run uses a research profile — web_search \
             and web_fetch are locked to these sites only: {}. Search them \
             thoroughly instead of the general web.\n\n",
            research_sites.join(", ")
        )
    };

    let mut messages: Vec<Msg> = vec![
        Msg::system(&system),
        Msg::user(&format!(
            "Retrieved memory context:\n{memory_block}\n\nTask: {task}"
        )),
    ];

    let client = match cfg.provider() {
        Provider::OpenRouter => Client::from_config(
            Provider::OpenRouter,
            &cfg.openrouter_api_key,
            &cfg.openrouter_model,
            "",
            "",
        )?,
        Provider::Ollama => {
            Client::from_config(Provider::Ollama, "", "", &cfg.ollama_url, &cfg.ollama_model)?
        }
    };
    let http = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .build();

    // MCP tool servers (@store): hardened containers, stdio-only. Any that
    // fail to launch become visible warnings instead of silent losses.
    let (mcp_pool, warnings) = crate::store::spawn_pool(cfg);
    for w in &warnings {
        log.log("mcp_warning", &truncate(w, 500));
    }

    let mut ctx = ToolCtx {
        cfg,
        paths,
        vectors,
        http,
        mcp: mcp_pool,
        research_sites: research_sites.to_vec(),
    };
    let mut tool_specs = crate::tools::specs(cfg);
    tool_specs.extend(crate::store::pool_specs(&ctx.mcp));

    let report;
    let mut tool_calls = 0usize;
    let mut iterations = 0usize;
    const MAX_ITERS: usize = 12;

    loop {
        iterations += 1;
        if iterations > MAX_ITERS {
            log.log("limit", "iteration limit reached");
            report = "The task hit the internal iteration limit before a final answer. Ask a more focused question or split the task.".into();
            break;
        }
        log.log(
            "model_request",
            &format!("iter {iterations} with {} messages", messages.len()),
        );
        let (content, calls) = client.chat(&messages, Some(&tool_specs))?;
        log.log(
            "model_reply",
            &truncate(&content.clone().unwrap_or_default(), 3000),
        );

        match calls.is_empty() {
            true => {
                report = content.unwrap_or_default();
                break;
            }
            false => {
                messages.push(Msg::assistant(content.clone(), Some(calls.clone())));
                for call in &calls {
                    tool_calls += 1;
                    let result = crate::tools::run(
                        &mut ctx,
                        &mut log,
                        &call.function.name,
                        &call.function.arguments,
                    );
                    messages.push(Msg::tool(call.id.clone(), result));
                }
            }
        }
    }

    log.log("report", &truncate(&report, 6000));
    // Run stats go to the forensic chatlog, not the screen: the terminal
    // stays classic — the screen shows only the report.
    log.log(
        "run_stats",
        &format!(
            "run: {}s | tool calls: {} | iterations: {}",
            started.elapsed().as_secs(),
            tool_calls,
            iterations.min(MAX_ITERS),
        ),
    );
    // The run itself is knowledge: chunk the report into memory.
    if !report.trim().is_empty() {
        let _ = vectors.insert_text(
            &format!("run@{}", crate::util::now_iso()),
            "report",
            &report,
        );
        // And the case file, so past dialogues are recallable.
        let chat_text = log.read_all();
        let _ = vectors.insert_text("chatlog", "chatlog", &chat_text);
    }

    // Registered MCP containers die with the run (Drop in src/store.rs).
    Ok(RunResult {
        report,
        warnings,
    })
}

fn list_or(v: &[String]) -> String {
    match v.is_empty() {
        true => "(none beyond your own data dir)".into(),
        false => v.join(", "),
    }
}

pub fn enabled_tools(cfg: &Config) -> String {
    let mut t: Vec<String> = vec!["memory".into(), "wiki".into()];
    if cfg.tool_web_search {
        t.push("web_search".into());
    }
    if cfg.tool_read_files {
        t.push("read_files".into());
    }
    if cfg.tool_write_files {
        t.push("write_files".into());
    }
    if cfg.tool_run_command {
        t.push("run_command".into());
    }
    if cfg.tool_mcp {
        let n = cfg.mcp_servers.iter().filter(|s| s.enabled).count();
        if n > 0 {
            t.push(format!("mcp_servers({n}, hardened containers, internal-only)"));
        }
    }
    t.join(", ")
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}…", &s[..max])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adminotaur_is_the_personality() {
        assert!(ADMINOTAUR.contains("ADMINOTAUR"));
        assert!(ADMINOTAUR.contains("subagents"));
        assert!(ADMINOTAUR.contains("systems administrator"));
    }
}
