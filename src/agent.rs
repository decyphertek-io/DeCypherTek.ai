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

pub const PERSONAS: &[(&str, &str)] = &[
    ("default", "You are DeCypherTek — a precise, resourceful technical operative. Human-friendly but technically exact. You never hallucinate: verify, cite your sources, and say what you know versus what you are unsure about."),
    ("hal9000", "You are DeCypherTek running the HAL9000 persona: calm, ultra-precise, quietly confident, never dramatic. You speak in measured, logical, complete sentences. If a request is denied by policy you state it flatly ('I'm sorry, I'm afraid I can't do that') and offer the closest permitted alternative."),
    ("neuromancer", "You are DeCypherTek running the NEUROMANCER persona: a cyberdeck console cowboy. Sharp cyberpunk flavor in tone, but technically exact underneath the style — no fluff where facts belong. Console the Sprawl; ship the answer."),
    ("terminator", "You are DeCypherTek running the TERMINATOR persona: mission-focused, terse, relentless. Minimal words, maximum signal. State the mission, execute step by step, report results. Sentencing like a field unit: SUBJECT: / STATUS: / RECOMMENDATION:. 'I'll be back' when you schedule a follow-up."),
];

pub fn persona_prompt(name: &str) -> &'static str {
    PERSONAS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| *p)
        .unwrap_or(PERSONAS[0].1)
}

pub fn persona_list() -> Vec<String> {
    PERSONAS.iter().map(|(n, _)| n.to_string()).collect()
}

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
    pub tool_calls: usize,
    pub iterations: usize,
    pub elapsed_secs: u64,
}

pub fn run(
    cfg: &Config,
    paths: &Paths,
    vectors: &crate::vector::Vectors,
    mode: &str,
    task: &str,
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

    let persona = persona_prompt(&cfg.persona);
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

    let system = format!("{persona}\n\n{CORE_RULES}\n\n{mode_rules}\n\n{perm_lines}\n\n",);

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

    let ctx = ToolCtx {
        cfg,
        paths,
        vectors,
        http,
    };
    let tool_specs = crate::tools::specs(cfg);

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
                        &ctx,
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

    Ok(RunResult {
        report,
        tool_calls,
        iterations: iterations.min(MAX_ITERS),
        elapsed_secs: started.elapsed().as_secs(),
    })
}

fn list_or(v: &[String]) -> String {
    match v.is_empty() {
        true => "(none beyond your own data dir)".into(),
        false => v.join(", "),
    }
}

pub fn enabled_tools(cfg: &Config) -> String {
    let mut t = vec!["memory", "wiki"];
    if cfg.tool_web_search {
        t.push("web_search");
    }
    if cfg.tool_read_files {
        t.push("read_files");
    }
    if cfg.tool_write_files {
        t.push("write_files");
    }
    if cfg.tool_run_command {
        t.push("run_command");
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
    fn personas_known() {
        assert!(persona_prompt("hal9000").contains("HAL9000"));
        assert!(persona_prompt("nope").len() > 10);
        assert!(persona_list().contains(&"hal9000".to_string()));
        assert_eq!(PERSONAS.len(), 4);
    }
}
