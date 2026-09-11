#![cfg(test)]

use super::*;

#[test]
fn normalize_resolves_dot_and_dotdot() {
    assert_eq!(
        normalize_lexical(Path::new("/a/b/../c/./d")),
        PathBuf::from("/a/c/d")
    );
    assert_eq!(
        normalize_lexical(Path::new("/a/../../b")),
        PathBuf::from("/b"),
        "dotdot past root is clamped"
    );
}

#[test]
fn relative_computes_updowns() {
    assert_eq!(
        relative_path(Path::new("/a/b"), Path::new("/a/b/c/d")),
        PathBuf::from("c/d")
    );
    assert_eq!(
        relative_path(Path::new("/a/b/c"), Path::new("/a/x")),
        PathBuf::from("../../x")
    );
    // A workspace whose medium is a sibling repository.
    assert_eq!(
        relative_path(Path::new("/m/public"), Path::new("/m/public/crates/x.rs")),
        PathBuf::from("crates/x.rs")
    );
    assert_eq!(
        relative_path(Path::new("/m/graph"), Path::new("/m/public/crates/x.rs")),
        PathBuf::from("../public/crates/x.rs")
    );
}

#[test]
fn pathspec_builds_glob_magic_relative_to_git_root() {
    let ws = Path::new("/m/graph");
    let git_root = Path::new("/m/public");
    assert_eq!(
        to_git_pathspec("../public/**/*.rs", git_root, ws, false),
        ":(glob)**/*.rs"
    );
    assert_eq!(
        to_git_pathspec("../public/target/**", git_root, ws, true),
        ":(glob,exclude)target/**"
    );
}

/// A `**`-prefixed pattern (the scaffolded facet default `**/*`) is
/// prefix-free and re-anchors verbatim onto the git root. Lexical
/// re-rooting would yield `:(glob)../**/*` for any sub-medium — an
/// out-of-tree pathspec git fatals on, degrading every diff to
/// no-signal.
#[test]
fn wildcard_prefixed_pathspec_reanchors_verbatim() {
    let ws = Path::new("/m/ws");
    let git_root = Path::new("/m/ws/src");
    assert_eq!(to_git_pathspec("**/*", git_root, ws, false), ":(glob)**/*");
    assert_eq!(
        in_repo_pathspec("**/__pycache__/**", git_root, ws, true).as_deref(),
        Some(":(glob,exclude)**/__pycache__/**")
    );
}

use memstead_base::binding_run::Source;
use memstead_base::pipeline::{MediumType, PatternEntry};

fn git(repo: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

fn primary(scope: Vec<PatternEntry>) -> Source {
    Source {
        name: "src".to_string(),
        medium_type: MediumType::Codebase,
        pointer: String::new(),
        change_detection: Some("git".to_string()),
        scope,
        engagement: None,
        preparation: None,
    }
}

/// One dialect, one implementation: the SAME entry list must exclude the
/// SAME files from an engine slice as [`memstead_base::check_path::check_deny_paths`]
/// denies. Successor to the retired cross-boundary fixture test that
/// pinned the engine against the plugin's JS dialect clone — both callers
/// now run engine code, and this test keeps the two engine consumers
/// (enumeration, path check) agreeing on shared data. Proven by
/// materialising every path into a temp workspace, scoping a facet to
/// `**` (everything), applying the entries as the ingest `deny_paths`,
/// and asserting `enumerate_facet_files` yields exactly `allowed`.
#[test]
fn deny_dialect_agrees_between_slice_and_check() {
    let strs = |items: &[&str]| -> Vec<String> { items.iter().map(|s| s.to_string()).collect() };
    let entries = strs(&["dev/**", "**/VISION.md", "docs/meta/CLAUDE.md"]);
    let blocked = strs(&[
        "dev/notes/a.md",
        "dev/x.rs",
        "dev/deep/nested/y.txt",
        "VISION.md",
        "crates/foo/VISION.md",
        "docs/meta/CLAUDE.md",
    ]);
    let allowed = strs(&[
        "src/lib.rs",
        "dev-tools/x.rs",
        "VISION-draft.md",
        "docs/meta/README.md",
        "other/CLAUDE.md",
        "crates/foo/mod.rs",
    ]);

    let ws = tempfile::tempdir().unwrap();
    for rel in blocked.iter().chain(allowed.iter()) {
        let path = ws.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "x").unwrap();
    }

    // Scope = everything; the ONLY exclusions are the ingest deny_paths.
    let source = primary(vec![PatternEntry {
        path: "**".to_string(),
        mode: PatternMode::Allow,
    }]);
    let mut got = enumerate_facet_files(&source, &entries, ws.path());
    got.sort();
    let mut want = allowed.clone();
    want.sort();
    assert_eq!(
        got, want,
        "engine slice must equal the fixture `allowed` set"
    );

    for b in &blocked {
        assert!(
            !got.contains(b),
            "denied `{b}` leaked into the engine slice"
        );
    }
    // The path check agrees on every case — the two engine consumers of
    // the dialect can never drift apart silently.
    let all: Vec<String> = blocked.iter().chain(allowed.iter()).cloned().collect();
    let checks = memstead_base::check_path::check_deny_paths(&entries, &all, ws.path(), ws.path());
    for c in &checks {
        let expect = blocked.contains(&c.path);
        assert_eq!(
            c.denied, expect,
            "check_deny_paths disagrees with the slice on `{}`",
            c.path
        );
    }
}

/// A cross-repo deny (its target sibling to the medium's git repo)
/// resolves outside `git_root` and is dropped from the pathspecs — pushing
/// it would make git fatal on the whole diff. An in-repo deny is kept.
#[test]
fn out_of_repo_deny_pathspec_is_dropped() {
    let ws = Path::new("/m/graph");
    let git_root = Path::new("/m/public");
    // `../dev/**` (workspace-relative) → /m/dev/** — outside /m/public.
    assert_eq!(in_repo_pathspec("../dev/**", git_root, ws, true), None);
    assert_eq!(in_repo_pathspec("../CLAUDE.md", git_root, ws, true), None);
    // An in-repo deny is preserved as a normal exclude pathspec.
    assert_eq!(
        in_repo_pathspec("../public/target/**", git_root, ws, true),
        Some(":(glob,exclude)target/**".to_string())
    );
}

/// A git-shaped baseline the repo does NOT contain reseeds at HEAD
/// instead of degrading to `GitUnavailable` forever. Regression for the
/// this project's own plugin/graph binding, whose stored baseline was a commit of a
/// *different* repo (seeded before the source moved into the submodule):
/// every pass diffed against a foreign sha, fataled, and the baseline
/// never seated.
#[test]
fn foreign_baseline_reseeds_instead_of_degrading() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    std::fs::write(root.join("keep.rs"), "one").unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "seed"]);

    let source = primary(vec![PatternEntry {
        path: "**/*.rs".to_string(),
        mode: PatternMode::Allow,
    }]);
    // Git-token-shaped, but no such commit exists in this repo.
    let foreign = "46ce8add0fe87250527b6fa21fcfdc2d943d51f0";
    match compute_git_slice(&source, &[], root, Some(foreign)) {
        SliceOutcome::Reseed { token } => {
            // Reseeds at the repo's actual HEAD — the baseline seats.
            let head = String::from_utf8(
                std::process::Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(root)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string();
            assert_eq!(token, head);
        }
        other => panic!("foreign baseline must reseed, got {other:?}"),
    }
}

/// A real git diff with a cross-repo deny present must still succeed (the
/// out-of-repo pathspec is dropped, not fataled), and the in-repo scope is
/// honoured. Regression for this project's own dialect (`../dev/**` under a
/// sub-medium): git must not degrade the whole slice.
#[test]
fn git_slice_survives_cross_repo_deny() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    std::fs::write(root.join("keep.rs"), "one").unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "seed"]);
    let baseline = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    std::fs::write(root.join("keep.rs"), "two").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "move"]);

    let source = primary(vec![PatternEntry {
        path: "**/*.rs".to_string(),
        mode: PatternMode::Allow,
    }]);
    // `../dev/**` resolves outside this repo — must be dropped, not fatal.
    let outcome = compute_git_slice(&source, &["../dev/**".to_string()], root, Some(&baseline));
    match outcome {
        SliceOutcome::Changed { slice, .. } => {
            assert_eq!(slice.modified, vec!["keep.rs"]);
        }
        other => panic!("expected Changed (deny dropped), got {other:?}"),
    }
}

/// A sub-tree-pointed source's changed slice honours the pointer join the
/// enumeration honours: a `**`-prefixed scope glob anchors at the medium
/// base, so a change OUTSIDE the pointed subtree never enters the slice.
/// Regression for drift-benchmark runs 03/06 (`plugin/graph`): the slice
/// presented repo-wide artifacts that `S(D)` correctly excluded and the
/// exclude gate refused, steering sync at artifacts it had no mandate over.
#[test]
fn git_slice_confines_to_the_medium_subtree() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/in.md"), "one").unwrap();
    std::fs::write(root.join("out.md"), "one").unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "seed"]);
    let baseline = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    std::fs::write(root.join("sub/in.md"), "two").unwrap();
    std::fs::write(root.join("out.md"), "two").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "move both"]);

    let source = Source {
        pointer: "sub".to_string(),
        ..primary(vec![PatternEntry {
            path: "**/*.md".to_string(),
            mode: PatternMode::Allow,
        }])
    };
    let outcome = compute_git_slice(&source, &[], root, Some(&baseline));
    match outcome {
        SliceOutcome::Changed { slice, .. } => {
            assert_eq!(slice.modified, vec!["sub/in.md"]);
            assert!(slice.added.is_empty());
            assert!(slice.deleted.is_empty());
        }
        other => panic!("expected Changed confined to the subtree, got {other:?}"),
    }
}

/// A real git diff: baseline commit → HEAD produces the changed slice,
/// classifying added / modified / deleted and honouring the scope.
#[test]
fn git_slice_diffs_baseline_to_head() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    std::fs::write(root.join("keep.rs"), "one").unwrap();
    std::fs::write(root.join("gone.rs"), "bye").unwrap();
    std::fs::write(root.join("note.md"), "ignored-by-scope").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    // Move: modify keep.rs, delete gone.rs, add new.rs, touch note.md.
    std::fs::write(root.join("keep.rs"), "two").unwrap();
    std::fs::remove_file(root.join("gone.rs")).unwrap();
    std::fs::write(root.join("new.rs"), "hi").unwrap();
    std::fs::write(root.join("note.md"), "still ignored").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "move"]);

    // Scope to *.rs only — note.md must not appear.
    let source = primary(vec![PatternEntry {
        path: "**/*.rs".to_string(),
        mode: PatternMode::Allow,
    }]);
    let outcome = compute_git_slice(&source, &[], root, Some(&baseline));
    match outcome {
        SliceOutcome::Changed {
            slice, degraded, ..
        } => {
            assert!(!degraded);
            assert_eq!(slice.added, vec!["new.rs"]);
            assert_eq!(slice.modified, vec!["keep.rs"]);
            assert_eq!(slice.deleted, vec!["gone.rs"]);
        }
        other => panic!("expected Changed, got {other:?}"),
    }

    // Same baseline == HEAD → Unchanged.
    let head = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    assert!(matches!(
        compute_git_slice(&source, &[], root, Some(&head)),
        SliceOutcome::Unchanged { .. }
    ));

    // A non-commit baseline → Reseed at HEAD.
    assert!(matches!(
        compute_git_slice(&source, &[], root, None),
        SliceOutcome::Reseed { .. }
    ));
}

/// Facet-file enumeration honours allow globs, deny globs, and the
/// codebase/filesystem medium-type gate.
/// The defect this join closes: a source with a non-empty pointer whose
/// scope is written the way the brief presents it (paths beneath the
/// pointer) selects those artifacts. Under the retired workspace-relative
/// reading the prefix-anchored pattern matched nothing and the walk
/// returned an empty set with no signal that anything had been dropped.
#[test]
fn scope_patterns_resolve_against_the_source_pointer() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src/lib/deep")).unwrap();
    std::fs::write(root.join("src/lib/a.rs"), "").unwrap();
    std::fs::write(root.join("src/lib/deep/b.rs"), "").unwrap();
    std::fs::write(root.join("src/lib/skip.md"), "").unwrap();
    std::fs::write(root.join("outside.rs"), "").unwrap();

    let mut source = primary(vec![
        PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        },
        PatternEntry {
            path: "deep/**".to_string(),
            mode: PatternMode::Deny,
        },
    ]);
    source.pointer = "src/lib".to_string();
    let got = enumerate_facet_files(&source, &[], root);
    assert_eq!(
        got,
        vec!["src/lib/a.rs".to_string()],
        "the deny is source-relative too, and nothing outside the pointer enters"
    );
}

/// A bare literal beneath the pointer is the shape that could never even
/// appear as uncovered under the old reading: it matched nothing, so it
/// was absent from the denominator rather than present-and-uncovered.
#[test]
fn a_bare_literal_under_the_pointer_selects_its_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(root.join("pkg/Cargo.toml"), "").unwrap();
    std::fs::write(root.join("pkg/other.toml"), "").unwrap();

    let mut source = primary(vec![PatternEntry {
        path: "Cargo.toml".to_string(),
        mode: PatternMode::Allow,
    }]);
    source.pointer = "pkg".to_string();
    assert_eq!(
        enumerate_facet_files(&source, &[], root),
        vec!["pkg/Cargo.toml".to_string()]
    );
}

/// An ingest-level deny stays workspace-relative — it spans every source
/// in the binding, so it has no pointer to be relative to. The two
/// namespaces coexist on one candidate without either leaking.
#[test]
fn ingest_denies_stay_workspace_relative() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src/secret")).unwrap();
    std::fs::write(root.join("src/keep.rs"), "").unwrap();
    std::fs::write(root.join("src/secret/hide.rs"), "").unwrap();

    let mut source = primary(vec![PatternEntry {
        path: "**/*.rs".to_string(),
        mode: PatternMode::Allow,
    }]);
    source.pointer = "src".to_string();
    let got = enumerate_facet_files(&source, &["src/secret/**".to_string()], root);
    assert_eq!(got, vec!["src/keep.rs".to_string()]);
}

/// An empty pointer is the shape the scaffolder writes and the only one
/// where the two readings coincide — it must be untouched by the join.
#[test]
fn an_empty_pointer_is_unaffected_by_the_join() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/a.md"), "").unwrap();
    std::fs::write(root.join("docs/b.txt"), "").unwrap();

    let source = primary(vec![PatternEntry {
        path: "docs/**/*.md".to_string(),
        mode: PatternMode::Allow,
    }]);
    assert_eq!(
        enumerate_facet_files(&source, &[], root),
        vec!["docs/a.md".to_string()]
    );
}

/// The enumerator and the path-check oracle agree on the rules that
/// EXTEND the glob, not merely on the glob library: a bare-name deny
/// (which degrades to a directory-prefix block) and a malformed deny
/// (whose literal base still blocks) must exclude from `S(D)` exactly
/// what they deny at the hook. Before the shared resolver these two
/// shapes were denied at the hook and still counted in the denominator.
#[test]
fn the_enumerator_applies_the_oracle_extra_rules_too() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for dir in ["bare", "broken"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::write(root.join(dir).join("f.rs"), "").unwrap();
    }
    std::fs::write(root.join("keep.rs"), "").unwrap();

    // `bare` is a legacy bare name (no metacharacter at all); `broken[`
    // will not compile, so only its literal base can block.
    let denies = vec!["bare".to_string(), "broken[".to_string()];
    let source = primary(vec![PatternEntry {
        path: "**/*.rs".to_string(),
        mode: PatternMode::Allow,
    }]);
    let enumerated = enumerate_facet_files(&source, &denies, root);
    assert_eq!(
        enumerated,
        vec!["keep.rs".to_string()],
        "both extra rules must exclude from S(D)"
    );

    // The same entries, answered by the hook's own surface.
    let verdicts = memstead_base::check_path::check_deny_paths(
        &denies,
        &[
            "bare/f.rs".to_string(),
            "broken/f.rs".to_string(),
            "keep.rs".to_string(),
        ],
        root,
        root,
    );
    let denied: Vec<bool> = verdicts.iter().map(|v| v.denied).collect();
    assert_eq!(
        denied,
        vec![true, true, false],
        "the hook and the enumerator must agree path for path"
    );
}

/// The dead-deny lint must answer with the resolution that ENFORCES a
/// deny, not with the raw glob. A bare-name entry (`dev`) blocks through
/// the literal-base directory-prefix rule, so it is live — reporting it as
/// matching nothing told the author to delete an entry whose removal would
/// have raised the denominator, while the same binding's slice and the
/// path-check command both proved it working.
#[test]
fn the_dead_deny_lint_agrees_with_the_resolution_that_enforces() {
    use memstead_base::binding::BuildMode;
    use memstead_base::pipeline::IngestTrigger;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("bare")).unwrap();
    std::fs::write(root.join("bare/f.md"), "").unwrap();
    std::fs::write(root.join("keep.md"), "").unwrap();

    let live = memstead_base::check_path::DenyOracle::new(&["bare".to_string()]);
    assert!(
        live.is_denied("bare/f.md"),
        "the enforcing resolution blocks a bare name by its literal base"
    );

    let resolved = ResolvedIngest {
        name: "ing".to_string(),
        mode: BuildMode::Discovery,
        trigger: IngestTrigger::Loop,
        batch_size: 50,
        deny_paths: vec!["bare".to_string(), "nothing-here/**".to_string()],
        projection_ref: "m/p".to_string(),
        projection_mem: "m".to_string(),
        projection_name: "p".to_string(),
        intent: None,
        sources: vec![ResolvedSource::Primary(primary(vec![PatternEntry {
            path: "**/*.md".to_string(),
            mode: PatternMode::Allow,
        }]))],
        destination_mem: "m".to_string(),
        rules: None,
        post_actions: None,
    };
    let dead = dead_deny_entries(&resolved, root);
    assert_eq!(
        dead,
        vec!["nothing-here/**".to_string()],
        "only the genuinely dead entry is reported; got {dead:?}"
    );
}

/// Partiality has two causes, not one. A malformed pattern is the obvious
/// one; a pattern still in the retired workspace-relative dialect is the
/// dangerous one, because a MIXED scope still enumerates and the surviving
/// subset looks like a population. Both must make `is_partial` true, or a
/// coverage percentage gets computed over a set that is provably short.
#[test]
fn a_legacy_dialect_pattern_makes_the_enumeration_partial() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "").unwrap();
    std::fs::write(root.join("src/b.md"), "").unwrap();

    let mut source = primary(vec![
        PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        },
        // Written against the workspace root, not the pointer: it selects
        // nothing now, and its share of the population is missing.
        PatternEntry {
            path: "src/b.md".to_string(),
            mode: PatternMode::Allow,
        },
    ]);
    source.pointer = "src".to_string();

    let got = enumerate_facet_files_reported(&source, &[], root);
    assert_eq!(got.files, vec!["src/a.rs".to_string()]);
    assert!(got.malformed.is_empty(), "nothing here fails to compile");
    assert!(
        got.is_partial(),
        "a legacy-dialect pattern truncates the set just as a malformed one does"
    );
    let why = got.partiality_reason().expect("a reason is stated");
    assert!(
        why.contains("src/b.md") && why.contains("pointer"),
        "the reason must name the pattern and the cause: {why}"
    );
}

/// Criterion 7's complement, allow half: a malformed allow no longer takes
/// the whole enumeration with it. The valid patterns still select, and the
/// bad one comes back by name so the caller can state the partiality.
#[test]
fn a_malformed_allow_does_not_empty_the_enumeration() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("a.rs"), "").unwrap();

    let source = primary(vec![
        PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        },
        PatternEntry {
            path: "[unclosed".to_string(),
            mode: PatternMode::Allow,
        },
    ]);
    let got = enumerate_facet_files_reported(&source, &[], root);
    assert_eq!(got.files, vec!["a.rs".to_string()]);
    assert_eq!(got.malformed, vec!["[unclosed".to_string()]);
    assert!(got.is_partial(), "a skipped pattern makes the set partial");
}

/// Criterion 7's complement, deny half: a malformed deny no longer
/// disables the other denies and inflates the denominator.
#[test]
fn a_malformed_deny_does_not_disable_the_other_denies() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("skip")).unwrap();
    std::fs::write(root.join("keep.rs"), "").unwrap();
    std::fs::write(root.join("skip/gone.rs"), "").unwrap();

    let source = primary(vec![
        PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        },
        PatternEntry {
            path: "skip/**".to_string(),
            mode: PatternMode::Deny,
        },
        PatternEntry {
            path: "[unclosed".to_string(),
            mode: PatternMode::Deny,
        },
    ]);
    let got = enumerate_facet_files_reported(&source, &[], root);
    assert_eq!(
        got.files,
        vec!["keep.rs".to_string()],
        "the good deny still applies"
    );
    assert_eq!(got.malformed, vec!["[unclosed".to_string()]);
}

/// A binding carrying a malformed scope pattern is refused at validation,
/// naming it — so the enumerator's tolerance above only ever carries
/// records written before this gate existed.
#[test]
fn validation_refuses_a_malformed_scope_pattern_by_name() {
    use memstead_base::binding::CapabilityError;
    let mut binding =
        memstead_base::binding::scaffold_binding(memstead_base::binding::ScaffoldParams {
            destination_mem: "home",
            source_name: "src",
            medium_type: MediumType::Codebase,
            pointer: "src",
            intent: None,
            additional_deny_paths: Vec::new(),
        })
        .binding;
    binding.sources[0].scope.push(PatternEntry {
        path: "[unclosed".to_string(),
        mode: PatternMode::Allow,
    });
    let errs = memstead_base::binding::validate_binding(&binding).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            e,
            CapabilityError::MalformedScopePattern { pattern, .. } if pattern == "[unclosed"
        )),
        "expected MalformedScopePattern naming the pattern, got {errs:?}"
    );
}

#[test]
fn migration_notes_name_old_dialect_patterns_and_suggest_the_rewrite() {
    let mut source = primary(vec![
        PatternEntry {
            path: "../public/plugins/claude-code/**/*.md".to_string(),
            mode: PatternMode::Allow,
        },
        PatternEntry {
            path: "../public/plugins/claude-code/dist/**".to_string(),
            mode: PatternMode::Deny,
        },
        PatternEntry {
            path: "**/*.mjs".to_string(),
            mode: PatternMode::Allow,
        },
    ]);
    source.pointer = "../public/plugins/claude-code".to_string();
    let notes = scope_migration_notes(&source);
    assert_eq!(notes.len(), 2, "the prefix-free pattern is not reported");
    assert_eq!(notes[0].suggested.as_deref(), Some("**/*.md"));
    assert!(!notes[0].deny);
    assert_eq!(notes[1].suggested.as_deref(), Some("dist/**"));
    assert!(notes[1].deny);
}

/// A pointer-less source has no dialect to migrate.
#[test]
fn migration_notes_are_empty_without_a_pointer() {
    let source = primary(vec![PatternEntry {
        path: "notes/**/*.md".to_string(),
        mode: PatternMode::Allow,
    }]);
    assert!(scope_migration_notes(&source).is_empty());
}

#[test]
fn enumerate_honours_allow_and_deny() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.rs"), "").unwrap();
    std::fs::write(root.join("sub/b.rs"), "").unwrap();
    std::fs::write(root.join("c.md"), "").unwrap();

    // medium_pointer "" → base is the workspace root; allow **/*.rs,
    // deny sub/** (so sub/b.rs is excluded, c.md never matched).
    let source = primary(vec![
        PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        },
        PatternEntry {
            path: "sub/**".to_string(),
            mode: PatternMode::Deny,
        },
    ]);
    assert_eq!(enumerate_facet_files(&source, &[], root), vec!["a.rs"]);

    // A graph medium is not a file tree, so the FILE walk yields nothing
    // for it — but that is a statement about this function, not about
    // graph enumerability. `enumerate_graph_entities` is the graph arm,
    // and `enumerate_source_artifacts` is what every S(D) consumer calls.
    let mut graph_source = source.clone();
    graph_source.medium_type = MediumType::Graph;
    assert!(enumerate_facet_files(&graph_source, &[], root).is_empty());
}

/// The allow-prefix pruning never changes WHAT an enumeration selects —
/// only which directories the walk bothers to enter. Anchored patterns
/// (`notes/**/*.md`) still find their files, root-level literals
/// (`VISION.md`) still match, and a subtree no pattern can reach
/// contributes nothing whether walked or pruned.
#[test]
fn enumerate_prunes_directories_outside_every_allow_prefix() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    std::fs::create_dir_all(root.join("notes/drafts")).unwrap();
    std::fs::create_dir_all(root.join("target/debug/build")).unwrap();
    std::fs::write(root.join("notes/drafts/a.md"), "").unwrap();
    std::fs::write(root.join("notes/top.md"), "").unwrap();
    std::fs::write(root.join("VISION.md"), "").unwrap();
    std::fs::write(root.join("target/debug/build/junk.md"), "").unwrap();

    let source = primary(vec![
        PatternEntry {
            path: "notes/**/*.md".to_string(),
            mode: PatternMode::Allow,
        },
        PatternEntry {
            path: "VISION.md".to_string(),
            mode: PatternMode::Allow,
        },
    ]);
    assert_eq!(
        enumerate_facet_files(&source, &[], root),
        vec!["VISION.md", "notes/drafts/a.md", "notes/top.md"],
    );
}

/// The prefix/could-match helpers, on their own terms: segment-exact
/// comparison (no byte-prefix confusion between `no` and `notes`), the
/// empty prefix of an unanchored pattern admitting everything, and both
/// directions of the common-length rule (ancestor of the literal region,
/// inside the glob region).
#[test]
fn allow_prefix_pruning_helpers() {
    assert_eq!(glob_literal_prefix("notes/**/*.md"), vec!["notes"]);
    assert_eq!(glob_literal_prefix("VISION.md"), vec!["VISION.md"]);
    assert!(glob_literal_prefix("**/*.rs").is_empty());
    assert_eq!(glob_literal_prefix("a/b/{c,d}/e"), vec!["a", "b"]);

    let prefixes = vec![vec!["notes".to_string()], vec!["VISION.md".to_string()]];
    assert!(allow_could_match_under(&prefixes, "notes"));
    assert!(allow_could_match_under(&prefixes, "notes/drafts"));
    assert!(!allow_could_match_under(&prefixes, "no"));
    assert!(!allow_could_match_under(&prefixes, "public"));
    assert!(!allow_could_match_under(&prefixes, "target/debug"));

    let unanchored = vec![Vec::<String>::new()];
    assert!(allow_could_match_under(&unanchored, "anything/at/all"));
}

/// Graph enumeration is real: a graph source's `S(D)` is the source mem's
/// in-scope entity set, selected by the entity vocabulary. This is the
/// bail the S1b pilot hit — enumeration returned empty for every graph
/// source, so coverage was vacuously 0/0 and `--full` passed over a
/// measurement that never happened.
#[test]
fn graph_enumeration_selects_the_source_mems_entities() {
    use memstead_base::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use memstead_base::workspace_store::WorkspaceStoreAdapter;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mem_dir = root.join("srcmem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();

    let entity = |slug: &str, ty: &str, title: &str| {
        std::fs::write(
            mem_dir.join(format!("{slug}.md")),
            format!("---\ntype: {ty}\n---\n\n# {title}\n\n## Decision\n\nBody.\n"),
        )
        .unwrap();
    };
    entity("alpha-choice", "decision", "Alpha choice");
    entity("beta-choice", "decision", "Beta choice");
    entity("gamma-note", "memo", "Gamma note");

    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
            &Workspace {
                mounts: vec![Mount {
                    mem: "srcmem".to_string(),
                    schema: Some("default@1.0.0".parse().unwrap()),
                    storage: MountStorage::Folder {
                        path: mem_dir.clone(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: false,
                    migration_target: None,
                }],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();

    let graph_source = |patterns: Vec<(&str, PatternMode)>| Source {
        name: "g".to_string(),
        medium_type: MediumType::Graph,
        pointer: "srcmem".to_string(),
        change_detection: None,
        scope: patterns
            .into_iter()
            .map(|(p, mode)| memstead_base::pipeline::PatternEntry {
                path: p.to_string(),
                mode,
            })
            .collect(),
        engagement: None,
        preparation: None,
    };

    // `*` — the whole mem. A real denominator, not an empty walk.
    let all = enumerate_graph_entities(&engine, &graph_source(vec![("*", PatternMode::Allow)]));
    assert_eq!(
        all,
        vec![
            "srcmem--alpha-choice".to_string(),
            "srcmem--beta-choice".to_string(),
            "srcmem--gamma-note".to_string(),
        ],
        "the whole-mem selector enumerates every real entity"
    );

    // `type:` selects on the type axis.
    let decisions = enumerate_graph_entities(
        &engine,
        &graph_source(vec![("type:decision", PatternMode::Allow)]),
    );
    assert_eq!(
        decisions,
        vec![
            "srcmem--alpha-choice".to_string(),
            "srcmem--beta-choice".to_string()
        ],
        "type selector excludes the memo"
    );

    // `id:` globs the id, and a deny subtracts from an allow.
    let globbed = enumerate_graph_entities(
        &engine,
        &graph_source(vec![
            ("id:srcmem--*-choice", PatternMode::Allow),
            ("id:srcmem--beta-*", PatternMode::Deny),
        ]),
    );
    assert_eq!(
        globbed,
        vec!["srcmem--alpha-choice".to_string()],
        "deny subtracts from allow in the entity namespace too"
    );

    // An unscoped graph facet enumerates nothing — the same posture the
    // path mediums have always had, and the reason the strategy layer
    // refuses it before ever reaching here.
    assert!(
        enumerate_graph_entities(&engine, &graph_source(vec![])).is_empty(),
        "an unscoped graph facet is never silently 'everything'"
    );

    // The dispatching entry point every S(D) consumer calls agrees.
    assert_eq!(
        enumerate_source_artifacts(
            &engine,
            &graph_source(vec![("*", PatternMode::Allow)]),
            &[],
            root
        ),
        all,
        "enumerate_source_artifacts routes a graph source to the graph arm"
    );
}

/// The S1b pilot's headline failure, encoded as a permanent regression
/// test: a stale-pinned entity anchor over a source entity that changed
/// since it was pinned must be flagged `drifted`. It used to go unflagged
/// — anchor resolution was 0/0 for every graph source, so drift was
/// structurally undetectable while the matrix claimed full parity.
#[test]
fn a_stale_entity_anchor_over_a_changed_entity_is_drifted() {
    use memstead_base::anchor::{
        Anchor, AnchorGrain, AnchorProvenanceClass, AnchorSidecar, AnchorState,
    };
    use memstead_base::entity::EntityId;
    use memstead_base::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use memstead_base::workspace_store::WorkspaceStoreAdapter;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mem_dir = root.join("mem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        mem_dir.join("pinned.md"),
        "---\ntype: decision\n---\n\n# Pinned\n\n## Decision\n\nOriginal body.\n",
    )
    .unwrap();
    std::fs::write(
        mem_dir.join("steady.md"),
        "---\ntype: decision\n---\n\n# Steady\n\n## Decision\n\nUnchanged body.\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
            &Workspace {
                mounts: vec![Mount {
                    mem: "mem".to_string(),
                    schema: Some("default@1.0.0".parse().unwrap()),
                    storage: MountStorage::Folder {
                        path: mem_dir.clone(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: false,
                    migration_target: None,
                }],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    // Hash the entities as they stand, so the anchors start out honest.
    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let hash_of = |engine: &memstead_base::Engine, id: &str| {
        let e = engine.store().get(&EntityId::canonical(id)).unwrap();
        memstead_base::anchor::prepared_content_hash(
            memstead_base::render::render_entity_markdown(e, None).as_bytes(),
        )
    };
    let pinned_hash = hash_of(&engine, "mem--pinned");
    let steady_hash = hash_of(&engine, "mem--steady");

    let entity_anchor = |artifact: &str, hash: &str| Anchor {
        artifact: artifact.to_string(),
        grain: AnchorGrain::Entity,
        class: AnchorProvenanceClass::Anchored,
        hash: Some(hash.to_string()),
        source: None,
        binding: None,
        at_version: None,
        derived_from: Vec::new(),
        hash_stability: memstead_base::anchor::AnchorHashStability::Stable,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };

    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "mem--holder",
        vec![
            entity_anchor("mem--pinned", &pinned_hash),
            entity_anchor("mem--steady", &steady_hash),
            // An anchor over an entity that does not exist at all.
            entity_anchor("mem--vanished", "deadbeefdeadbeef"),
        ],
    );
    std::fs::write(
        mem_dir.join(".memstead").join("anchors.json"),
        sidecar.to_bytes(),
    )
    .unwrap();
    std::fs::write(
        mem_dir.join("holder.md"),
        "---\ntype: decision\n---\n\n# Holder\n\n## Decision\n\nHolds anchors.\n",
    )
    .unwrap();

    // Now change ONE source entity — the pilot's move.
    std::fs::write(
        mem_dir.join("pinned.md"),
        "---\ntype: decision\n---\n\n# Pinned\n\n## Decision\n\nBody rewritten.\n",
    )
    .unwrap();

    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let resolved = engine.entity_anchors_resolved(&EntityId::canonical("mem--holder"));
    let state_of = |artifact: &str| {
        resolved
            .iter()
            .find(|r| r.anchor.artifact == artifact)
            .unwrap_or_else(|| panic!("no resolved anchor for {artifact}"))
            .state
    };

    assert_eq!(
        state_of("mem--pinned"),
        Some(AnchorState::Drifted),
        "a stale-pinned anchor over a CHANGED entity must be drifted — \
             this is the pilot failure that went unflagged"
    );
    assert_eq!(
        state_of("mem--steady"),
        Some(AnchorState::Resolves),
        "an anchor over an unchanged entity still resolves"
    );
    assert_eq!(
        state_of("mem--vanished"),
        Some(AnchorState::Orphaned),
        "an anchor over an entity that is not there is orphaned, not unobserved"
    );

    // The complement: a `url` grain genuinely cannot be observed, and must
    // stay unobserved rather than being swept up by the widened arm.
    let mut sc2 = AnchorSidecar::default();
    sc2.set(
        "mem--holder",
        vec![Anchor {
            artifact: "https://example.invalid/doc".to_string(),
            grain: AnchorGrain::Url,
            class: AnchorProvenanceClass::InformedBy,
            hash: None,
            source: None,
            binding: None,
            at_version: None,
            derived_from: Vec::new(),
            hash_stability: memstead_base::anchor::AnchorHashStability::Stable,
            span_unvalidated: false,
            hash_source: None,
            last_observed: None,
        }],
    );
    std::fs::write(
        mem_dir.join(".memstead").join("anchors.json"),
        sc2.to_bytes(),
    )
    .unwrap();
    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let url_state = engine.entity_anchors_resolved(&EntityId::canonical("mem--holder"))[0].state;
    assert_eq!(
        url_state, None,
        "url anchors stay unobserved — the fix widens observation, never the \
             scoring of non-observation"
    );
}

/// Touchpoint A of the preparation registry, end to end: a graph source
/// declaring `entity-load-bearing` makes its entity anchors hash the
/// type's load-bearing sections. A notes-only edit (an optional section)
/// keeps the prepared anchor resolving while an anchor over the default
/// form (no source, hence no preparation) drifts — today's behaviour,
/// untouched for it; a load-bearing edit drifts both. The standalone
/// `verify_mem_anchors` walks the same observation and inherits the
/// preparation unchanged. An unregistered identifier reaching a record
/// by hand computes no form: its anchors stay unobserved, never scored.
#[test]
fn entity_load_bearing_preparation_ignores_notes_edits_and_catches_claim_edits() {
    use memstead_base::anchor::{
        Anchor, AnchorGrain, AnchorProvenanceClass, AnchorSidecar, AnchorState,
    };
    use memstead_base::binding::{
        BINDING_VERSION, Binding, BuildMode, BuildOperation, Operations, VerifyOperation,
    };
    use memstead_base::entity::EntityId;
    use memstead_base::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode, Source};
    use memstead_base::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use memstead_base::workspace_store::WorkspaceStoreAdapter;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mem_dir = root.join("home");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    // `assertion` in default@1.0.0: `claim` and `evidence` are required
    // (the load-bearing set), `conditions` is optional (notes-class).
    let write_pinned = |claim: &str, conditions: &str| {
        std::fs::write(
            mem_dir.join("pinned.md"),
            format!(
                "---\ntype: assertion\n---\n\n# Pinned\n\n## Claim\n\n{claim}\n\n\
                     ## Evidence\n\nMeasured.\n\n## Conditions\n\n{conditions}\n"
            ),
        )
        .unwrap();
    };
    write_pinned("The sky is blue.", "daylight");
    std::fs::write(
        mem_dir.join("holder.md"),
        "---\ntype: assertion\n---\n\n# Holder\n\n## Claim\n\nDepends on pinned.\n\n\
             ## Evidence\n\nSee pinned.\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
            &Workspace {
                mounts: vec![Mount {
                    mem: "home".to_string(),
                    schema: Some("default@1.0.0".parse().unwrap()),
                    storage: MountStorage::Folder {
                        path: mem_dir.clone(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: false,
                    migration_target: None,
                }],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    // The binding: one graph source named `claims`, declaring the
    // registered preparation. Written straight to the store — the shape
    // every edit path validates (`validate_binding`) accepts it.
    let binding_with = |preparation: Option<&str>| Binding {
        version: BINDING_VERSION,
        intent: None,
        sources: vec![Source {
            name: "claims".to_string(),
            medium_type: MediumType::Graph,
            pointer: "home".to_string(),
            change_detection: None,
            scope: vec![PatternEntry {
                path: "*".to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: preparation.map(str::to_string),
        }],
        reference_mems: vec![],
        destination_mem: "home".to_string(),
        deny_paths: vec![],
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: Operations {
            build: Some(BuildOperation {
                mode: BuildMode::Discovery,
                trigger: IngestTrigger::Manual,
                batch_size: 5,
                post_actions: None,
            }),
            sync: None,
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 5,
                adjudication_cap: 0,
                full_resync_every: 0,
            }),
        },
    };
    let prepared_binding = binding_with(Some(memstead_base::preparation::ENTITY_LOAD_BEARING));
    assert!(memstead_base::binding::validate_binding(&prepared_binding).is_ok());
    memstead_base::pipeline_store::write_binding(root, "home", "claims", &prepared_binding)
        .unwrap();

    // Record both anchors honestly against the entity as it stands: one
    // produced by the `claims` source (prepared form), one hand-authored
    // (no source: the default form, the canonical rendered markdown).
    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let pinned = engine
        .store()
        .get(&EntityId::canonical("home--pinned"))
        .unwrap();
    let type_def = engine
        .schema_for("home")
        .and_then(|s| s.get_type("assertion"))
        .expect("default@1.0.0 declares assertion");
    assert!(
        memstead_base::preparation::load_bearing_sections(&type_def)
            .iter()
            .map(|s| s.key.as_str())
            .eq(["claim", "evidence"]),
        "the required sections are the load-bearing set"
    );
    let prepared_hash = memstead_base::preparation::entity_prepared_hash(
        pinned,
        Some(&type_def),
        Some(memstead_base::preparation::ENTITY_LOAD_BEARING),
    )
    .unwrap();
    let default_hash =
        memstead_base::preparation::entity_prepared_hash(pinned, Some(&type_def), None).unwrap();
    assert_ne!(prepared_hash, default_hash);

    let anchor = |source: Option<&str>, hash: &str| Anchor {
        artifact: "home--pinned".to_string(),
        grain: AnchorGrain::Entity,
        class: AnchorProvenanceClass::Anchored,
        hash: Some(hash.to_string()),
        source: source.map(str::to_string),
        binding: None,
        at_version: None,
        derived_from: Vec::new(),
        hash_stability: memstead_base::anchor::AnchorHashStability::Stable,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    // Two holders so the two anchors over one artifact stay distinct rows.
    std::fs::write(
        mem_dir.join("holder2.md"),
        "---\ntype: assertion\n---\n\n# Holder2\n\n## Claim\n\nAlso depends.\n\n\
             ## Evidence\n\nSee pinned.\n",
    )
    .unwrap();
    let mut sidecar = AnchorSidecar::default();
    sidecar.set("home--holder", vec![anchor(Some("claims"), &prepared_hash)]);
    sidecar.set("home--holder2", vec![anchor(None, &default_hash)]);
    std::fs::write(
        mem_dir.join(".memstead").join("anchors.json"),
        sidecar.to_bytes(),
    )
    .unwrap();

    let states = |root: &std::path::Path| {
        let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
        let state_of =
            |holder: &str| engine.entity_anchors_resolved(&EntityId::canonical(holder))[0].state;
        let standalone = engine.verify_mem_anchors("home").unwrap();
        (
            state_of("home--holder"),
            state_of("home--holder2"),
            standalone,
        )
    };

    // Unchanged: both resolve.
    let (prepared, plain, report) = states(root);
    assert_eq!(prepared, Some(AnchorState::Resolves));
    assert_eq!(plain, Some(AnchorState::Resolves));
    assert_eq!(
        (report.figure.count_for_assertions(), report.drifted),
        (2, 0)
    );

    // A notes-only edit (`conditions` is not load-bearing): the prepared
    // anchor holds, the default-form anchor drifts — today's behaviour,
    // byte-for-byte, for a source that declares nothing.
    write_pinned("The sky is blue.", "daylight, clear weather");
    let (prepared, plain, report) = states(root);
    assert_eq!(
        prepared,
        Some(AnchorState::Resolves),
        "a comma in the notes must not break a load-bearing anchor"
    );
    assert_eq!(plain, Some(AnchorState::Drifted));
    assert_eq!(
        (report.figure.count_for_assertions(), report.drifted),
        (1, 1)
    );

    // A load-bearing edit: both drift.
    write_pinned("The sky is green.", "daylight, clear weather");
    let (prepared, plain, report) = states(root);
    assert_eq!(prepared, Some(AnchorState::Drifted));
    assert_eq!(plain, Some(AnchorState::Drifted));
    assert_eq!(
        (report.figure.count_for_assertions(), report.drifted),
        (0, 2)
    );

    // Complement: a hand-edited record naming an identifier the registry
    // does not know computes no form — its anchors are unobserved, never
    // scored as drift or resolution; the source-less anchor is unaffected.
    memstead_base::pipeline_store::write_binding(
        root,
        "home",
        "claims",
        &binding_with(Some("pdf-to-markdown")),
    )
    .unwrap();
    let (prepared, plain, report) = states(root);
    assert_eq!(
        prepared, None,
        "an unknown preparation yields no observation"
    );
    assert_eq!(plain, Some(AnchorState::Drifted));
    // The unknown-preparation anchor yields NO observation, so it is
    // `unobserved`, not `unresolvable`.
    // Before the split this assertion read `(1, 1)` on
    // `unresolvable`, which is the collapse itself: the pass not reaching
    // an artifact was reported as the artifact being gone.
    assert_eq!(
        (report.unresolvable, report.unobserved, report.drifted),
        (0, 1, 1)
    );
}

/// Touchpoint B's order is a property of the units, not of discovery: a
/// shuffled collection sorts into the identical sequence.
#[test]
fn shuffled_discovery_sequences_identically() {
    use memstead_base::preparation::UnitChange;
    let unit = |id: &str, order: &str| DeliveredUnit {
        id: id.to_string(),
        order_key: order.to_string(),
        change: UnitChange::Added,
        disposed: false,
    };
    let ordered = vec![
        unit("corpus/notes.md#whole", ""),
        unit("corpus/b.md#2026-08-20T00:00:00", "2026-08-20T00:00:00"),
        unit("corpus/a.md#2026-08-21T00:00:00", "2026-08-21T00:00:00"),
        unit("corpus/a.md#2026-08-21T00:00:00.2", "2026-08-21T00:00:00"),
        unit("corpus/c.md#2026-08-21T00:00:00", "2026-08-21T00:00:00"),
        unit("corpus/b.md#2026-08-22T00:00:00", "2026-08-22T00:00:00"),
    ];
    for shuffle in [
        vec![5, 3, 0, 4, 1, 2],
        vec![2, 1, 0, 5, 4, 3],
        vec![4, 0, 5, 2, 3, 1],
    ] {
        let mut units: Vec<DeliveredUnit> = shuffle.iter().map(|i| ordered[*i].clone()).collect();
        sequence_units(&mut units);
        assert_eq!(units, ordered, "discovery order {shuffle:?} must not leak");
    }

    // A date-only day with twelve entries: the same-stamp ordinal orders
    // numerically, never as text (`.10` after `.9`, not before `.2`).
    let day = "2026-08-24T00:00:00";
    let expected: Vec<String> = (1..=12)
        .map(|n| {
            if n == 1 {
                format!("journal.md#{day}")
            } else {
                format!("journal.md#{day}.{n}")
            }
        })
        .collect();
    let mut units: Vec<DeliveredUnit> = expected.iter().rev().map(|id| unit(id, day)).collect();
    sequence_units(&mut units);
    assert_eq!(
        units.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
}

/// Touchpoint B end to end over a git corpus whose path order is not its
/// chronological order: the first run delivers every unit in stamp order
/// interleaved across files; the sibling source without a preparation
/// keeps file-granularity delivery; disposing advances through the
/// sequence, an anchor over exactly a unit auto-disposes it while a
/// file-level anchor does not; the change run delivers only the new,
/// changed and removed units at their ordered positions (keys stable
/// under growth); and a span anchor over a unit observes the unit, not
/// the file.
#[test]
fn dated_entries_deliver_in_a_total_order_across_first_and_change_runs() {
    use crate::advance::{DispositionInput, advance_baseline};
    use crate::brief::render_changed_slice;
    use memstead_base::anchor::{
        Anchor, AnchorGrain, AnchorProvenanceClass, AnchorSidecar, AnchorState,
    };
    use memstead_base::binding::{
        BINDING_VERSION, Binding, BuildMode, BuildOperation, Operations, VerifyOperation,
    };
    use memstead_base::binding_run::resolve_binding_run;
    use memstead_base::entity::EntityId;
    use memstead_base::pipeline::{IngestTrigger, PatternMode};
    use memstead_base::preparation::{DATED_ENTRIES, UnitChange, unitize};
    use memstead_base::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use memstead_base::workspace_store::WorkspaceStoreAdapter;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Destination: a folder mem `home` with one holder entity for anchors.
    let mem_dir = root.join("home");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        mem_dir.join("holder.md"),
        "---\ntype: assertion\n---\n\n# Holder\n\n## Claim\n\nHolds anchors.\n\n\
             ## Evidence\n\nSee corpus.\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
            &Workspace {
                mounts: vec![Mount {
                    mem: "home".to_string(),
                    schema: Some("default@1.0.0".parse().unwrap()),
                    storage: MountStorage::Folder {
                        path: mem_dir.clone(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: false,
                    migration_target: None,
                }],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    // Source: a git corpus whose lexical path order (a, b, notes) is not
    // its chronological order.
    let corpus = root.join("corpus");
    std::fs::create_dir_all(corpus.join("plain")).unwrap();
    git(&corpus, &["init", "-q"]);
    let write = |name: &str, text: &str| std::fs::write(corpus.join(name), text).unwrap();
    write(
        "a.md",
        "2026-08-21 alpha one\nbody a1\n2026-08-23 alpha two\nbody a2\n",
    );
    write(
        "b.md",
        "2026-08-20 beta one\nbody b1\n2026-08-22 beta two\nbody b2\n",
    );
    write("notes.md", "undated notes\n");
    write("plain/readme.txt", "plain source, file granularity\n");
    git(&corpus, &["add", "."]);
    git(&corpus, &["commit", "-q", "-m", "corpus"]);

    let source = |name: &str, scope: &str, preparation: Option<&str>| Source {
        name: name.to_string(),
        medium_type: MediumType::Filesystem,
        pointer: "corpus".to_string(),
        change_detection: Some("git".to_string()),
        scope: vec![PatternEntry {
            path: scope.to_string(),
            mode: PatternMode::Allow,
        }],
        engagement: None,
        preparation: preparation.map(str::to_string),
    };
    let binding = Binding {
        version: BINDING_VERSION,
        intent: None,
        sources: vec![
            // Source-relative against the `corpus` pointer.
            source("logs", "*.md", Some(DATED_ENTRIES)),
            source("plain", "plain/**", None),
        ],
        reference_mems: vec![],
        destination_mem: "home".to_string(),
        deny_paths: vec![],
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: Operations {
            build: Some(BuildOperation {
                mode: BuildMode::Discovery,
                trigger: IngestTrigger::Manual,
                batch_size: 3,
                post_actions: None,
            }),
            sync: None,
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 5,
                adjudication_cap: 0,
                full_resync_every: 0,
            }),
        },
    };
    assert!(
        memstead_base::binding::validate_binding(&binding).is_ok(),
        "{:?}",
        memstead_base::binding::validate_binding(&binding)
    );
    memstead_base::pipeline_store::write_binding(root, "home", "corpus", &binding).unwrap();
    let resolved = resolve_binding_run("home/corpus", &binding).unwrap();

    // ---- First run: every unit, in stamp order, interleaved across files.
    let mut engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let cursor = compute_source_cursor(&engine, &resolved, root);
    let expected: Vec<&str> = vec![
        "corpus/notes.md#whole",
        "corpus/b.md#2026-08-20T00:00:00",
        "corpus/a.md#2026-08-21T00:00:00",
        "corpus/b.md#2026-08-22T00:00:00",
        "corpus/a.md#2026-08-23T00:00:00",
    ];
    assert_eq!(
        cursor.delivery.len(),
        1,
        "one sequence, for the prepared source only"
    );
    let seq = &cursor.delivery[0];
    assert_eq!(
        (seq.source.as_str(), seq.preparation.as_str()),
        ("logs", DATED_ENTRIES)
    );
    assert!(seq.first_run && !seq.degraded && seq.batch == 3);
    assert_eq!(
        seq.units.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        expected
    );
    assert!(
        seq.units
            .iter()
            .all(|u| u.change == UnitChange::Added && !u.disposed)
    );
    let mut expected_sorted: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    expected_sorted.sort();
    assert_eq!(
        cursor.union.added, expected_sorted,
        "the advance gate accepts the unit ids"
    );
    // The sibling source without a preparation keeps file granularity:
    // a plain first-run reseed, no units, no sequence.
    assert!(
        cursor
            .reseed
            .iter()
            .any(|c| c.key == "home/corpus/plain#synced")
    );
    assert!(
        cursor
            .write_commands
            .iter()
            .any(|c| c.key == "home/corpus/logs#synced")
    );
    assert!(
        !cursor
            .union
            .added
            .iter()
            .any(|a| a.starts_with("corpus/plain"))
    );
    // Recomputing yields the identical sequence.
    assert_eq!(compute_source_cursor(&engine, &resolved, root), cursor);

    let brief = render_changed_slice(&cursor);
    assert!(
        brief.contains("### Delivery sequence: `logs` (`dated-entries`)"),
        "{brief}"
    );
    assert!(brief.contains("First delivery of this source"));
    let listed: Vec<&str> = brief
        .lines()
        .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()) && l.contains("`corpus/"))
        .collect();
    assert_eq!(
        listed,
        vec![
            "1. `corpus/notes.md#whole` (new)",
            "2. `corpus/b.md#2026-08-20T00:00:00` (new)",
            "3. `corpus/a.md#2026-08-21T00:00:00` (new)",
        ],
        "the batch presents the first three in order"
    );
    assert!(brief.contains("…and 2 more, presented in order once these are disposed"));
    assert!(
        !brief.contains("**Added:**"),
        "unit ids never repeat in a class list: {brief}"
    );

    // ---- Advance through the sequence.
    let dispositions: BTreeMap<String, DispositionInput> =
        [(expected[0], "skipped"), (expected[1], "worked")]
            .into_iter()
            .map(|(a, d)| (a.to_string(), DispositionInput::Verdict(d.to_string())))
            .collect();
    let outcome = advance_baseline(&mut engine, root, &resolved, &dispositions).unwrap();
    assert_eq!((outcome.pending, outcome.completed), (3, false));
    let cursor = compute_source_cursor(&engine, &resolved, root);
    assert!(cursor.delivery[0].units[0].disposed && cursor.delivery[0].units[1].disposed);
    let brief = render_changed_slice(&cursor);
    assert!(
        brief.contains("3. `corpus/a.md#2026-08-21T00:00:00` (new)"),
        "{brief}"
    );
    assert!(
        !brief.contains("1. `corpus/notes.md#whole`"),
        "disposed units are not re-presented"
    );
    assert!(brief.contains("2 units of this sequence already disposed"));

    // An anchor over exactly a unit disposes it; a file-level anchor over
    // `b.md` disposes none of b's units.
    let a21_text = std::fs::read_to_string(corpus.join("a.md")).unwrap();
    let a21_unit = unitize(DATED_ENTRIES, &a21_text)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "2026-08-21T00:00:00")
        .unwrap();
    let anchor = |artifact: &str, grain: AnchorGrain, hash: &str| Anchor {
        artifact: artifact.to_string(),
        grain,
        class: AnchorProvenanceClass::Anchored,
        hash: Some(hash.to_string()),
        source: Some("logs".to_string()),
        binding: None,
        at_version: None,
        derived_from: Vec::new(),
        hash_stability: memstead_base::anchor::AnchorHashStability::Stable,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    let b_file_hash =
        memstead_base::anchor::prepared_content_hash(&std::fs::read(corpus.join("b.md")).unwrap());
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "home--holder",
        vec![
            anchor(expected[2], AnchorGrain::Span, &a21_unit.hash),
            anchor("corpus/b.md", AnchorGrain::File, &b_file_hash),
        ],
    );
    std::fs::write(
        mem_dir.join(".memstead").join("anchors.json"),
        sidecar.to_bytes(),
    )
    .unwrap();
    let mut engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let outcome = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new()).unwrap();
    assert_eq!(
        outcome.pending, 2,
        "the unit anchor auto-disposed its unit, the file anchor nothing"
    );
    assert!(outcome.remainder.added.contains(&expected[3].to_string()));
    assert!(outcome.remainder.added.contains(&expected[4].to_string()));
    let rest: BTreeMap<String, DispositionInput> = [expected[3], expected[4]]
        .into_iter()
        .map(|a| {
            (
                a.to_string(),
                DispositionInput::Verdict("worked".to_string()),
            )
        })
        .collect();
    let outcome = advance_baseline(&mut engine, root, &resolved, &rest).unwrap();
    assert!(outcome.completed, "{outcome:?}");
    assert!(
        outcome
            .tokens_written
            .contains(&"home/corpus/logs#synced".to_string())
    );

    // ---- Change run: one earlier entry appended to a.md, b's second
    // entry edited, b's first entry removed. Only those three units are
    // delivered, at their ordered positions; every other key survives.
    write(
        "a.md",
        "2026-08-21 alpha one\nbody a1\n2026-08-23 alpha two\nbody a2\n2026-08-19 alpha zero\nbody a0\n",
    );
    write("b.md", "2026-08-22 beta two\nbody b2, revised\n");
    git(&corpus, &["add", "."]);
    git(&corpus, &["commit", "-q", "-m", "grow, edit, remove"]);
    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let cursor = compute_source_cursor(&engine, &resolved, root);
    let seq = &cursor.delivery[0];
    assert!(!seq.first_run && !seq.degraded);
    assert_eq!(
        seq.units
            .iter()
            .map(|u| (u.id.as_str(), u.change))
            .collect::<Vec<_>>(),
        vec![
            ("corpus/a.md#2026-08-19T00:00:00", UnitChange::Added),
            ("corpus/b.md#2026-08-20T00:00:00", UnitChange::Deleted),
            ("corpus/b.md#2026-08-22T00:00:00", UnitChange::Modified),
        ]
    );
    let brief = render_changed_slice(&cursor);
    assert!(brief.contains("The units that changed since the last pass"));
    assert!(
        brief.contains("1. `corpus/a.md#2026-08-19T00:00:00` (new)"),
        "{brief}"
    );
    assert!(brief.contains("3. `corpus/b.md#2026-08-22T00:00:00` (changed)"));

    // ---- Touchpoint A over units: the span anchor on a.md's unchanged
    // unit still resolves although its file changed; an anchor on the
    // removed unit orphans; one on the edited unit drifts.
    let b22_old_text = "2026-08-22 beta two\nbody b2\n";
    let b22_old = unitize(DATED_ENTRIES, b22_old_text).unwrap()[0]
        .hash
        .clone();
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "home--holder",
        vec![
            anchor(expected[2], AnchorGrain::Span, &a21_unit.hash),
            anchor(expected[1], AnchorGrain::Span, "deadbeefdeadbeef"),
            anchor(expected[3], AnchorGrain::Span, &b22_old),
        ],
    );
    std::fs::write(
        mem_dir.join(".memstead").join("anchors.json"),
        sidecar.to_bytes(),
    )
    .unwrap();
    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let resolved_anchors = engine.entity_anchors_resolved(&EntityId::canonical("home--holder"));
    let state_of = |artifact: &str| {
        resolved_anchors
            .iter()
            .find(|r| r.anchor.artifact == artifact)
            .unwrap()
            .state
    };
    assert_eq!(
        state_of(expected[2]),
        Some(AnchorState::Resolves),
        "unit unchanged, file changed"
    );
    assert_eq!(
        state_of(expected[1]),
        Some(AnchorState::Orphaned),
        "unit removed"
    );
    assert_eq!(
        state_of(expected[3]),
        Some(AnchorState::Drifted),
        "unit edited"
    );
}

/// Touchpoint A's code-map flavour end to end: anchors on a code-map
/// source hash interface digests, so a comment, formatting or body edit
/// leaves file, span and tree anchors resolving while a signature edit
/// drifts them; a file joining the tree drifts the tree anchor alone;
/// the sibling source without a preparation keeps whole-file hashing
/// and its tree anchor hashes the plain per-file digest (so it resolves
/// and drifts deterministically like everything else — the once-stated
/// unhashed remainder is closed); and a write-time `content` on a
/// code-map source records the digest hash, never the raw one.
#[test]
fn code_map_anchors_drift_on_interface_changes_only() {
    use memstead_base::anchor::{
        Anchor, AnchorGrain, AnchorInput, AnchorProvenanceClass, AnchorSidecar, AnchorState,
    };
    use memstead_base::binding::{
        BINDING_VERSION, Binding, BuildMode, BuildOperation, Operations, VerifyOperation,
    };
    use memstead_base::binding_run::Source;
    use memstead_base::entity::EntityId;
    use memstead_base::pipeline::{IngestTrigger, PatternMode};
    use memstead_base::preparation::{CODE_MAP, code_map_digest, code_map_tree_digest};
    use memstead_base::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use memstead_base::workspace_store::WorkspaceStoreAdapter;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mem_dir = root.join("home");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        mem_dir.join("holder.md"),
        "---\ntype: assertion\n---\n\n# Holder\n\n## Claim\n\nHolds anchors.\n\n\
             ## Evidence\n\nSee code.\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
            &Workspace {
                mounts: vec![Mount {
                    mem: "home".to_string(),
                    schema: Some("default@1.0.0".parse().unwrap()),
                    storage: MountStorage::Folder {
                        path: mem_dir.clone(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: false,
                    migration_target: None,
                }],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    // The corpus: a source tree under code-map, a sibling tree without.
    let corpus = root.join("corpus");
    std::fs::create_dir_all(corpus.join("src")).unwrap();
    std::fs::create_dir_all(corpus.join("plain")).unwrap();
    const A: &str = "// auth\nimport axios from 'axios'\n\nexport async function login(user, password) {\n  // call\n  return axios.post('/login', { user, password })\n}\n";
    const B: &str = "// the limit\nexport const LIMIT = 10\n";
    let write = |rel: &str, text: &str| std::fs::write(corpus.join(rel), text).unwrap();
    write("src/a.js", A);
    write("src/b.js", B);
    write("plain/notes.js", "export const N = 1\n");

    let source = |name: &str, scope: &str, preparation: Option<&str>| Source {
        name: name.to_string(),
        medium_type: MediumType::Codebase,
        pointer: "corpus".to_string(),
        change_detection: Some("none".to_string()),
        scope: vec![PatternEntry {
            path: scope.to_string(),
            mode: PatternMode::Allow,
        }],
        engagement: None,
        preparation: preparation.map(str::to_string),
    };
    let binding = Binding {
        version: BINDING_VERSION,
        intent: None,
        sources: vec![
            // Scope patterns are SOURCE-relative: the pointer is
            // `corpus`, so these read as `corpus/src/**/*.js` and
            // `corpus/plain/**` on disk.
            source("code", "src/**/*.js", Some(CODE_MAP)),
            source("plain", "plain/**", None),
        ],
        reference_mems: vec![],
        destination_mem: "home".to_string(),
        deny_paths: vec![],
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: Operations {
            build: Some(BuildOperation {
                mode: BuildMode::Discovery,
                trigger: IngestTrigger::Manual,
                batch_size: 5,
                post_actions: None,
            }),
            sync: None,
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 5,
                adjudication_cap: 0,
                full_resync_every: 0,
            }),
        },
    };
    assert!(memstead_base::binding::validate_binding(&binding).is_ok());
    memstead_base::pipeline_store::write_binding(root, "home", "code", &binding).unwrap();

    let digest_hash = |rel: &str, text: &str| {
        memstead_base::anchor::prepared_content_hash(code_map_digest(rel, text).as_bytes())
    };
    let tree_hash = |files: &[(&str, &str)]| {
        let owned: Vec<(String, String)> = files
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect();
        memstead_base::anchor::prepared_content_hash(code_map_tree_digest(&owned).as_bytes())
    };
    let anchor = |artifact: &str, grain: AnchorGrain, source: &str, hash: &str| Anchor {
        artifact: artifact.to_string(),
        grain,
        class: AnchorProvenanceClass::Anchored,
        hash: Some(hash.to_string()),
        source: Some(source.to_string()),
        binding: None,
        at_version: None,
        derived_from: Vec::new(),
        hash_stability: memstead_base::anchor::AnchorHashStability::Stable,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    let plain_raw = memstead_base::anchor::prepared_content_hash(b"export const N = 1\n");
    let plain_tree = memstead_base::anchor::prepared_content_hash(
        memstead_base::preparation::plain_tree_digest(&[(
            "corpus/plain/notes.js".to_string(),
            b"export const N = 1\n".to_vec(),
        )])
        .as_bytes(),
    );
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "home--holder",
        vec![
            anchor(
                "corpus/src/a.js",
                AnchorGrain::File,
                "code",
                &digest_hash("corpus/src/a.js", A),
            ),
            anchor(
                "corpus/src/a.js#L4-L7",
                AnchorGrain::Span,
                "code",
                &digest_hash("corpus/src/a.js", A),
            ),
            anchor(
                "corpus/src",
                AnchorGrain::Tree,
                "code",
                &tree_hash(&[("corpus/src/a.js", A), ("corpus/src/b.js", B)]),
            ),
            anchor(
                "corpus/plain/notes.js",
                AnchorGrain::File,
                "plain",
                &plain_raw,
            ),
            anchor("corpus/plain", AnchorGrain::Tree, "plain", &plain_tree),
        ],
    );
    std::fs::write(
        mem_dir.join(".memstead").join("anchors.json"),
        sidecar.to_bytes(),
    )
    .unwrap();

    let states = |root: &std::path::Path| {
        let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
        let resolved = engine.entity_anchors_resolved(&EntityId::canonical("home--holder"));
        let of = |artifact: &str| {
            let r = resolved
                .iter()
                .find(|r| r.anchor.artifact == artifact)
                .unwrap_or_else(|| panic!("no anchor {artifact}"));
            (r.state, r.observed_hash.clone())
        };
        (
            of("corpus/src/a.js").0,
            of("corpus/src/a.js#L4-L7").0,
            of("corpus/src").0,
            of("corpus/plain/notes.js").0,
            of("corpus/plain"),
            engine.verify_mem_anchors("home").unwrap(),
        )
    };

    // Unchanged: everything resolves — the plain tree included, since
    // its prepared form is the plain per-file digest of its scoped files
    // (the once-stated unhashed remainder is closed).
    let (file, span, tree, plain_file, plain_tree_state, report) = states(root);
    assert_eq!(
        (file, span, tree, plain_file),
        (
            Some(AnchorState::Resolves),
            Some(AnchorState::Resolves),
            Some(AnchorState::Resolves),
            Some(AnchorState::Resolves)
        )
    );
    assert_eq!(
        plain_tree_state,
        (Some(AnchorState::Resolves), Some(plain_tree.clone()))
    );
    assert_eq!(
        (
            report.figure.count_for_assertions(),
            report.drifted,
            report.recheck
        ),
        (5, 0, 0)
    );

    // Comment, formatting and body edits: invisible.
    write(
        "src/a.js",
        "// auth (rewritten comment)\nimport axios from 'axios'\n\nexport async function login(user, password) {\n    return await axios.post('/session',   { user, password })\n}\n",
    );
    let (file, span, tree, _, _, report) = states(root);
    assert_eq!(
        (file, span, tree),
        (
            Some(AnchorState::Resolves),
            Some(AnchorState::Resolves),
            Some(AnchorState::Resolves)
        ),
        "a body edit must not drift a code-map anchor"
    );
    assert_eq!(report.drifted, 0);

    // A signature edit: file, span and tree drift.
    write(
        "src/a.js",
        "// auth\nimport axios from 'axios'\n\nexport async function login(user, password, remember) {\n  return axios.post('/login', { user, password, remember })\n}\n",
    );
    let (file, span, tree, plain_file, _, report) = states(root);
    assert_eq!(
        (file, span, tree),
        (
            Some(AnchorState::Drifted),
            Some(AnchorState::Drifted),
            Some(AnchorState::Drifted)
        )
    );
    assert_eq!(plain_file, Some(AnchorState::Resolves));
    assert_eq!(report.drifted, 3);

    // Restore, then a new file joins the tree: the tree drifts alone.
    write("src/a.js", A);
    write("src/c.js", "export const C = 1\n");
    let (file, span, tree, _, _, _) = states(root);
    assert_eq!(
        (file, span),
        (Some(AnchorState::Resolves), Some(AnchorState::Resolves))
    );
    assert_eq!(
        tree,
        Some(AnchorState::Drifted),
        "a file joining the tree changes its code map"
    );

    // The plain source is untouched by the code map: a body edit in its
    // file drifts the whole-file hash exactly as before — and now the
    // plain TREE anchor too, since any scoped-file byte change moves the
    // plain per-file digest.
    write("plain/notes.js", "export const N = 1 // note\n");
    let (_, _, _, plain_file, plain_tree_state, _) = states(root);
    assert_eq!(plain_file, Some(AnchorState::Drifted));
    assert_eq!(
        plain_tree_state.0,
        Some(AnchorState::Drifted),
        "a body edit in a scoped file drifts the plain tree anchor"
    );

    // Write time: `content` on a code-map source records the digest hash.
    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let input = AnchorInput {
        artifact: Some("corpus/src/b.js".to_string()),
        grain: Some("file".to_string()),
        class: Some("anchored".to_string()),
        source: Some("code".to_string()),
        content: Some(B.to_string()),
        ..Default::default()
    };
    let validated = engine.validate_anchor_inputs("home", &[input]).unwrap();
    assert_eq!(
        validated[0].hash.as_deref(),
        Some(digest_hash("corpus/src/b.js", B).as_str())
    );
    assert_ne!(
        validated[0].hash.as_deref(),
        Some(memstead_base::anchor::prepared_content_hash(B.as_bytes()).as_str())
    );
    let plain_input = AnchorInput {
        artifact: Some("corpus/plain/notes.js".to_string()),
        grain: Some("file".to_string()),
        class: Some("anchored".to_string()),
        source: Some("plain".to_string()),
        content: Some("export const N = 1\n".to_string()),
        ..Default::default()
    };
    let validated = engine
        .validate_anchor_inputs("home", &[plain_input])
        .unwrap();
    assert_eq!(
        validated[0].hash.as_deref(),
        Some(plain_raw.as_str()),
        "no preparation: the raw canonicalization, as before"
    );
}

/// Criterion 6's regression pin: the graph change-detection half is
/// untouched except for the deliberate unscoped gate. A SCOPED graph
/// facet still routes to the graph strategy and reports the same
/// no-signal reason it always did when the source mem exposes no
/// snapshot token (a folder mem tracks no head); an UNSCOPED one now
/// refuses as `Unscoped`, exactly as the git and mtime arms have always
/// done. Distinguishing the two is the whole point — before this, an
/// unscoped graph facet silently proceeded.
#[test]
fn graph_scoping_changes_only_the_unscoped_arm() {
    use memstead_base::binding::BuildMode;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mem_dir = root.join("srcmem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        mem_dir.join("one.md"),
        "---\ntype: decision\n---\n\n# One\n\n## Decision\n\nBody.\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    {
        use memstead_base::workspace::{
            Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
        };
        use memstead_base::workspace_store::WorkspaceStoreAdapter;
        memstead_base::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![Mount {
                        mem: "srcmem".to_string(),
                        schema: Some("default@1.0.0".parse().unwrap()),
                        storage: MountStorage::Folder {
                            path: mem_dir.clone(),
                        },
                        capability: MountCapability::Write,
                        lifecycle: MountLifecycle::Eager,
                        cross_linkable: false,
                        migration_target: None,
                    }],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();
    }
    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();

    let resolved_with = |scope: Vec<memstead_base::pipeline::PatternEntry>| ResolvedIngest {
        name: "srcmem/p".to_string(),
        mode: BuildMode::Discovery,
        trigger: memstead_base::pipeline::IngestTrigger::Manual,
        batch_size: 20,
        deny_paths: Vec::new(),
        projection_ref: "srcmem/p".to_string(),
        projection_mem: "srcmem".to_string(),
        projection_name: "p".to_string(),
        intent: None,
        sources: vec![ResolvedSource::Primary(Source {
            name: "g".to_string(),
            medium_type: MediumType::Graph,
            pointer: "srcmem".to_string(),
            change_detection: None,
            scope,
            engagement: None,
            preparation: None,
        })],
        destination_mem: "srcmem".to_string(),
        rules: None,
        post_actions: None,
    };

    let scoped = compute_source_cursor(
        &engine,
        &resolved_with(vec![memstead_base::pipeline::PatternEntry {
            path: "*".to_string(),
            mode: PatternMode::Allow,
        }]),
        root,
    );
    let unscoped = compute_source_cursor(&engine, &resolved_with(Vec::new()), root);

    let reason_of = |c: &SourceCursor| c.no_signal.first().map(|n| n.reason);
    assert_eq!(
        reason_of(&scoped),
        Some(NoSignalReason::GraphSnapshotMissing),
        "a scoped graph facet still routes to the graph strategy and reports \
             its own no-signal reason — the change-detection half is untouched"
    );
    assert_eq!(
        reason_of(&unscoped),
        Some(NoSignalReason::Unscoped),
        "an unscoped graph facet refuses like every other medium's, instead of \
             silently proceeding"
    );
}

/// The git medium enumerates through the same path walk as codebase and
/// filesystem — its artifacts are paths pinned at a commit, so the walk is
/// identical and only the anchor namespace differs. It was excluded from
/// that arm for no reason beyond the arm's shape, which made its
/// `enumerable: true` row a claim nothing delivered. Pinned so a refactor
/// cannot quietly drop it back out.
#[test]
fn git_medium_enumerates_through_the_path_walk() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    std::fs::write(root.join("a.rs"), "").unwrap();
    std::fs::write(root.join("b.rs"), "").unwrap();

    let source = |medium: MediumType| Source {
        name: "s".to_string(),
        medium_type: medium,
        pointer: ".".to_string(),
        change_detection: None,
        scope: vec![memstead_base::pipeline::PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }],
        engagement: None,
        preparation: None,
    };

    let want = vec!["a.rs".to_string(), "b.rs".to_string()];
    for medium in [
        MediumType::Codebase,
        MediumType::Filesystem,
        MediumType::Git,
    ] {
        assert_eq!(
            enumerate_facet_files(&source(medium), &[], root),
            want,
            "{medium:?} walks the file tree — every medium the matrix marks \
                 enumerable with a path namespace must actually enumerate"
        );
        assert!(
            memstead_base::binding::medium_capabilities(medium).enumerable,
            "{medium:?} claims enumerability, and now delivers it"
        );
    }
}

/// A narrowing selector must bound the CHANGED SLICE, not only `S(D)`.
/// It used to bound only enumeration, so a brief could print
/// `Entities: type:concept` and then present a changed `memo` two
/// sections below — an artifact its own coverage model calls out of
/// scope, which `advance` would accept because its gate is the presented
/// slice. Scope interpreted in one place and decorative in the other is
/// the defect this pins closed.
#[test]
fn a_narrowing_selector_bounds_the_changed_slice_too() {
    use memstead_base::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use memstead_base::workspace_store::WorkspaceStoreAdapter;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mem_dir = root.join("srcmem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let write = |slug: &str, ty: &str| {
        std::fs::write(
            mem_dir.join(format!("{slug}.md")),
            format!("---\ntype: {ty}\n---\n\n# {slug}\n\n## Decision\n\nBody.\n"),
        )
        .unwrap();
    };
    write("kept", "decision");
    write("other", "memo");
    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
            &Workspace {
                mounts: vec![Mount {
                    mem: "srcmem".to_string(),
                    schema: Some("default@1.0.0".parse().unwrap()),
                    storage: MountStorage::Folder {
                        path: mem_dir.clone(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: false,
                    migration_target: None,
                }],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();
    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();

    let source = Source {
        name: "g".to_string(),
        medium_type: MediumType::Graph,
        pointer: "srcmem".to_string(),
        change_detection: None,
        scope: vec![memstead_base::pipeline::PatternEntry {
            path: "type:decision".to_string(),
            mode: PatternMode::Allow,
        }],
        engagement: None,
        preparation: None,
    };

    let mut slice = Slice {
        added: vec!["srcmem--other".to_string()],
        modified: vec!["srcmem--kept".to_string(), "srcmem--other".to_string()],
        deleted: vec!["srcmem--vanished".to_string()],
    };
    filter_graph_slice_to_scope(&engine, &source, &mut slice);

    assert_eq!(
        slice.modified,
        vec!["srcmem--kept".to_string()],
        "the out-of-scope memo is dropped from the slice the brief presents"
    );
    assert!(
        slice.added.is_empty(),
        "an added out-of-scope entity is out of scope too"
    );
    assert_eq!(
        slice.deleted,
        vec!["srcmem--vanished".to_string()],
        "a DELETED entity is kept even though its type can no longer be \
             read — a deletion that cannot be classified must be reported, \
             never silently dropped"
    );

    // The complement: the whole-mem selector narrows nothing.
    let mut wide = Slice {
        added: Vec::new(),
        modified: vec!["srcmem--kept".to_string(), "srcmem--other".to_string()],
        deleted: Vec::new(),
    };
    let mut all = source.clone();
    all.scope = vec![memstead_base::pipeline::PatternEntry {
        path: "*".to_string(),
        mode: PatternMode::Allow,
    }];
    filter_graph_slice_to_scope(&engine, &all, &mut wide);
    assert_eq!(wide.modified.len(), 2, "`*` selects the whole mem");
}

/// The entity-selector grammar is closed: three legal forms, everything
/// else refused. A pattern that parses to `None` is a validation refusal
/// at declaration — never a rule that silently selects nothing.
#[test]
fn entity_selector_grammar_is_closed() {
    use super::EntitySelector;
    assert_eq!(parse_entity_selector("*"), Some(EntitySelector::All));
    assert_eq!(
        parse_entity_selector("type:decision"),
        Some(EntitySelector::Type("decision".to_string()))
    );
    assert_eq!(
        parse_entity_selector("id:engine--*"),
        Some(EntitySelector::Id("engine--*".to_string()))
    );
    // The path glob `projection init` used to scaffold for graph sources:
    // it looks like scope and selects nothing. Refused, not accepted.
    assert_eq!(parse_entity_selector("**/*"), None);
    assert_eq!(parse_entity_selector("src/**"), None);
    assert_eq!(parse_entity_selector("type:"), None);
    assert_eq!(parse_entity_selector("id:"), None);
    assert_eq!(parse_entity_selector(""), None);
}

/// The mtime driver reseeds on the first pass (writing the memo), then
/// diffs precisely against the memoised map — including deletions.
#[test]
fn mtime_driver_reseeds_then_diffs_precisely() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let cache = root.join(".memstead.cache").join("ingest");
    std::fs::write(root.join("a.rs"), "one").unwrap();
    std::fs::write(root.join("gone.rs"), "bye").unwrap();
    let source = primary(vec![PatternEntry {
        path: "**/*.rs".to_string(),
        mode: PatternMode::Allow,
    }]);

    // First pass: no baseline → reseed at the current digest, memo written.
    let token = match compute_mtime_slice(&source, "ing", &[], root, &cache, None) {
        SliceOutcome::Reseed { token } => token,
        other => panic!("expected Reseed, got {other:?}"),
    };

    // Move the source: modify a.rs (size change), delete gone.rs, add new.rs.
    std::fs::write(root.join("a.rs"), "one-longer").unwrap();
    std::fs::remove_file(root.join("gone.rs")).unwrap();
    std::fs::write(root.join("new.rs"), "x").unwrap();

    // Second pass with the reseed token → precise diff from the memo.
    match compute_mtime_slice(&source, "ing", &[], root, &cache, Some(&token)) {
        SliceOutcome::Changed {
            slice, degraded, ..
        } => {
            assert!(
                !degraded,
                "memo present → precise, not a degraded full scan"
            );
            assert_eq!(slice.added, vec!["new.rs"]);
            assert_eq!(slice.modified, vec!["a.rs"]);
            assert_eq!(
                slice.deleted,
                vec!["gone.rs"],
                "deletions come from the memo"
            );
        }
        other => panic!("expected Changed, got {other:?}"),
    }

    // A run whose baseline aggregate is not memoised degrades to a full
    // scan (every current file as added, no deletions).
    let stale = super::super::change_detection::serialize_digest_token(
        &super::super::change_detection::digest_stat_map(&stat_map_for(&["absent.rs"])),
    );
    match compute_mtime_slice(&source, "ing", &[], root, &cache, Some(&stale)) {
        SliceOutcome::Changed { degraded, .. } => assert!(degraded, "memo miss → degraded"),
        other => panic!("expected degraded Changed, got {other:?}"),
    }
}

fn head_sha(repo: &Path) -> String {
    String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

fn slice_contains(slice: &Slice, path: &str) -> bool {
    let p = path.to_string();
    slice.added.contains(&p) || slice.modified.contains(&p) || slice.deleted.contains(&p)
}

/// The mtime `source_moved` / `current_primary_token` value: the digest
/// token over the deny-filtered enumeration — exactly what the mtime branch
/// of `current_primary_token` computes.
fn mtime_token(source: &Source, deny: &[String], root: &Path) -> String {
    let files = enumerate_facet_files(source, deny, root);
    serialize_digest_token(&digest_stat_map(&compute_stat_map(&files, root)))
}

/// AC1 (deny invariance): a file matching an ingest `deny_paths` entry
/// appears in **no** changed slice (git, mtime), **no** refinement batch,
/// and does **not** influence the mtime digest / `source_moved` token —
/// exercising the *same* denied file across every strategy that reads a
/// file tree.
#[test]
fn deny_paths_excluded_from_every_strategy_and_token() {
    use crate::refinement::next_batch;
    use memstead_base::binding::BuildMode;
    use memstead_base::pipeline::IngestTrigger;

    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    let cache = root.join(".memstead.cache").join("ingest");

    // One tree that is both the git work tree and the mtime/refinement
    // workspace root (medium_pointer "" → base == root).
    git(root, &["init", "-q"]);
    std::fs::write(root.join("keep.rs"), "one").unwrap();
    std::fs::write(root.join("denied.rs"), "secret-one").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = head_sha(root);

    // Both files genuinely move — denied.rs must never surface anywhere.
    std::fs::write(root.join("keep.rs"), "two").unwrap();
    std::fs::write(root.join("denied.rs"), "secret-two").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "move"]);

    // Scope allows every .rs; the ingest denies denied.rs by the same
    // workspace-relative glob grammar the git strategy uses.
    let source = primary(vec![PatternEntry {
        path: "**/*.rs".to_string(),
        mode: PatternMode::Allow,
    }]);
    let deny = vec!["denied.rs".to_string()];

    // (1) git slice — with the deny, only keep.rs.
    match compute_git_slice(&source, &deny, root, Some(&baseline)) {
        SliceOutcome::Changed { slice, .. } => {
            assert_eq!(slice.modified, vec!["keep.rs"]);
            assert!(!slice_contains(&slice, "denied.rs"), "git deny leak");
        }
        other => panic!("git: expected Changed, got {other:?}"),
    }
    // Control: without the deny, denied.rs *is* a real change — proving the
    // deny (not the scope) is what excludes it above.
    match compute_git_slice(&source, &[], root, Some(&baseline)) {
        SliceOutcome::Changed { slice, .. } => {
            assert!(
                slice_contains(&slice, "denied.rs"),
                "un-denied, denied.rs is a genuine git change"
            );
        }
        other => panic!("git(no-deny): expected Changed, got {other:?}"),
    }

    // (2) enumeration (mtime input set + refinement source set).
    assert_eq!(enumerate_facet_files(&source, &deny, root), vec!["keep.rs"]);
    assert!(
        enumerate_facet_files(&source, &[], root).contains(&"denied.rs".to_string()),
        "un-denied, denied.rs is enumerated"
    );

    // (2b) mtime slice — reseed, then move both files; only keep.rs surfaces.
    let token = match compute_mtime_slice(&source, "ing", &deny, root, &cache, None) {
        SliceOutcome::Reseed { token } => token,
        other => panic!("mtime reseed expected, got {other:?}"),
    };
    std::fs::write(root.join("keep.rs"), "three-longer").unwrap();
    std::fs::write(root.join("denied.rs"), "secret-three-longer").unwrap();
    match compute_mtime_slice(&source, "ing", &deny, root, &cache, Some(&token)) {
        SliceOutcome::Changed { slice, .. } => {
            assert_eq!(slice.modified, vec!["keep.rs"]);
            assert!(!slice_contains(&slice, "denied.rs"), "mtime deny leak");
        }
        other => panic!("mtime: expected Changed, got {other:?}"),
    }

    // (3) mtime digest / source_moved token — invariant to denied.rs, since
    // the token is the digest over the deny-filtered enumeration. Removing
    // denied.rs from disk leaves the token unchanged; a leak would show it
    // as a deletion and shift the digest.
    let token_present = mtime_token(&source, &deny, root);
    std::fs::remove_file(root.join("denied.rs")).unwrap();
    let token_absent = mtime_token(&source, &deny, root);
    assert_eq!(
        token_present, token_absent,
        "denied.rs must not influence the mtime digest / source_moved token"
    );
    std::fs::write(root.join("denied.rs"), "secret-restored").unwrap();

    // (4) refinement batch — the denied file is never batched.
    let resolved = ResolvedIngest {
        name: "ing".to_string(),
        mode: BuildMode::Discovery,
        trigger: IngestTrigger::Loop,
        batch_size: 50,
        deny_paths: deny.clone(),
        projection_ref: "m/p".to_string(),
        projection_mem: "m".to_string(),
        projection_name: "p".to_string(),
        intent: None,
        sources: vec![ResolvedSource::Primary(source.clone())],
        destination_mem: "m".to_string(),
        rules: None,
        post_actions: None,
    };
    let engine = memstead_base::Engine::from_mounts(Vec::new()).unwrap();
    let batch = next_batch(&engine, &resolved, root, &cache, 20).unwrap();
    assert!(
        batch.files.contains(&"keep.rs".to_string()),
        "keep.rs batched"
    );
    assert!(
        !batch.files.contains(&"denied.rs".to_string()),
        "denied.rs must never enter a refinement batch"
    );
}

/// AC2 (one empty-scope semantic): an **unscoped** facet (no allow
/// patterns) is the same typed refusal — `NoSignal { Unscoped }` — on git
/// AND mtime, never a silent empty slice. AC2 complement: an empty
/// `deny_paths` list does NOT trip that refusal — a *scoped* facet still
/// classifies normally (empty scope and empty deny_paths are different
/// fields with different semantics).
#[test]
fn unscoped_facet_refuses_uniformly_and_empty_deny_is_distinct() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    let cache = root.join(".memstead.cache").join("ingest");
    git(root, &["init", "-q"]);
    std::fs::write(root.join("a.rs"), "one").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = head_sha(root);
    std::fs::write(root.join("a.rs"), "two").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "move"]);

    // Unscoped: a deny pattern but no allow. `deny_paths` is empty here —
    // so the refusal comes from the empty *scope*, not from denies.
    let unscoped = primary(vec![PatternEntry {
        path: "target/**".to_string(),
        mode: PatternMode::Deny,
    }]);
    assert_eq!(
        compute_git_slice(&unscoped, &[], root, Some(&baseline)),
        SliceOutcome::NoSignal {
            reason: NoSignalReason::Unscoped
        },
        "git refuses an unscoped facet"
    );
    assert_eq!(
        compute_mtime_slice(&unscoped, "ing", &[], root, &cache, None),
        SliceOutcome::NoSignal {
            reason: NoSignalReason::Unscoped
        },
        "mtime refuses an unscoped facet identically"
    );
    // A fully empty scope is unscoped too.
    let empty_scope = primary(vec![]);
    assert_eq!(
        compute_git_slice(&empty_scope, &[], root, Some(&baseline)),
        SliceOutcome::NoSignal {
            reason: NoSignalReason::Unscoped
        }
    );

    // Complement: a SCOPED facet with an empty `deny_paths` classifies
    // normally — empty deny_paths (no denies) must not trip the refusal.
    let scoped = primary(vec![PatternEntry {
        path: "**/*.rs".to_string(),
        mode: PatternMode::Allow,
    }]);
    assert!(
        matches!(
            compute_git_slice(&scoped, &[], root, Some(&baseline)),
            SliceOutcome::Changed { .. }
        ),
        "scoped facet + empty deny_paths → normal git slice, not a refusal"
    );
    assert!(
        matches!(
            compute_mtime_slice(&scoped, "ing", &[], root, &cache, None),
            SliceOutcome::Reseed { .. }
        ),
        "scoped facet + empty deny_paths → normal mtime reseed, not a refusal"
    );
}

/// AC2 refinement leg: an ingest whose only source is unscoped emits no
/// refinement batch — the refusal, not a silent empty batch.
#[test]
fn unscoped_facet_emits_no_refinement_batch() {
    use crate::refinement::next_batch;
    use memstead_base::binding::BuildMode;
    use memstead_base::pipeline::IngestTrigger;

    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let cache = root.join(".memstead.cache").join("ingest");
    std::fs::write(root.join("a.rs"), "x").unwrap();

    let resolved = ResolvedIngest {
        name: "ing".to_string(),
        mode: BuildMode::Discovery,
        trigger: IngestTrigger::Loop,
        batch_size: 50,
        deny_paths: vec![],
        projection_ref: "m/p".to_string(),
        projection_mem: "m".to_string(),
        projection_name: "p".to_string(),
        intent: None,
        // Only source: an unscoped facet (no allow patterns).
        sources: vec![ResolvedSource::Primary(primary(vec![]))],
        destination_mem: "m".to_string(),
        rules: None,
        post_actions: None,
    };
    assert!(
        next_batch(
            &memstead_base::Engine::from_mounts(Vec::new()).unwrap(),
            &resolved,
            root,
            &cache,
            20
        )
        .is_none(),
        "an all-unscoped ingest emits no refinement batch"
    );
}

/// AC3 (visible NoSignal) end-to-end through the cursor: a `signal:none`
/// source and an unscoped source each contribute a distinct no-signal note;
/// a first-seen (reseed) source does NOT — only no-signal reasons are
/// noted. The rendered preface names `signal:none` explicitly and the
/// unscoped reason distinctly.
#[test]
fn compute_source_cursor_notes_no_signal_reasons() {
    use memstead_base::binding::BuildMode;
    use memstead_base::pipeline::IngestTrigger;

    let engine = memstead_base::Engine::from_mounts(Vec::new()).unwrap();
    // No `.git` over the workspace → mtime strategy for `auto`/`mtime`.
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    std::fs::write(root.join("a.rs"), "x").unwrap();

    let allow_rs = || {
        vec![PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }]
    };
    let src = |facet: &str, declared: &str, scope: Vec<PatternEntry>| {
        ResolvedSource::Primary(Source {
            name: facet.to_string(),
            medium_type: MediumType::Filesystem,
            pointer: String::new(),
            change_detection: Some(declared.to_string()),
            scope,
            engagement: None,
            preparation: None,
        })
    };

    let resolved = ResolvedIngest {
        name: "ing".to_string(),
        mode: BuildMode::Discovery,
        trigger: IngestTrigger::Loop,
        batch_size: 20,
        deny_paths: vec![],
        projection_ref: "m/p".to_string(),
        projection_mem: "m".to_string(),
        projection_name: "p".to_string(),
        intent: None,
        sources: vec![
            // signal:none → DetectionNone note (even though it is scoped).
            src("plan", "none", allow_rs()),
            // mtime + no allows → Unscoped note.
            src("blind", "mtime", vec![]),
            // mtime + allows, first-seen → Reseed, NOT a no-signal note.
            src("watched", "mtime", allow_rs()),
        ],
        destination_mem: "m".to_string(),
        rules: None,
        post_actions: None,
    };

    let cursor = compute_source_cursor(&engine, &resolved, root);
    let reasons: BTreeMap<&str, NoSignalReason> = cursor
        .no_signal
        .iter()
        .map(|n| (n.source.as_str(), n.reason))
        .collect();
    assert_eq!(reasons.get("plan"), Some(&NoSignalReason::DetectionNone));
    assert_eq!(reasons.get("blind"), Some(&NoSignalReason::Unscoped));
    assert!(
        !reasons.contains_key("watched"),
        "a first-seen (reseed) source is not a no-signal note"
    );
    assert_eq!(cursor.no_signal.len(), 2);
    // The reseed source still produced a reseed command.
    assert!(cursor.reseed.iter().any(|c| c.key == "ing/watched#synced"));

    // The rendered preface names signal:none and the unscoped reason.
    let out = crate::brief::render_changed_slice(&cursor);
    assert!(out.contains("- `plan`: `signal:none`"));
    assert!(out.contains("- `blind`: unscoped facet"));
}

fn stat_map_for(paths: &[&str]) -> super::super::change_detection::StatMap {
    paths
        .iter()
        .map(|p| {
            (
                (*p).to_string(),
                super::super::change_detection::StatEntry { mtime: 1, size: 1 },
            )
        })
        .collect()
}

/// Engine self-exclusion: `.memstead/**`, `.memstead.cache/**`, and
/// every mount's resolved storage location (here a mem-repo at a
/// NON-default directory name) are absent from the enumeration
/// regardless of configuration — explicit allow globs covering them
/// do not admit them.
#[test]
fn engine_state_never_enumerates_even_when_allowed() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    for rel in [
        ".memstead/state/findings/muehle/f.json",
        ".memstead/projections/muehle/f.json",
        ".memstead.cache/ingest/source-cursor/muehle/f/f.json",
        "custom-repo/README.md",
        "Allgemein/Protokoll.md",
        "Allgemein/Vertrag.md",
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "x").unwrap();
    }
    // Engine-managed workspace state resolving the mem-repo at
    // `custom-repo/` — the exclusion must key on this resolved
    // location, not on the literal default name `mem-repo/`.
    std::fs::write(
        root.join(".memstead/workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".memstead/state/mounts.json"),
        serde_json::json!({
            "format": "memstead-mounts-3",
            "mounts": [{
                "mem": "muehle",
                "schema": "default@1.0.0",
                "storage": {
                    "type": "git-branch",
                    "gitdir": "custom-repo/.git",
                    "branch": "refs/heads/muehle"
                },
                "capability": "write",
                "lifecycle": "eager",
                "cross_linkable": true
            }]
        })
        .to_string(),
    )
    .unwrap();

    // Allow everything AND explicitly try to admit engine state.
    let source = primary(vec![
        PatternEntry {
            path: "**/*".to_string(),
            mode: PatternMode::Allow,
        },
        PatternEntry {
            path: ".memstead/**".to_string(),
            mode: PatternMode::Allow,
        },
        PatternEntry {
            path: "custom-repo/**".to_string(),
            mode: PatternMode::Allow,
        },
    ]);
    let got = enumerate_facet_files(&source, &[], root);
    assert_eq!(
        got,
        vec!["Allgemein/Protokoll.md", "Allgemein/Vertrag.md"],
        "only source artifacts may enter the denominator"
    );
}

/// The git strategy pushes the same engine-state excludes as
/// pathspecs: a diff touching `.memstead/**` and the resolved
/// mem-repo path yields a slice naming neither — denominator and
/// slice stay strategy-invariant.
#[test]
fn git_slice_excludes_engine_state() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    std::fs::write(
        root.join("workspace.rs"), // placeholder so base commit is non-empty
        "x",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".memstead/state")).unwrap();
    std::fs::write(
        root.join(".memstead/workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".memstead/state/mounts.json"),
        serde_json::json!({
            "format": "memstead-mounts-3",
            "mounts": [{
                "mem": "muehle",
                "schema": "default@1.0.0",
                "storage": {
                    "type": "git-branch",
                    "gitdir": "custom-repo/.git",
                    "branch": "refs/heads/muehle"
                },
                "capability": "write",
                "lifecycle": "eager",
                "cross_linkable": true
            }]
        })
        .to_string(),
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    // Move: one real file, one engine-state file, one mem-repo file.
    std::fs::write(root.join("real.md"), "signal").unwrap();
    std::fs::write(root.join(".memstead/state/findings.json"), "self").unwrap();
    std::fs::create_dir_all(root.join("custom-repo")).unwrap();
    std::fs::write(root.join("custom-repo/README.md"), "repo").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "move"]);

    let source = primary(vec![PatternEntry {
        path: "**/*".to_string(),
        mode: PatternMode::Allow,
    }]);
    match compute_git_slice(&source, &[], root, Some(&baseline)) {
        SliceOutcome::Changed { slice, .. } => {
            assert_eq!(
                slice.added,
                vec!["real.md"],
                "engine state leaked: {slice:?}"
            );
            assert!(slice.modified.is_empty(), "{slice:?}");
        }
        other => panic!("expected Changed, got {other:?}"),
    }
}

/// The dead-deny lint never flags the scaffold's own default hygiene
/// entries (they can never match on a git-enumerated source — the
/// engine must not call its own output a typo), while a user-authored
/// entry that matches nothing keeps the loud warning and one that
/// matches stays silent.
#[test]
fn dead_deny_lint_exempts_scaffold_defaults_but_not_user_typos() {
    use memstead_base::binding::{BuildMode, DEFAULT_SCAFFOLD_DENY_PATHS};
    use memstead_base::pipeline::IngestTrigger;

    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "code").unwrap();

    let mut deny_paths: Vec<String> = DEFAULT_SCAFFOLD_DENY_PATHS
        .iter()
        .map(|s| s.to_string())
        .collect();
    deny_paths.push("typo/**".to_string()); // user typo — matches nothing
    deny_paths.push("src/**".to_string()); // user entry that matches

    let resolved = ResolvedIngest {
        name: "ing".to_string(),
        mode: BuildMode::Discovery,
        trigger: IngestTrigger::Loop,
        batch_size: 20,
        deny_paths,
        projection_ref: "m/p".to_string(),
        projection_mem: "m".to_string(),
        projection_name: "p".to_string(),
        intent: None,
        sources: vec![ResolvedSource::Primary(primary(vec![PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }]))],
        destination_mem: "m".to_string(),
        rules: None,
        post_actions: None,
    };

    let dead = dead_deny_entries(&resolved, root);
    assert_eq!(
        dead,
        vec!["typo/**".to_string()],
        "only the user typo is flagged — scaffold defaults and matching \
             entries stay silent"
    );
}
