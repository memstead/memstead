#![cfg(test)]

use super::*;

/// The scaffold is one definition, so every front door writes the same
/// record: scoped source, materialised deny defaults, full operations.
#[test]
fn codebase_scaffold_carries_the_deny_defaults_and_every_operation() {
    let s = scaffold_binding(ScaffoldParams {
        destination_mem: "app",
        source_name: "app",
        pointer: ".",
        medium_type: MediumType::Codebase,
        intent: Some("model it".to_string()),
        additional_deny_paths: Vec::new(),
    });
    assert_eq!(s.operations, vec!["build", "sync", "verify"]);
    assert_eq!(s.warnings, Vec::<String>::new());
    assert_eq!(s.binding.deny_paths, DEFAULT_SCAFFOLD_DENY_PATHS);
    assert_eq!(s.binding.sources[0].scope[0].path, "**/*");
    assert_eq!(s.binding.sources[0].pointer, ".");
    assert!(s.binding.coverage_semantics.is_none(), "asserts nothing");
    assert!(
        s.binding.prune.is_some(),
        "sync survived, so prune rides it"
    );
}

/// A caller's extra deny entries are materialised alongside the
/// defaults — visible, editable, and never silently deduplicated away
/// into a different list.
#[test]
fn additional_deny_paths_are_appended_once() {
    let s = scaffold_binding(ScaffoldParams {
        destination_mem: "app",
        source_name: "app",
        pointer: ".",
        medium_type: MediumType::Codebase,
        intent: None,
        additional_deny_paths: vec!["build/**".to_string(), "**/.git/**".to_string()],
    });
    let expected: Vec<String> = DEFAULT_SCAFFOLD_DENY_PATHS
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once("build/**".to_string()))
        .collect();
    assert_eq!(s.binding.deny_paths, expected);
}

/// A medium the matrix cannot serve loses the operation and says so,
/// rather than scaffolding a record that refuses at run time.
#[test]
fn web_scaffold_loses_sync_and_verify_with_a_warning() {
    let s = scaffold_binding(ScaffoldParams {
        destination_mem: "app",
        source_name: "manual",
        pointer: "https://example.com/manual",
        medium_type: MediumType::Web,
        intent: None,
        additional_deny_paths: Vec::new(),
    });
    assert_eq!(s.operations, vec!["build"]);
    assert!(!s.warnings.is_empty(), "the deferral is named");
    assert!(s.binding.deny_paths.is_empty(), "no path denies over web");
    assert!(s.binding.prune.is_none(), "no sync, no prune");
}
