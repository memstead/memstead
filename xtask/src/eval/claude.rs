//! The real agent runner: shells to `claude -p` and parses its stream-json.
//!
//! The argument vector mirrors the production agent invocation (the same
//! `claude -p --mcp-config` shape the app's chat runtime spawns) so the eval
//! exercises the exact mount path a real user gets. The two arms differ in
//! precisely one place — whether
//! `--mcp-config` (and the `memstead_*` allow-list) is present:
//!
//! - **mem-on** mounts the subject mem over MCP and whitelists only
//!   `mcp__memstead__*`, so the agent's only tool is the graph.
//! - **mem-off** passes no MCP config and an empty allow-list, so the agent
//!   answers from the bare model — every other condition identical.
//!
//! That isolation is the whole point: the delta is attributable to the graph and
//! nothing else.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::{AgentAnswer, ArmConfig, Condition, Runner};

/// Runs prompts through the `claude` CLI.
pub struct ClaudeRunner {
    /// The `claude` executable (name on `$PATH`, or an absolute path).
    pub executable: String,
    /// An **empty** working directory the agent runs from. This is the real
    /// confound control: claude's built-in file tools (Read/Grep/Glob/Bash) are
    /// auto-allowed regardless of `--allowedTools` under this permission mode, so
    /// the only way to deny both arms direct codebase access is to give them a
    /// directory with no codebase in it. The mounted mem uses absolute paths
    /// and is reachable from anywhere — so mem-on can still read the graph
    /// while neither arm can read the source tree. The single variable holds.
    pub sandbox_dir: PathBuf,
}

impl Default for ClaudeRunner {
    fn default() -> Self {
        Self {
            executable: "claude".to_string(),
            sandbox_dir: std::env::temp_dir().join("memstead-eval-sandbox"),
        }
    }
}

impl Runner for ClaudeRunner {
    fn run(&self, arm: &ArmConfig) -> Result<AgentAnswer> {
        std::fs::create_dir_all(&self.sandbox_dir).with_context(|| {
            format!("creating agent sandbox dir {}", self.sandbox_dir.display())
        })?;
        let args = build_args(arm);
        let output = Command::new(&self.executable)
            .args(&args)
            // Run from the empty sandbox so built-in file tools find no codebase.
            .current_dir(&self.sandbox_dir)
            // Give the MCP server room to finish its handshake before the agent's
            // first turn. The pro server is ready in well under a second, but
            // claude's default connect window is tight enough that a `pending`
            // server occasionally leaves the agent tool-less for that turn.
            .env("MCP_TIMEOUT", "60000")
            .output()
            .with_context(|| {
                format!(
                    "spawning `{}` — is the claude CLI installed and on PATH?",
                    self.executable
                )
            })?;
        if !output.status.success() {
            bail!(
                "claude exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_stream_json(&stdout)
    }
}

/// Build the `claude -p` argument vector for an arm.
///
/// Pure and unit-tested: the single-variable property is visible here — the
/// mem-on branch is the only one that adds `--mcp-config` and the `memstead_*`
/// allow-list; everything before the `match` is identical across arms.
///
/// Tool gating is done entirely through `--allowedTools` under
/// `--permission-mode dontAsk`: in that mode any tool *not* on the allow-list is
/// auto-denied, so the allow-list is the exclusive tool set. mem-on allows only
/// `mcp__memstead__*` (the graph, and nothing that could read the codebase
/// directly — no confound); mem-off allows nothing (the bare model). We
/// deliberately do **not** pass `--tools ""`: in current `claude`, that flag
/// suppresses MCP tool registration entirely, leaving the agent tool-less and
/// prone to *fabricating* tool-call text instead of mounting the mem.
pub fn build_args(arm: &ArmConfig) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        arm.task_text.clone(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--model".to_string(),
        arm.model.clone(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
        "--strict-mcp-config".to_string(),
        // Replace the default system prompt (which references built-in tools the
        // allow-list denies) rather than appending to it.
        "--system-prompt".to_string(),
        arm.system_prompt.clone(),
    ];
    match (arm.condition, arm.mcp_config.as_ref()) {
        (Condition::MemOn, Some(cfg)) => {
            args.push("--mcp-config".to_string());
            args.push(cfg.display().to_string());
            args.push("--allowedTools".to_string());
            args.push("mcp__memstead__*".to_string());
        }
        // mem-off, or a degenerate mem-on with no mount: no MCP, no tools.
        _ => {
            args.push("--allowedTools".to_string());
            args.push(String::new());
        }
    }
    args
}

/// Parse `claude --output-format stream-json --verbose` NDJSON into an answer.
///
/// Port of the reference parser in `AgentRuntime.swift`: `assistant` events carry
/// a `message.content[]` array of `text` / `tool_use` items; the final `result`
/// event carries the answer text as a fallback. Unparseable lines are skipped
/// (the stream interleaves `system` and rate-limit events).
pub fn parse_stream_json(stdout: &str) -> Result<AgentAnswer> {
    let mut texts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<String> = Vec::new();
    let mut result_text: Option<String> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                if let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for item in content {
                        match item.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                                    if !t.is_empty() {
                                        texts.push(t.to_string());
                                    }
                                }
                            }
                            Some("tool_use") => {
                                if let Some(n) = item.get("name").and_then(|n| n.as_str()) {
                                    tool_calls.push(n.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("result") => {
                if let Some(r) = v.get("result").and_then(|r| r.as_str()) {
                    result_text = Some(r.to_string());
                }
            }
            _ => {}
        }
    }

    let text = if texts.is_empty() {
        result_text.unwrap_or_default()
    } else {
        texts.join("\n")
    };
    if text.trim().is_empty() {
        bail!("claude produced no answer text in its stream-json output");
    }
    Ok(AgentAnswer { text, tool_calls })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::TaskSpec;
    use crate::eval::arm::build_arms;

    fn task() -> TaskSpec {
        TaskSpec {
            id: "t".into(),
            prompt: "what changed?".into(),
            reference: "X".into(),
        }
    }

    #[test]
    fn mem_on_args_carry_mcp_config_and_allowlist() {
        let (on, _) = build_arms(&task(), "claude-opus-4-8", "sys", Some("/tmp/on.json".into()));
        let args = build_args(&on);
        assert!(args.windows(2).any(|w| w[0] == "--mcp-config" && w[1] == "/tmp/on.json"));
        assert!(args.windows(2).any(|w| w[0] == "--allowedTools" && w[1] == "mcp__memstead__*"));
        // The task text and model are passed through.
        assert!(args.windows(2).any(|w| w[0] == "-p" && w[1] == "what changed?"));
        assert!(args.windows(2).any(|w| w[0] == "--model" && w[1] == "claude-opus-4-8"));
    }

    #[test]
    fn mem_off_args_have_no_mcp_and_empty_allowlist() {
        let (_, off) = build_arms(&task(), "claude-opus-4-8", "sys", Some("/tmp/on.json".into()));
        let args = build_args(&off);
        assert!(!args.iter().any(|a| a == "--mcp-config"), "{args:?}");
        // allowedTools is present but empty — every tool denied.
        let allow_idx = args.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(args[allow_idx + 1], "");
    }

    #[test]
    fn args_differ_only_in_the_mount_block() {
        // Everything up to the arm-specific tail is byte-identical.
        let (on, off) = build_arms(&task(), "m", "sys", Some("/tmp/on.json".into()));
        let on_args = build_args(&on);
        let off_args = build_args(&off);
        let head = on_args.len() - 4; // both share the same fixed prefix length
        assert_eq!(on_args[..head], off_args[..head]);
    }

    #[test]
    fn parses_assistant_text_and_tool_use() {
        let stream = r#"
{"type":"system","subtype":"init"}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"mcp__memstead__memstead_search","input":{}}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"The serve crate added a read-only projection."}]}}
{"type":"result","subtype":"success","result":"The serve crate added a read-only projection."}
"#;
        let ans = parse_stream_json(stream).unwrap();
        assert!(ans.text.contains("read-only projection"));
        assert_eq!(ans.tool_calls, vec!["mcp__memstead__memstead_search"]);
    }

    #[test]
    fn falls_back_to_result_when_no_assistant_text() {
        let stream = r#"{"type":"result","subtype":"success","result":"final answer"}"#;
        let ans = parse_stream_json(stream).unwrap();
        assert_eq!(ans.text, "final answer");
        assert!(ans.tool_calls.is_empty());
    }

    #[test]
    fn empty_stream_is_an_error() {
        assert!(parse_stream_json("{\"type\":\"system\"}\n").is_err());
    }

    #[test]
    fn mem_off_stream_has_no_tool_calls() {
        // A bare-model answer — exactly what validate_mount_evidence expects off.
        let stream = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"from memory: X"}]}}"#;
        let ans = parse_stream_json(stream).unwrap();
        assert!(ans.tool_calls.is_empty());
    }
}
