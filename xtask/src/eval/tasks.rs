//! Loading the run inputs: the task set (a JSON file) and the mem states (CLI
//! `--state label=path` flags).
//!
//! The task file is source-agnostic by design — a hand-authored question and a
//! git-mined "what broke when X landed?" task are the same three fields, so both
//! kinds flow through the identical scorer without a format change.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{TaskSpec, MemState};

#[derive(Debug, Deserialize)]
struct TaskFileEntry {
    id: String,
    prompt: String,
    reference: String,
}

/// Load `[{id, prompt, reference}, …]` from a JSON file.
pub fn load_tasks(path: &Path) -> Result<Vec<TaskSpec>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading task file {}", path.display()))?;
    let entries: Vec<TaskFileEntry> =
        serde_json::from_str(&text).with_context(|| format!("parsing task file {}", path.display()))?;
    if entries.is_empty() {
        bail!("task file {} contains no tasks", path.display());
    }
    Ok(entries
        .into_iter()
        .map(|e| TaskSpec {
            id: e.id,
            prompt: e.prompt,
            reference: e.reference,
        })
        .collect())
}

/// Parse a `--state label=path` argument into a [`MemState`].
///
/// `label=` with an empty path yields a state with no mount (`mcp_config: None`)
/// — the degenerate "no mem" baseline. A non-empty path is the MCP config that
/// mounts that state of the mem; an *empty graph* state is expressed as a path
/// to an MCP config whose mem happens to be empty, so its mem-on arm still
/// mounts and the mount-evidence check still holds.
pub fn parse_state_arg(arg: &str) -> Result<MemState> {
    let (label, path) = arg
        .split_once('=')
        .with_context(|| format!("--state must be `label=path`, got {arg:?}"))?;
    if label.is_empty() {
        bail!("--state label must be non-empty in {arg:?}");
    }
    let mcp_config = if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    };
    Ok(MemState {
        label: label.to_string(),
        mcp_config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_tasks_from_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        std::fs::write(
            &path,
            r#"[{"id":"a","prompt":"why X?","reference":"because Y"},
                {"id":"b","prompt":"what broke?","reference":"the parser"}]"#,
        )
        .unwrap();
        let tasks = load_tasks(&path).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "a");
        assert_eq!(tasks[1].reference, "the parser");
    }

    #[test]
    fn empty_task_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        std::fs::write(&path, "[]").unwrap();
        assert!(load_tasks(&path).is_err());
    }

    #[test]
    fn parses_state_with_path() {
        let s = parse_state_arg("v1=/tmp/v1.json").unwrap();
        assert_eq!(s.label, "v1");
        assert_eq!(s.mcp_config, Some(PathBuf::from("/tmp/v1.json")));
    }

    #[test]
    fn parses_empty_state_as_no_mount() {
        let s = parse_state_arg("baseline=").unwrap();
        assert_eq!(s.label, "baseline");
        assert_eq!(s.mcp_config, None);
    }

    #[test]
    fn rejects_malformed_state() {
        assert!(parse_state_arg("no-equals-sign").is_err());
        assert!(parse_state_arg("=/tmp/x.json").is_err());
    }
}
