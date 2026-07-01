//! Cross-language golden-fixture tests for `check_config`. Inputs and
//! committed outputs live under `tests/fixtures/validation/` and are
//! consumed by both this Rust test and the Swift `WorkspaceService`
//! tests.
//!
//! Set `UPDATE_FIXTURES=1` to regenerate
//! `validation/invalid/expected/*.json` from current Rust behavior.
//! Commit the updates alongside any intentional semantic change.
//!
//! The pre-rework migration fixtures (`fixtures/migration/`) were
//! retired by the workspace rewrite — the engine no
//! longer recognises `mediums` / `projections` blocks, so the
//! projection-shape migration has no target to migrate toward.

use std::path::{Path, PathBuf};

use memstead_schema::check_config;
use serde_json::Value;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn update_fixtures() -> bool {
    std::env::var_os("UPDATE_FIXTURES").is_some()
}

fn read_json(path: &Path) -> Value {
    let s = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&s)
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

fn list_json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("missing fixture dir {}: {e}", dir.display()))
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    out.sort();
    out
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct InvalidExpectation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
    /// Substrings that must appear in at least one error message each.
    /// Kept loose (substring, not exact match) so minor wording changes
    /// don't force a fixture update.
    #[serde(default)]
    errors_contain: Vec<String>,
}

#[test]
fn validation_valid_fixtures_pass() {
    let dir = fixtures_root().join("validation/valid");
    let files = list_json_files(&dir);
    assert!(!files.is_empty(), "no valid fixtures found");
    for path in files {
        let v = read_json(&path);
        let result = check_config(&v);
        assert!(
            result.valid,
            "valid fixture {} failed validation: {:?}",
            path.file_name().unwrap().to_string_lossy(),
            result.errors
        );
    }
}

#[test]
fn validation_invalid_fixtures_fail_expected() {
    let input_dir = fixtures_root().join("validation/invalid/input");
    let expected_dir = fixtures_root().join("validation/invalid/expected");
    std::fs::create_dir_all(&expected_dir).ok();

    let inputs = list_json_files(&input_dir);
    assert!(!inputs.is_empty(), "no invalid fixtures found");

    for input_path in inputs {
        let name = input_path.file_name().unwrap().to_os_string();
        let expected_path = expected_dir.join(&name);

        let v = read_json(&input_path);
        let result = check_config(&v);
        assert!(
            !result.valid,
            "invalid fixture {} unexpectedly passed validation",
            name.to_string_lossy()
        );

        if update_fixtures() {
            let snapshot = InvalidExpectation {
                error_code: result.error_code.clone(),
                errors_contain: result.errors.clone(),
            };
            std::fs::write(
                &expected_path,
                serde_json::to_string_pretty(&snapshot).unwrap() + "\n",
            )
            .unwrap();
            continue;
        }

        let raw = std::fs::read_to_string(&expected_path).unwrap_or_else(|_| {
            panic!(
                "missing expected file for {}; rerun with UPDATE_FIXTURES=1",
                name.to_string_lossy()
            )
        });
        let expected: InvalidExpectation = serde_json::from_str(&raw).unwrap();

        assert_eq!(
            result.error_code, expected.error_code,
            "error_code mismatch for {}",
            name.to_string_lossy()
        );
        for needle in &expected.errors_contain {
            assert!(
                result.errors.iter().any(|e| e.contains(needle)),
                "fixture {}: no error contains {:?}; got {:?}",
                name.to_string_lossy(),
                needle,
                result.errors
            );
        }
    }
}
