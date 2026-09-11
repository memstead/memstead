#![cfg(test)]

use super::*;
use crate::storage::git_tree::GitTreeBackend;
use crate::vcs::CommitContext;
use memstead_base::backend::MemBackend;
use std::path::PathBuf;
use tempfile::TempDir;

fn init_gitdir(tmp: &TempDir) -> PathBuf {
    let gitdir = tmp.path().join("mem-repo").join(".git");
    std::fs::create_dir_all(&gitdir).unwrap();
    gix::init_bare(&gitdir).unwrap();
    gitdir
}

fn body_with_title(title: &str) -> String {
    // Per-title unique padding so gix's similarity-driven rename
    // detection (50% default) does not pair unrelated test
    // entities as a single Rewrite event. Plain `# {title}` bodies
    // were too similar across entities; the repeated title token
    // here pushes each body's hash far enough apart that gix sees
    // distinct adds and deletes.
    let unique = title.repeat(64);
    format!(
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\n{unique}\n"
    )
}

fn write_and_commit(gitdir: &Path, mem: &str, entries: &[(&str, &str)], subject: &str) -> String {
    let writer = GitTreeBackend::new(gitdir.to_path_buf(), format!("refs/heads/{mem}"));
    for (path, content) in entries {
        writer
            .write_entity(Path::new(path), content.as_bytes())
            .unwrap();
    }
    writer.commit(subject, &CommitContext::internal()).unwrap()
}

#[test]
fn diff_unknown_ref_returns_unknown_ref_marker() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    let err = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        "no-such-ref",
        "no-such-other",
        &DiffConfig::default(),
    )
    .unwrap_err();
    match err {
        BackendError::Other(msg) => {
            assert!(
                msg.starts_with("UNKNOWN_REF:"),
                "expected UNKNOWN_REF marker, got: {msg}",
            );
        }
        other => panic!("expected Other, got {other:?}"),
    }
}

#[test]
fn normalise_rewrites_only_the_head_token() {
    // #53: the HEAD base of a revspec re-anchors on the mem branch,
    // suffix preserved.
    assert_eq!(normalise_ref_for_branch("v", "HEAD"), "refs/heads/v");
    assert_eq!(normalise_ref_for_branch("v", "HEAD~5"), "refs/heads/v~5");
    assert_eq!(normalise_ref_for_branch("v", "HEAD^"), "refs/heads/v^");
    assert_eq!(
        normalise_ref_for_branch("v", "HEAD^{tree}"),
        "refs/heads/v^{tree}"
    );
    assert_eq!(
        normalise_ref_for_branch("v", "HEAD@{1}"),
        "refs/heads/v@{1}"
    );
    // Refusal: a ref that merely starts with "HEAD" is left alone.
    assert_eq!(normalise_ref_for_branch("v", "HEADER"), "HEADER");
    assert_eq!(normalise_ref_for_branch("v", "HEAD-foo"), "HEAD-foo");
    // Refusal: a plain branch / SHA passes through unchanged.
    assert_eq!(normalise_ref_for_branch("v", "main"), "main");
    assert_eq!(normalise_ref_for_branch("v", "deadbeef"), "deadbeef");
    // The anchor is the DECLARED branch, never a mem-name-derived
    // ref: a namespaced mount's `HEAD` lands on its namespace, and
    // an already-qualified declared branch is used verbatim.
    assert_eq!(
        normalise_ref_for_branch("team/v", "HEAD"),
        "refs/heads/team/v"
    );
    assert_eq!(
        normalise_ref_for_branch("refs/heads/team/v", "HEAD~2"),
        "refs/heads/team/v~2"
    );
}

#[test]
fn diff_bare_head_resolves_on_namespaced_declared_branch() {
    // A mem whose declared branch is not its name under the
    // standard prefix: `HEAD` must land on the declared branch.
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/team/specs".to_string());
    writer
        .write_entity(Path::new("a.md"), body_with_title("A").as_bytes())
        .unwrap();
    let sha = writer.commit("seed", &CommitContext::internal()).unwrap();

    let diff = diff_two_refs(
        &gitdir,
        "team/specs",
        "specs",
        EMPTY_TREE_SHA,
        "HEAD",
        &DiffConfig::default(),
    )
    .unwrap();
    assert_eq!(diff.resolved_b_sha, sha);
    assert_eq!(diff.entries.len(), 1);
}

#[test]
fn diff_head_revspec_anchors_on_mem_branch() {
    // #53: `HEAD~1` / `HEAD` resolve against `refs/heads/<mem>`, not
    // the gitdir's symbolic HEAD (the dummy default branch, which has no
    // commits here — before the fix these revspecs would refuse).
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha"))],
        "c1",
    );
    write_and_commit(
        &gitdir,
        "specs",
        &[("beta.md", &body_with_title("Beta"))],
        "c2 add beta",
    );

    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        "HEAD~1",
        "HEAD",
        &DiffConfig::default(),
    )
    .expect("HEAD revspec must resolve against the mem branch");
    assert_eq!(diff.entries.len(), 1, "only beta added between c1 and c2");
    assert!(
        matches!(diff.entries[0], EntityDiff::Added { .. }),
        "the one change is beta added: {:?}",
        diff.entries[0]
    );
}

#[test]
fn diff_added_modified_deleted_surface() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);

    // Ref A: alpha + beta with body B0.
    write_and_commit(
        &gitdir,
        "specs",
        &[
            ("alpha.md", &body_with_title("Alpha")),
            ("beta.md", &body_with_title("Beta-v0")),
        ],
        "seed",
    );
    let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

    // Ref B: alpha unchanged, beta body changed, gamma added.
    write_and_commit(
        &gitdir,
        "specs",
        &[
            ("beta.md", &body_with_title("Beta-v1")),
            ("gamma.md", &body_with_title("Gamma")),
        ],
        "update beta + add gamma",
    );
    // Drop alpha in a third commit so it surfaces as a deletion.
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    writer.delete_entity(Path::new("alpha.md")).unwrap();
    writer
        .commit("drop alpha", &CommitContext::internal())
        .unwrap();

    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &sha_a,
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .unwrap();
    assert_eq!(diff.entries.len(), 3, "Add+Modify+Delete expected");
    let statuses: Vec<&str> = diff
        .entries
        .iter()
        .map(|e| match e {
            EntityDiff::Added { .. } => "added",
            EntityDiff::Modified { .. } => "modified",
            EntityDiff::Deleted { .. } => "deleted",
            EntityDiff::Renamed { .. } => "renamed",
            EntityDiff::InvalidEntity { .. } => "invalid",
        })
        .collect();
    // Sorted by primary id: alpha (deleted), beta (modified), gamma (added).
    assert_eq!(statuses, vec!["deleted", "modified", "added"]);

    // Content enrichment defaults to on: both sides populated for
    // the modified entry; one-sided for added/deleted.
    let beta = diff
        .entries
        .iter()
        .find(|e| matches!(e, EntityDiff::Modified { .. }))
        .unwrap();
    match beta {
        EntityDiff::Modified {
            content_before,
            content_after,
            ..
        } => {
            assert!(content_before.as_ref().unwrap().contains("Beta-v0"));
            assert!(content_after.as_ref().unwrap().contains("Beta-v1"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn diff_include_content_false_strips_bodies() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha"))],
        "seed",
    );
    let sha_seed = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha-v2"))],
        "rev2",
    );

    let cfg = DiffConfig {
        include_content: false,
        ..DiffConfig::default()
    };
    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &sha_seed,
        "refs/heads/specs",
        &cfg,
    )
    .unwrap();
    assert_eq!(diff.entries.len(), 1);
    match &diff.entries[0] {
        EntityDiff::Modified {
            title,
            entity_type,
            content_before,
            content_after,
            ..
        } => {
            assert!(
                content_before.is_none(),
                "include_content=false elides before"
            );
            assert!(
                content_after.is_none(),
                "include_content=false elides after"
            );
            // Metadata is present regardless of the content toggle —
            // the docstring's metadata-only shape promises it and a
            // JSON consumer needs `title`/`entity_type` without
            // parsing bodies. Sourced from the post-state (`ref_b`).
            assert_eq!(
                title.as_deref(),
                Some("Alpha-v2"),
                "title present (from ref_b) even with include_content=false"
            );
            assert_eq!(
                entity_type.as_deref(),
                Some("spec"),
                "entity_type present even with include_content=false"
            );
        }
        other => panic!("expected Modified, got {other:?}"),
    }
}

/// With `include_content: true` the metadata fields are additive to
/// the body fields — `{id, title, entity_type, status}` plus
/// `content_before`/`content_after`, not a replacement. Also pins
/// the added-entry shape (post-state metadata from `ref_b`).
#[test]
fn diff_populates_title_and_type_with_content_on() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    let sha_empty = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"; // empty tree
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha"))],
        "seed",
    );

    let cfg = DiffConfig {
        include_content: true,
        ..DiffConfig::default()
    };
    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        sha_empty,
        "refs/heads/specs",
        &cfg,
    )
    .unwrap();
    assert_eq!(diff.entries.len(), 1);
    match &diff.entries[0] {
        EntityDiff::Added {
            title,
            entity_type,
            content_after,
            ..
        } => {
            assert_eq!(title.as_deref(), Some("Alpha"), "title populated on Added");
            assert_eq!(
                entity_type.as_deref(),
                Some("spec"),
                "entity_type populated"
            );
            assert!(
                content_after.is_some(),
                "content_after present and additive to the metadata fields"
            );
        }
        other => panic!("expected Added, got {other:?}"),
    }
}

#[test]
fn diff_rename_chain_collapses_multi_step_engine_authored_renames() {
    // Seed alpha, rename to beta via an engine-style commit, then
    // rename beta to gamma. Diffing seed → head should surface a
    // single `Renamed { from: alpha, to: gamma, chain: [beta] }`
    // rather than a chain of intermediate edits.
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);

    let body = body_with_title("Title");
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    writer
        .write_entity(Path::new("alpha.md"), body.as_bytes())
        .unwrap();
    writer.commit("seed", &CommitContext::internal()).unwrap();
    let sha_seed = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

    // alpha → beta. Move the file (delete + write) and emit the
    // commit subject the engine uses on its rename pipeline.
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    writer.delete_entity(Path::new("alpha.md")).unwrap();
    writer
        .write_entity(Path::new("beta.md"), body.as_bytes())
        .unwrap();
    writer
        .commit(
            "memstead: rename specs--alpha → specs--beta",
            &CommitContext::internal(),
        )
        .unwrap();

    // beta → gamma. Same shape.
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    writer.delete_entity(Path::new("beta.md")).unwrap();
    writer
        .write_entity(Path::new("gamma.md"), body.as_bytes())
        .unwrap();
    writer
        .commit(
            "memstead: rename specs--beta → specs--gamma",
            &CommitContext::internal(),
        )
        .unwrap();

    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &sha_seed,
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .unwrap();

    let renamed = diff
        .entries
        .iter()
        .find(|e| matches!(e, EntityDiff::Renamed { .. }))
        .expect("a Renamed entry must surface");
    match renamed {
        EntityDiff::Renamed {
            from_id,
            to_id,
            rename_chain,
            ..
        } => {
            assert_eq!(from_id.to_string(), "specs--alpha");
            assert_eq!(to_id.to_string(), "specs--gamma");
            assert_eq!(
                rename_chain
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>(),
                vec!["specs--beta".to_string()],
                "the multi-step rename's intermediate id must surface in rename_chain",
            );
        }
        _ => unreachable!(),
    }
    // No leftover Added/Deleted entries for the chain endpoints.
    let leftover: Vec<_> = diff
        .entries
        .iter()
        .filter(|e| matches!(e, EntityDiff::Added { .. } | EntityDiff::Deleted { .. }))
        .collect();
    assert!(
        leftover.is_empty(),
        "agent-notes promotion must absorb the Added+Deleted pair, got: {leftover:?}",
    );
}

#[test]
fn diff_ripple_lists_incoming_wikilinks_on_each_side() {
    // Build a mem with three entities: alpha, beta, gamma.
    // beta links to alpha on ref_a; gamma links to alpha on ref_b.
    // Modify alpha between the two refs. The diff entry for
    // alpha should surface both referrers in its ripple list,
    // each tagged with the right side.
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);

    let alpha_v1 = body_with_title("Alpha-v1");
    let beta_links_alpha = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Beta\n\n## Identity\n\nLinks to [[specs--alpha]].\n".to_string();
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &alpha_v1), ("beta.md", &beta_links_alpha)],
        "seed",
    );
    let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

    let alpha_v2 = body_with_title("Alpha-v2");
    let gamma_links_alpha = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Gamma\n\n## Identity\n\nLinks to [[specs--alpha]].\n".to_string();
    // Drop beta to break its outbound link on ref_b. Add gamma
    // with a fresh inbound link to alpha.
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    writer
        .write_entity(Path::new("alpha.md"), alpha_v2.as_bytes())
        .unwrap();
    writer
        .write_entity(Path::new("gamma.md"), gamma_links_alpha.as_bytes())
        .unwrap();
    writer.delete_entity(Path::new("beta.md")).unwrap();
    writer.commit("rev2", &CommitContext::internal()).unwrap();

    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &sha_a,
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .unwrap();

    // Find the entry for alpha; both ripple sides must surface.
    let alpha = diff
        .entries
        .iter()
        .find(|e| matches!(e, EntityDiff::Modified { id, .. } if id.to_string() == "specs--alpha"))
        .expect("alpha must show as modified");
    match alpha {
        EntityDiff::Modified { ripple, .. } => {
            let mut sides_seen: Vec<String> = ripple
                .iter()
                .map(|r| format!("{}@{}", r.from_id, r.side))
                .collect();
            sides_seen.sort();
            assert_eq!(
                sides_seen,
                vec![
                    "specs--beta@ref_a".to_string(),
                    "specs--gamma@ref_b".to_string(),
                ],
                "ripple must list beta on ref_a and gamma on ref_b",
            );
        }
        _ => unreachable!(),
    }
}

/// The ripple scanner holds a whole git blob, not a section body,
/// so it must trim the frontmatter before the link scan. Without
/// that, a YAML value that reads as a CommonMark fence opener
/// (legal at 1-3 spaces) opens a code block that runs past the
/// `---` terminator to end of file and masks every link in the
/// body away — the referrer silently vanishes from the ripple list.
#[test]
fn ripple_survives_frontmatter_that_looks_like_a_fence() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);

    let alpha_v1 = body_with_title("Alpha-v1");
    // `notes: |` opens a YAML block scalar whose first line is a
    // 3-space-indented fence.
    let beta = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\nnotes: |\n   ```\n   sample\n---\n# Beta\n\n## Identity\n\nLinks to [[specs--alpha]].\n"
            .to_string();
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &alpha_v1), ("beta.md", &beta)],
        "seed",
    );
    let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

    let alpha_v2 = body_with_title("Alpha-v2");
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    writer
        .write_entity(Path::new("alpha.md"), alpha_v2.as_bytes())
        .unwrap();
    writer.commit("rev2", &CommitContext::internal()).unwrap();

    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &sha_a,
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .unwrap();

    let alpha = diff
        .entries
        .iter()
        .find(|e| matches!(e, EntityDiff::Modified { id, .. } if id.to_string() == "specs--alpha"))
        .expect("alpha must show as modified");
    match alpha {
        EntityDiff::Modified { ripple, .. } => {
            assert!(
                ripple
                    .iter()
                    .any(|r| r.from_id.to_string() == "specs--beta"),
                "beta's prose link must still ripple; frontmatter masked the body away: {ripple:?}"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn diff_include_ripple_false_leaves_ripple_empty() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    let beta_links_alpha = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Beta\n\n## Identity\n\n[[specs--alpha]]\n";
    write_and_commit(
        &gitdir,
        "specs",
        &[
            ("alpha.md", &body_with_title("Alpha-v1")),
            ("beta.md", beta_links_alpha),
        ],
        "seed",
    );
    let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha-v2"))],
        "rev2",
    );

    let cfg = DiffConfig {
        include_ripple: false,
        ..DiffConfig::default()
    };
    let diff = diff_two_refs(&gitdir, "specs", "specs", &sha_a, "refs/heads/specs", &cfg).unwrap();
    for entry in &diff.entries {
        let ripple = match entry {
            EntityDiff::Added { ripple, .. }
            | EntityDiff::Modified { ripple, .. }
            | EntityDiff::Deleted { ripple, .. }
            | EntityDiff::Renamed { ripple, .. } => ripple.clone(),
            EntityDiff::InvalidEntity { .. } => Vec::new(),
        };
        assert!(
            ripple.is_empty(),
            "include_ripple=false must produce empty ripple lists: {entry:?}",
        );
    }
}

#[test]
fn diff_invalid_entity_surfaces_for_missing_frontmatter() {
    // An entity whose markdown body has no frontmatter block
    // demotes to `InvalidEntity` instead of `Modified` / `Added`.
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha-v1"))],
        "seed",
    );
    let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();
    // Overwrite alpha with a body that has no frontmatter.
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    writer
        .write_entity(
            Path::new("alpha.md"),
            b"# Alpha but no frontmatter\n\nbody.\n",
        )
        .unwrap();
    writer.commit("break", &CommitContext::internal()).unwrap();

    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &sha_a,
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .unwrap();
    let alpha = diff.entries.first().expect("alpha should appear");
    match alpha {
        EntityDiff::InvalidEntity {
            id, side, error, ..
        } => {
            assert_eq!(id.to_string(), "specs--alpha");
            assert_eq!(side, "ref_b");
            assert!(error.contains("frontmatter"), "unexpected error: {error}");
        }
        other => panic!("expected InvalidEntity, got {other:?}"),
    }
}

#[test]
fn engine_diff_routes_git_branch_mount_through_hook() {
    // End-to-end: build an engine with a git-branch mount that
    // points at our seeded gitdir, install the full ops bundle so
    // the engine's `diff` dispatcher reaches our `diff_two_refs`
    // implementation, and assert the returned `Diff` is the same
    // one calling the function directly would produce.
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha"))],
        "seed",
    );
    let sha_seed = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha-v2"))],
        "rev2",
    );

    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "default",
            semver::Version::new(1, 0, 0),
        )),
        storage: memstead_base::MountStorage::GitBranch {
            gitdir: gitdir.clone(),
            branch: "specs".to_string(),
        },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let backend = crate::storage::instantiate_full_backend(&mount).unwrap();
    let mut engine = memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap();
    engine.set_git_branch_ops(crate::storage::FULL_GIT_BRANCH_OPS);

    let diff = engine
        .diff("specs", &sha_seed, "refs/heads/specs", None)
        .unwrap();
    assert_eq!(diff.entries.len(), 1);
    assert!(matches!(diff.entries[0], EntityDiff::Modified { .. }));
    assert_eq!(diff.resolved_a_sha, sha_seed);
}

#[test]
fn engine_diff_unknown_ref_surfaces_typed_engine_error() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha"))],
        "seed",
    );

    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "default",
            semver::Version::new(1, 0, 0),
        )),
        storage: memstead_base::MountStorage::GitBranch {
            gitdir: gitdir.clone(),
            branch: "specs".to_string(),
        },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let backend = crate::storage::instantiate_full_backend(&mount).unwrap();
    let mut engine = memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap();
    engine.set_git_branch_ops(crate::storage::FULL_GIT_BRANCH_OPS);

    let err = engine.diff("specs", "nope-a", "nope-b", None).unwrap_err();
    match err {
        memstead_base::EngineError::UnknownRef(raw) => {
            assert!(raw.contains("nope"), "unexpected UnknownRef payload: {raw}");
        }
        other => panic!("expected UnknownRef, got {other:?}"),
    }
}

#[test]
fn diff_resolves_refs_to_sha_in_response() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha"))],
        "seed",
    );
    let sha = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        "refs/heads/specs",
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .unwrap();
    assert_eq!(diff.resolved_a_sha, sha);
    assert_eq!(diff.resolved_b_sha, sha);
    // Same ref vs. same ref → no entries.
    assert!(diff.entries.is_empty());
}

/// The canonical empty-tree SHA is accepted as
/// `ref_a` and short-circuits to the empty tree, so a first-sync
/// diff lists every entity in the mem as `added`.
/// `resolved_a_sha` echoes the sentinel verbatim.
#[test]
fn diff_accepts_empty_tree_sentinel_for_ref_a() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[
            ("alpha.md", &body_with_title("Alpha")),
            ("beta.md", &body_with_title("Beta")),
        ],
        "seed",
    );

    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        EMPTY_TREE_SHA,
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .expect("empty-tree sentinel must be accepted");
    assert_eq!(diff.resolved_a_sha, EMPTY_TREE_SHA);
    assert_eq!(diff.entries.len(), 2, "first-sync lists every entity");
    for entry in &diff.entries {
        assert!(
            matches!(entry, EntityDiff::Added { .. }),
            "first-sync entries must all be `added`, got: {entry:?}",
        );
    }
}

/// A real tree-only SHA that
/// is NOT the canonical empty-tree sentinel continues to refuse
/// with `UNKNOWN_REF`. The sentinel handling is keyed on the
/// literal hash, not on "is it a tree".
#[test]
fn diff_refuses_arbitrary_tree_sha_with_unknown_ref() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha"))],
        "seed",
    );
    // Find a real tree SHA (the seed commit's tree) — that SHA
    // is a tree, not a commit, and isn't the canonical empty
    // tree, so resolve_tree's "is it a commit" gate refuses it.
    let repo = gix::open(&gitdir).unwrap();
    let head = repo.rev_parse_single("refs/heads/specs").unwrap();
    let head_commit = head.object().unwrap().try_into_commit().unwrap();
    let tree_sha = head_commit.tree().unwrap().id.to_string();
    assert_ne!(
        tree_sha, EMPTY_TREE_SHA,
        "tree SHA must differ from sentinel for this test to be meaningful"
    );

    let err = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &tree_sha,
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .unwrap_err();
    match err {
        BackendError::Other(msg) => assert!(
            msg.starts_with("UNKNOWN_REF:"),
            "expected UNKNOWN_REF marker, got: {msg}",
        ),
        other => panic!("expected Other, got {other:?}"),
    }
}

/// Bare `HEAD` substitutes to `refs/heads/<mem>`
/// so the diff targets the mem's branch tip, not the gitdir's
/// symbolic HEAD on a dummy default branch. Compares behaviour
/// against the explicit `refs/heads/<mem>` form — both calls
/// produce the same `resolved_b_sha` and same entries.
#[test]
fn diff_resolves_bare_head_to_mem_branch_tip() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    // Seed a commit on the mem branch.
    write_and_commit(
        &gitdir,
        "specs",
        &[("alpha.md", &body_with_title("Alpha"))],
        "seed",
    );
    let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

    // Advance the mem branch.
    write_and_commit(
        &gitdir,
        "specs",
        &[("beta.md", &body_with_title("Beta"))],
        "add beta",
    );

    // The gitdir's symbolic HEAD points at `refs/heads/main` (an
    // unrelated default branch with no commits) by default —
    // resolving bare HEAD literally would either refuse or hit
    // the wrong branch. The substitution targets
    // `refs/heads/<mem>` per the mem selector.
    let via_head = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &sha_a,
        "HEAD",
        &DiffConfig::default(),
    )
    .expect("bare HEAD must resolve via mem substitution");
    let via_explicit = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        &sha_a,
        "refs/heads/specs",
        &DiffConfig::default(),
    )
    .expect("explicit refs/heads/<mem> still works");

    // Both calls land on the same commit and produce the same
    // entry set — the bare-HEAD substitution is structurally
    // equivalent to the explicit form.
    assert_eq!(via_head.resolved_b_sha, via_explicit.resolved_b_sha);
    assert_eq!(via_head.entries.len(), via_explicit.entries.len());
}

/// First-sync diff against
/// the mem-branch HEAD using only the canonical sentinel and
/// bare `HEAD`. Collapses to one call (no need to look up the
/// explicit ref names).
#[test]
fn diff_empty_tree_sentinel_and_bare_head_compose() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    write_and_commit(
        &gitdir,
        "specs",
        &[
            ("alpha.md", &body_with_title("Alpha")),
            ("beta.md", &body_with_title("Beta")),
        ],
        "seed",
    );
    let diff = diff_two_refs(
        &gitdir,
        "specs",
        "specs",
        EMPTY_TREE_SHA,
        "HEAD",
        &DiffConfig::default(),
    )
    .expect("sentinel + HEAD must compose");
    assert_eq!(diff.resolved_a_sha, EMPTY_TREE_SHA);
    assert_eq!(diff.entries.len(), 2);
    assert!(
        diff.entries
            .iter()
            .all(|e| matches!(e, EntityDiff::Added { .. })),
        "every entity surfaces as added on first-sync diff"
    );
}
