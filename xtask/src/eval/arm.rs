//! Arm configuration and the two non-negotiable invariants that make a run
//! valid: **one variable** and **real mount path**.

use std::path::PathBuf;

use anyhow::{Result, bail};

use super::{AgentAnswer, Condition, TaskSpec};

/// The full configuration of one arm. Two arms of the same task are identical in
/// every field except `condition` and `mcp_config` — that pair *is* the single
/// variable under test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmConfig {
    pub condition: Condition,
    pub model: String,
    pub system_prompt: String,
    pub task_text: String,
    /// The MCP config that mounts the subject mem. `Some` for the mem-on arm,
    /// `None` for mem-off. The mem-off arm has every other tool but no
    /// `memstead_*` surface.
    pub mcp_config: Option<PathBuf>,
}

/// Build the matched arm pair for a task. By construction the two arms share
/// model, system prompt, and task text; they differ only in condition and mount.
///
/// Callers never hand-assemble arms for a real run — they go through here so the
/// single-variable property holds at the source. [`check_single_variable`] is the
/// backstop that catches a caller who bypasses this.
pub fn build_arms(
    task: &TaskSpec,
    model: &str,
    system_prompt: &str,
    mcp_config: Option<PathBuf>,
) -> (ArmConfig, ArmConfig) {
    let on = ArmConfig {
        condition: Condition::MemOn,
        model: model.to_string(),
        system_prompt: system_prompt.to_string(),
        task_text: task.prompt.clone(),
        mcp_config,
    };
    let off = ArmConfig {
        condition: Condition::MemOff,
        mcp_config: None,
        ..on.clone()
    };
    (on, off)
}

/// Refuse the run if the two arms differ in anything but mount state.
///
/// The mount variable is expressed by exactly two fields — `condition` and
/// `mcp_config` — and those are *expected* to differ. Any divergence in model,
/// system prompt, or task text is a confound: a second variable that would make
/// the resulting delta unattributable. The error names every offending field so
/// the operator sees precisely what leaked.
pub fn check_single_variable(on: &ArmConfig, off: &ArmConfig) -> Result<()> {
    let mut confounds = Vec::new();
    if on.model != off.model {
        confounds.push(format!("model ({:?} vs {:?})", on.model, off.model));
    }
    if on.system_prompt != off.system_prompt {
        confounds.push("system_prompt".to_string());
    }
    if on.task_text != off.task_text {
        confounds.push("task_text".to_string());
    }
    // The conditions must be the two distinct arms — not both the same.
    if on.condition == off.condition {
        confounds.push(format!(
            "condition (both {} — the arms are not distinct)",
            on.condition.label()
        ));
    }
    if !confounds.is_empty() {
        bail!(
            "refusing to run: the two arms differ in more than mount state — {}. \
             The only permitted difference is condition + mcp_config; everything else \
             is a confound that makes the delta unattributable.",
            confounds.join(", ")
        );
    }
    Ok(())
}

/// Confirm the arm actually exercised the mount path it claims.
///
/// Mem-on must show at least one `memstead_*` tool call — otherwise the mount
/// silently failed and the "with mem" answer was produced without the mem,
/// which would understate the delta. Mem-off must show *no* `memstead_*` call —
/// otherwise the mem leaked into the control arm. Either way the trial is
/// invalid and the run stops rather than reporting a corrupted number.
pub fn validate_mount_evidence(condition: Condition, answer: &AgentAnswer) -> Result<()> {
    let used_memstead = answer.tool_calls.iter().any(|t| is_memstead_tool(t));
    match condition {
        Condition::MemOn if !used_memstead => bail!(
            "invalid trial: mem-on arm shows no memstead_* tool use — the MCP mount \
             did not engage, so this answer was not actually produced with the mem"
        ),
        Condition::MemOff if used_memstead => bail!(
            "invalid trial: mem-off arm shows memstead_* tool use — the mem leaked \
             into the control arm: {}",
            answer
                .tool_calls
                .iter()
                .filter(|t| is_memstead_tool(t))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => Ok(()),
    }
}

/// A tool call that reaches the memstead MCP surface. Matches the
/// `mcp__memstead__*` naming the CLI emits as well as a bare `memstead_*` form.
fn is_memstead_tool(tool: &str) -> bool {
    tool.contains("memstead__") || tool.starts_with("memstead_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> TaskSpec {
        TaskSpec {
            id: "t".into(),
            prompt: "what is X?".into(),
            reference: "X is Y".into(),
        }
    }

    #[test]
    fn build_arms_differ_only_in_mount() {
        let (on, off) = build_arms(&task(), "claude-x", "sys", Some("/tmp/on.json".into()));
        assert_eq!(on.condition, Condition::MemOn);
        assert_eq!(off.condition, Condition::MemOff);
        assert_eq!(on.mcp_config, Some("/tmp/on.json".into()));
        assert_eq!(off.mcp_config, None);
        // Everything else matches.
        assert_eq!(on.model, off.model);
        assert_eq!(on.system_prompt, off.system_prompt);
        assert_eq!(on.task_text, off.task_text);
        // ...and the matched pair passes the guard.
        check_single_variable(&on, &off).unwrap();
    }

    #[test]
    fn confound_different_model_is_refused() {
        let (mut on, off) = build_arms(&task(), "claude-x", "sys", Some("/tmp/on.json".into()));
        on.model = "claude-y".into(); // a second variable
        let err = check_single_variable(&on, &off).unwrap_err().to_string();
        assert!(err.contains("model"), "{err}");
    }

    #[test]
    fn confound_altered_prompt_is_refused() {
        let (on, mut off) = build_arms(&task(), "claude-x", "sys", Some("/tmp/on.json".into()));
        off.system_prompt = "a different system prompt".into();
        let err = check_single_variable(&on, &off).unwrap_err().to_string();
        assert!(err.contains("system_prompt"), "{err}");
    }

    #[test]
    fn confound_altered_task_text_is_refused() {
        let (on, mut off) = build_arms(&task(), "claude-x", "sys", Some("/tmp/on.json".into()));
        off.task_text = "a different task".into();
        let err = check_single_variable(&on, &off).unwrap_err().to_string();
        assert!(err.contains("task_text"), "{err}");
    }

    #[test]
    fn non_distinct_arms_are_refused() {
        let (on, _) = build_arms(&task(), "claude-x", "sys", Some("/tmp/on.json".into()));
        let err = check_single_variable(&on, &on).unwrap_err().to_string();
        assert!(err.contains("condition"), "{err}");
    }

    #[test]
    fn mem_on_without_memstead_tool_is_invalid() {
        let ans = AgentAnswer {
            text: "answer".into(),
            tool_calls: vec!["Read".into()],
        };
        assert!(validate_mount_evidence(Condition::MemOn, &ans).is_err());
    }

    #[test]
    fn mem_on_with_memstead_tool_is_valid() {
        let ans = AgentAnswer {
            text: "answer".into(),
            tool_calls: vec!["mcp__memstead__memstead_search".into()],
        };
        validate_mount_evidence(Condition::MemOn, &ans).unwrap();
    }

    #[test]
    fn mem_off_with_memstead_tool_is_invalid() {
        // The mem leaked into the control arm.
        let ans = AgentAnswer {
            text: "answer".into(),
            tool_calls: vec!["mcp__memstead__memstead_entity".into()],
        };
        assert!(validate_mount_evidence(Condition::MemOff, &ans).is_err());
    }

    #[test]
    fn mem_off_without_memstead_tool_is_valid() {
        let ans = AgentAnswer {
            text: "answer".into(),
            tool_calls: vec!["Read".into(), "Grep".into()],
        };
        validate_mount_evidence(Condition::MemOff, &ans).unwrap();
    }
}
