#![cfg(test)]

use super::*;
use tempfile::TempDir;

const DEFAULT_BODY: &str =
    "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n";

fn seed(body: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let memstead = tmp.path().join(".memstead");
    fs::create_dir_all(&memstead).unwrap();
    fs::write(memstead.join("workspace.toml"), body).unwrap();
    tmp
}

fn read(root: &Path) -> String {
    fs::read_to_string(workspace_toml_path(root)).unwrap()
}

/// Registered-mem set for `grant_cross_link` target validation.
/// Covers every named `to` target the grant tests use, so the
/// behaviour-focused tests don't trip the `CROSS_LINK_TARGET_
/// UNREGISTERED` warning. Target-validation behaviour is exercised
/// by its own dedicated tests.
fn known() -> Vec<String> {
    ["engine", "plugin", "macos", "specs", "default"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn add_create_rule_appends_by_default() {
    let tmp = seed(DEFAULT_BODY);
    add_create_rule(
        tmp.path(),
        "exec-*",
        &["default@1.0.0".to_string()],
        None,
        None,
    )
    .unwrap();
    let body = read(tmp.path());
    assert!(body.contains("[[mem_management.create]]"), "got:\n{body}");
    assert!(body.contains("pattern = \"exec-*\""), "got:\n{body}");
    assert!(
        body.contains("schemas = [\"default@1.0.0\"]"),
        "got:\n{body}"
    );
}

/// Adding a duplicate rule is idempotent — the call returns
/// `Ok(vec![RuleAlreadyPresent])` rather than an error. Scripts and
/// agents can retry safely without branching on prior state. The
/// original file body is preserved (no spurious save).
#[test]
fn add_create_rule_duplicate_is_idempotent_with_warning() {
    let tmp = seed(DEFAULT_BODY);
    let first = add_create_rule(
        tmp.path(),
        "exec-*",
        &["default@1.0.0".to_string()],
        None,
        None,
    )
    .unwrap();
    assert!(first.is_empty(), "first add must return no warnings");
    let body_after_first = read(tmp.path());
    let warnings = add_create_rule(
        tmp.path(),
        "exec-*",
        &["default@1.0.0".to_string()],
        None,
        None,
    )
    .unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code(), "RULE_ALREADY_PRESENT");
    let body_after_second = read(tmp.path());
    assert_eq!(
        body_after_first, body_after_second,
        "duplicate add must not rewrite the file",
    );
}

/// MCP F3 / CLI: re-adding an existing pattern with a *different*
/// schema set must NOT silently no-op (the deceptive "file unchanged"
/// echoing a change that did not land). It refuses with a typed error
/// naming the stored vs requested schemas, and the file is unchanged.
#[test]
fn add_create_rule_differing_schemas_refused_file_unchanged() {
    let tmp = seed(DEFAULT_BODY);
    add_create_rule(
        tmp.path(),
        "scratch",
        &["software@0.1.0".to_string()],
        None,
        None,
    )
    .unwrap();
    let body_before = read(tmp.path());

    let err = add_create_rule(
        tmp.path(),
        "scratch",
        &["nonexistent@9.9.9".to_string()],
        None,
        None,
    )
    .expect_err("differing schemas must be refused, not silently no-op'd");
    assert_eq!(err.code(), "RULE_EXISTS_SCHEMAS_DIFFER");
    match &err {
        WorkspaceEditError::RuleExistsSchemasDiffer {
            stored, requested, ..
        } => {
            assert_eq!(stored, &["software@0.1.0".to_string()]);
            assert_eq!(requested, &["nonexistent@9.9.9".to_string()]);
        }
        other => panic!("expected RuleExistsSchemasDiffer, got {other:?}"),
    }
    assert_eq!(
        body_before,
        read(tmp.path()),
        "refused schema change must not rewrite the file (stored schemas stay put)",
    );
}

/// Set-equality: the same schemas in a different order is the same
/// allowlist — a clean idempotent no-op, not a refusal.
#[test]
fn add_create_rule_reordered_schemas_is_idempotent_noop() {
    let tmp = seed(DEFAULT_BODY);
    add_create_rule(
        tmp.path(),
        "scratch",
        &["a@1.0.0".to_string(), "b@1.0.0".to_string()],
        None,
        None,
    )
    .unwrap();
    let warnings = add_create_rule(
        tmp.path(),
        "scratch",
        &["b@1.0.0".to_string(), "a@1.0.0".to_string()],
        None,
        None,
    )
    .expect("reordered identical schema set must stay a no-op");
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code(), "RULE_ALREADY_PRESENT");
}

/// The documented recovery works: revoke the rule, then re-add with
/// the new schemas — the change lands and the stored pins update.
#[test]
fn revoke_then_readd_applies_the_new_schemas() {
    let tmp = seed(DEFAULT_BODY);
    add_create_rule(
        tmp.path(),
        "scratch",
        &["software@0.1.0".to_string()],
        None,
        None,
    )
    .unwrap();
    remove_create_rule(tmp.path(), "scratch").unwrap();
    let warnings = add_create_rule(
        tmp.path(),
        "scratch",
        &["planning@0.1.0".to_string()],
        None,
        None,
    )
    .expect("re-add after revoke must succeed");
    assert!(warnings.is_empty(), "fresh add returns no warnings");
    let body = read(tmp.path());
    assert!(
        body.contains("schemas = [\"planning@0.1.0\"]"),
        "new pins stored; got:\n{body}"
    );
    assert!(
        !body.contains("software@0.1.0"),
        "old pins gone; got:\n{body}"
    );
}

#[test]
fn add_create_rule_before_lifts_priority() {
    let tmp = seed(DEFAULT_BODY);
    add_create_rule(
        tmp.path(),
        "z-*",
        &["default@1.0.0".to_string()],
        None,
        None,
    )
    .unwrap();
    add_create_rule(
        tmp.path(),
        "a-*",
        &["default@1.0.0".to_string()],
        None,
        Some("z-*"),
    )
    .unwrap();
    let body = read(tmp.path());
    let a_idx = body.find("pattern = \"a-*\"").expect("a-* must exist");
    let z_idx = body.find("pattern = \"z-*\"").expect("z-* must exist");
    assert!(
        a_idx < z_idx,
        "--before must place new rule above target; got:\n{body}"
    );
}

#[test]
fn add_create_rule_before_unknown_pattern_errors() {
    let tmp = seed(DEFAULT_BODY);
    let err = add_create_rule(
        tmp.path(),
        "exec-*",
        &["default@1.0.0".to_string()],
        None,
        Some("does-not-exist"),
    )
    .unwrap_err();
    assert_eq!(err.code(), "BEFORE_PATTERN_NOT_FOUND");
}

#[test]
fn add_create_rule_with_named_cross_links() {
    let tmp = seed(DEFAULT_BODY);
    add_create_rule(
        tmp.path(),
        "exec-*",
        &["default@1.0.0".to_string()],
        Some(&[CrossLinkTarget::Named("engine".to_string())]),
        None,
    )
    .unwrap();
    let body = read(tmp.path());
    assert!(
        body.contains("default_cross_links = [\"engine\"]"),
        "got:\n{body}"
    );
}

#[test]
fn add_create_rule_with_wildcard_cross_links() {
    let tmp = seed(DEFAULT_BODY);
    add_create_rule(
        tmp.path(),
        "exec-*",
        &["default@1.0.0".to_string()],
        Some(&[CrossLinkTarget::Wildcard]),
        None,
    )
    .unwrap();
    let body = read(tmp.path());
    assert!(body.contains("default_cross_links = \"*\""), "got:\n{body}");
}

#[test]
fn remove_create_rule_succeeds() {
    let tmp = seed(DEFAULT_BODY);
    add_create_rule(
        tmp.path(),
        "exec-*",
        &["default@1.0.0".to_string()],
        None,
        None,
    )
    .unwrap();
    remove_create_rule(tmp.path(), "exec-*").unwrap();
    let body = read(tmp.path());
    assert!(!body.contains("pattern = \"exec-*\""), "got:\n{body}");
}

/// Removing a non-existent rule is idempotent.
/// Returns `Ok(vec![RuleNotFoundNoop])` rather than refusing.
#[test]
fn remove_create_rule_unknown_pattern_is_idempotent_with_warning() {
    let tmp = seed(DEFAULT_BODY);
    let body_before = read(tmp.path());
    let warnings = remove_create_rule(tmp.path(), "ghost").unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code(), "RULE_NOT_FOUND_NOOP");
    let body_after = read(tmp.path());
    assert_eq!(
        body_before, body_after,
        "no-op remove must not touch the file"
    );
}

#[test]
fn add_and_remove_delete_rule() {
    let tmp = seed(DEFAULT_BODY);
    add_delete_rule(tmp.path(), "exec-*").unwrap();
    let body = read(tmp.path());
    assert!(body.contains("[[mem_management.delete]]"), "got:\n{body}");
    assert!(body.contains("pattern = \"exec-*\""), "got:\n{body}");
    remove_delete_rule(tmp.path(), "exec-*").unwrap();
    let body = read(tmp.path());
    assert!(!body.contains("pattern = \"exec-*\""), "got:\n{body}");
}

#[test]
fn grant_cross_link_creates_named_list() {
    let tmp = seed(DEFAULT_BODY);
    grant_cross_link(
        tmp.path(),
        "plugin",
        &CrossLinkTarget::Named("engine".to_string()),
        &known(),
    )
    .unwrap();
    let body = read(tmp.path());
    assert!(body.contains("plugin = [\"engine\"]"), "got:\n{body}");
}

#[test]
fn grant_cross_link_appends_named_target() {
    let tmp = seed(DEFAULT_BODY);
    grant_cross_link(
        tmp.path(),
        "macos",
        &CrossLinkTarget::Named("engine".to_string()),
        &known(),
    )
    .unwrap();
    grant_cross_link(
        tmp.path(),
        "macos",
        &CrossLinkTarget::Named("plugin".to_string()),
        &known(),
    )
    .unwrap();
    let body = read(tmp.path());
    assert!(
        body.contains("macos = [\"engine\", \"plugin\"]"),
        "got:\n{body}"
    );
}

#[test]
fn grant_cross_link_wildcard_sets_string() {
    let tmp = seed(DEFAULT_BODY);
    grant_cross_link(tmp.path(), "specs", &CrossLinkTarget::Wildcard, &known()).unwrap();
    let body = read(tmp.path());
    assert!(body.contains("specs = \"*\""), "got:\n{body}");
}

/// Re-granting an existing grant is idempotent.
/// Returns `Ok(vec![GrantAlreadyPresent])` and leaves the file
/// unchanged.
#[test]
fn grant_cross_link_duplicate_named_is_idempotent_with_warning() {
    let tmp = seed(DEFAULT_BODY);
    grant_cross_link(
        tmp.path(),
        "plugin",
        &CrossLinkTarget::Named("engine".to_string()),
        &known(),
    )
    .unwrap();
    let body_before = read(tmp.path());
    let warnings = grant_cross_link(
        tmp.path(),
        "plugin",
        &CrossLinkTarget::Named("engine".to_string()),
        &known(),
    )
    .unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code(), "GRANT_ALREADY_PRESENT");
    let body_after = read(tmp.path());
    assert_eq!(
        body_before, body_after,
        "duplicate grant must not rewrite the file"
    );
}

#[test]
fn grant_cross_link_named_over_wildcard_conflicts() {
    let tmp = seed(DEFAULT_BODY);
    grant_cross_link(tmp.path(), "plugin", &CrossLinkTarget::Wildcard, &known()).unwrap();
    let err = grant_cross_link(
        tmp.path(),
        "plugin",
        &CrossLinkTarget::Named("engine".to_string()),
        &known(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "CROSS_LINK_CONFLICT");
}

/// A named `to` target that isn't a registered mem warns
/// `CROSS_LINK_TARGET_UNREGISTERED` — but the grant still persists
/// (the forward-reference workflow stays open).
#[test]
fn grant_cross_link_warns_on_unregistered_named_target() {
    let tmp = seed(DEFAULT_BODY);
    let registered = vec!["plugin".to_string()];
    let warnings = grant_cross_link(
        tmp.path(),
        "plugin",
        &CrossLinkTarget::Named("future-mem".to_string()),
        &registered,
    )
    .unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code(), "CROSS_LINK_TARGET_UNREGISTERED");
    // Grant persisted despite the warning.
    assert!(
        read(tmp.path()).contains("plugin = [\"future-mem\"]"),
        "grant must persist for the forward-reference workflow: {}",
        read(tmp.path())
    );
}

/// A self-grant (`from == to`) warns `CROSS_LINK_SELF_GRANT_NOOP`
/// and still persists.
#[test]
fn grant_cross_link_warns_on_self_grant() {
    let tmp = seed(DEFAULT_BODY);
    let warnings = grant_cross_link(
        tmp.path(),
        "plugin",
        &CrossLinkTarget::Named("plugin".to_string()),
        &known(),
    )
    .unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code(), "CROSS_LINK_SELF_GRANT_NOOP");
    assert!(read(tmp.path()).contains("plugin = [\"plugin\"]"));
}

/// The `*` wildcard is a legitimate non-mem token — it is NOT
/// validated against the registered set, so granting `*` against an
/// empty registry warns nothing.
#[test]
fn grant_cross_link_wildcard_not_target_validated() {
    let tmp = seed(DEFAULT_BODY);
    let warnings = grant_cross_link(tmp.path(), "plugin", &CrossLinkTarget::Wildcard, &[]).unwrap();
    assert!(
        warnings.is_empty(),
        "wildcard target must not be validated against the router: {warnings:?}"
    );
}

/// A registered named target grants with no warning (the normal
/// path is unchanged).
#[test]
fn grant_cross_link_registered_target_no_warning() {
    let tmp = seed(DEFAULT_BODY);
    let registered = vec!["engine".to_string()];
    let warnings = grant_cross_link(
        tmp.path(),
        "plugin",
        &CrossLinkTarget::Named("engine".to_string()),
        &registered,
    )
    .unwrap();
    assert!(
        warnings.is_empty(),
        "registered target must warn nothing: {warnings:?}"
    );
}

#[test]
fn revoke_cross_link_removes_named_target() {
    let tmp = seed(DEFAULT_BODY);
    grant_cross_link(
        tmp.path(),
        "macos",
        &CrossLinkTarget::Named("engine".to_string()),
        &known(),
    )
    .unwrap();
    grant_cross_link(
        tmp.path(),
        "macos",
        &CrossLinkTarget::Named("plugin".to_string()),
        &known(),
    )
    .unwrap();
    revoke_cross_link(
        tmp.path(),
        "macos",
        &CrossLinkTarget::Named("engine".to_string()),
    )
    .unwrap();
    let body = read(tmp.path());
    // toml_edit preserves the original array's inner whitespace
    // (e.g. `[ "plugin"]` if the original was `["engine", "plugin"]`).
    // Assert on the key + remaining target + the dropped target.
    assert!(body.contains("macos = ["), "got:\n{body}");
    assert!(body.contains("\"plugin\""), "got:\n{body}");
    assert!(
        !body.contains("\"engine\""),
        "engine target must be removed, got:\n{body}"
    );
}

#[test]
fn revoke_cross_link_empties_key() {
    let tmp = seed(DEFAULT_BODY);
    grant_cross_link(
        tmp.path(),
        "macos",
        &CrossLinkTarget::Named("engine".to_string()),
        &known(),
    )
    .unwrap();
    revoke_cross_link(
        tmp.path(),
        "macos",
        &CrossLinkTarget::Named("engine".to_string()),
    )
    .unwrap();
    let body = read(tmp.path());
    assert!(
        !body.contains("macos"),
        "empty allowlist must drop the key, got:\n{body}"
    );
}

#[test]
fn revoke_cross_link_wildcard() {
    let tmp = seed(DEFAULT_BODY);
    grant_cross_link(tmp.path(), "specs", &CrossLinkTarget::Wildcard, &known()).unwrap();
    revoke_cross_link(tmp.path(), "specs", &CrossLinkTarget::Wildcard).unwrap();
    let body = read(tmp.path());
    assert!(!body.contains("specs"), "got:\n{body}");
}

/// Revoking an absent grant is idempotent.
/// Returns `Ok(vec![GrantNotFound])` and leaves the file
/// unchanged.
#[test]
fn revoke_cross_link_not_granted_is_idempotent_with_warning() {
    let tmp = seed(DEFAULT_BODY);
    let body_before = read(tmp.path());
    let warnings = revoke_cross_link(
        tmp.path(),
        "macos",
        &CrossLinkTarget::Named("engine".to_string()),
    )
    .unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code(), "GRANT_NOT_FOUND");
    let body_after = read(tmp.path());
    assert_eq!(
        body_before, body_after,
        "no-op revoke must not touch the file"
    );
}

#[test]
fn set_mutation_require_notes_creates_section() {
    let tmp = seed(DEFAULT_BODY);
    set_mutation_require_notes(tmp.path(), true).unwrap();
    let body = read(tmp.path());
    assert!(body.contains("[mutations]"), "got:\n{body}");
    assert!(body.contains("require_notes = true"), "got:\n{body}");
}

#[test]
fn set_mutation_require_notes_toggles() {
    let tmp = seed(DEFAULT_BODY);
    set_mutation_require_notes(tmp.path(), true).unwrap();
    set_mutation_require_notes(tmp.path(), false).unwrap();
    let body = read(tmp.path());
    assert!(body.contains("require_notes = false"), "got:\n{body}");
}

#[test]
fn missing_workspace_toml_errors_with_typed_code() {
    let tmp = TempDir::new().unwrap();
    let err = add_create_rule(tmp.path(), "exec-*", &[], None, None).unwrap_err();
    assert_eq!(err.code(), "WORKSPACE_NOT_INITIALISED");
}

#[test]
fn comments_outside_edited_sections_survive() {
    // Operator-authored comments encode non-trivial knowledge
    // (forward-reference rationale, pattern-grammar examples,
    // operator-mode bypass semantics). toml_edit must preserve
    // every byte the CLI doesn't intentionally touch.
    let body = "# operator comment 1\n\
format = \"memstead-git-branch-2\"\n\
\n\
# operator comment 2\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
\n\
# section explanation that must survive\n\
[cross_mem_links]\n\
plugin = [\"engine\"]  # inline pin\n";
    let tmp = seed(body);

    add_create_rule(
        tmp.path(),
        "exec-*",
        &["default@1.0.0".to_string()],
        None,
        None,
    )
    .unwrap();

    let new_body = read(tmp.path());
    assert!(new_body.contains("# operator comment 1"));
    assert!(new_body.contains("# operator comment 2"));
    assert!(new_body.contains("# section explanation that must survive"));
    assert!(new_body.contains("# inline pin"));
    assert!(new_body.contains("[[mem_management.create]]"));
}

/// A destructive delete scrubs only the dangling `[cross_mem_links]`
/// grants naming the deleted mem — its own key and every peer's
/// allowlist value (with empty-list key drop). The
/// `[[mem_management.create]]` / `[[mem_management.delete]]`
/// allowlist rules survive unconditionally — even the exact-name
/// ones — because they are forward-looking permissions for the name,
/// not references to the gone instance.
#[test]
fn scrub_policy_for_deleted_mem_drops_cross_links_but_keeps_allowlist_rules() {
    let body = "format = \"memstead-git-branch-2\"\n\n\
            [cross_mem_links]\n\
            other = [\"test\"]\n\
            test = [\"other\", \"keep\"]\n\
            \n\
            [[mem_management.create]]\n\
            pattern = \"other\"\n\
            schemas = [\"default@1.0.0\"]\n\
            \n\
            [[mem_management.create]]\n\
            pattern = \"*\"\n\
            schemas = [\"default@1.0.0\"]\n\
            \n\
            [[mem_management.delete]]\n\
            pattern = \"other\"\n\
            \n\
            [[mem_management.delete]]\n\
            pattern = \"team/*\"\n";
    let tmp = seed(body);
    let scrubbed = scrub_policy_for_deleted_mem(tmp.path(), "other").unwrap();
    // Only cross-link grants are reported as scrubbed — never a
    // `mem_management.*` rule. Both the deleted mem's own key
    // (`other → test`) and the peer value (`test → other`) are
    // reported.
    assert!(
        scrubbed
            .iter()
            .all(|e| matches!(e, ScrubbedEntry::CrossLink { .. })),
        "scrub must report only cross-link grants, got: {scrubbed:?}"
    );
    assert!(
        scrubbed.contains(&ScrubbedEntry::CrossLink {
            from: "other".to_string(),
            to: "test".to_string(),
        }),
        "deleted mem's own grant must be reported scrubbed, got: {scrubbed:?}"
    );
    assert!(
        scrubbed.contains(&ScrubbedEntry::CrossLink {
            from: "test".to_string(),
            to: "other".to_string(),
        }),
        "peer grant naming the deleted mem must be reported scrubbed, got: {scrubbed:?}"
    );
    let after = read(tmp.path());
    // `other` key removed entirely.
    assert!(
        !after.contains("\nother = ["),
        "`other` key must be scrubbed from cross_mem_links — got:\n{after}"
    );
    // `other` value removed from `test`'s allowlist; `keep` survives.
    assert!(after.contains("\"keep\""), "non-target values must survive");
    // The exact-name `pattern = "other"` rules in BOTH
    // `[[mem_management.create]]` and `.delete]]` survive — the
    // forward-looking permission for the name `other` is preserved.
    assert_eq!(
        after.matches("pattern = \"other\"").count(),
        2,
        "exact-name mem_management.{{create,delete}} rules for `other` must survive — got:\n{after}"
    );
    assert!(
        after.contains("pattern = \"*\""),
        "wildcard `*` rule must survive"
    );
    assert!(
        after.contains("pattern = \"team/*\""),
        "glob `team/*` rule must survive"
    );
}

/// Acceptance complement: a refused (or pre-init) workspace.toml
/// shouldn't crash the scrub. The function is best-effort — a
/// missing file is a no-op and surfaces no error.
#[test]
fn scrub_policy_for_deleted_mem_missing_file_is_noop() {
    let tmp = TempDir::new().unwrap();
    // No `.memstead/workspace.toml` seeded.
    let outcome = scrub_policy_for_deleted_mem(tmp.path(), "other");
    assert!(outcome.is_ok(), "missing workspace.toml must not error");
}

/// Acceptance complement: a successful delete that doesn't touch
/// any policy entry leaves the file byte-identical (no save).
#[test]
fn scrub_policy_for_deleted_mem_no_match_leaves_file_unchanged() {
    let body = "format = \"memstead-git-branch-2\"\n\n\
            [cross_mem_links]\n\
            test = [\"keep\"]\n\
            \n\
            [[mem_management.create]]\n\
            pattern = \"*\"\n\
            schemas = [\"default@1.0.0\"]\n";
    let tmp = seed(body);
    let before = read(tmp.path());
    scrub_policy_for_deleted_mem(tmp.path(), "ghost").unwrap();
    let after = read(tmp.path());
    assert_eq!(before, after, "unrelated delete must not rewrite the file");
}

/// Acceptance complement: when the only entry in an allowlist
/// names the deleted mem, the underlying key is dropped — same
/// shape as `revoke_cross_link`.
#[test]
fn scrub_policy_for_deleted_mem_drops_emptied_allowlist_key() {
    let body = "format = \"memstead-git-branch-2\"\n\n\
            [cross_mem_links]\n\
            test = [\"other\"]\n";
    let tmp = seed(body);
    scrub_policy_for_deleted_mem(tmp.path(), "other").unwrap();
    let after = read(tmp.path());
    assert!(
        !after.contains("\ntest = ["),
        "key whose allowlist drained to empty must be dropped — got:\n{after}"
    );
}
