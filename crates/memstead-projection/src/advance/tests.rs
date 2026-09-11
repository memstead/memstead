#![cfg(test)]

use super::*;
use memstead_base::binding::BuildMode;
use memstead_base::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode};
use memstead_base::storage::FilesystemBackend;
use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
use tempfile::TempDir;

// ── pure helpers ─────────────────────────────────────────────────────

fn slice(added: &[&str], modified: &[&str], deleted: &[&str]) -> Slice {
    Slice {
        added: added.iter().map(|s| s.to_string()).collect(),
        modified: modified.iter().map(|s| s.to_string()).collect(),
        deleted: deleted.iter().map(|s| s.to_string()).collect(),
    }
}

fn disp(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(a, d)| (a.to_string(), d.to_string()))
        .collect()
}

/// The [`DispositionInput`] map an `advance_baseline` call takes: bare
/// verdicts (the common form).
fn input(pairs: &[(&str, &str)]) -> BTreeMap<String, DispositionInput> {
    pairs
        .iter()
        .map(|(a, d)| (a.to_string(), DispositionInput::Verdict(d.to_string())))
        .collect()
}

/// The store round-trips and `delete` is idempotent.
#[test]
fn advance_store_round_trips_and_delete_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    assert!(
        read_advance_store(root, "engine", "graph")
            .unwrap()
            .is_none()
    );

    let state = AdvanceState {
        binding: "engine/graph".to_string(),
        frozen_slice: slice(&["c.rs"], &["a.rs"], &["b.rs"]),
        dispositions: disp(&[("a.rs", "worked")]),
        exclusions: BTreeMap::new(),
        ..Default::default()
    };
    write_advance_store(root, "engine", "graph", &state).unwrap();
    assert!(
        advance_store_path(root, "engine", "graph").ends_with("state/advance/engine/graph.json")
    );
    let back = read_advance_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    assert_eq!(back, state);

    delete_advance_store(root, "engine", "graph").unwrap();
    assert!(
        read_advance_store(root, "engine", "graph")
            .unwrap()
            .is_none()
    );
    // Idempotent: deleting an absent store is a no-op, not an error.
    delete_advance_store(root, "engine", "graph").unwrap();
}

/// An entity-only exclusion ledger counts as durable content: the two
/// guards that may delete the store consult it through this predicate.
#[test]
fn entity_exclusions_alone_keep_the_store_durable() {
    let mut state = AdvanceState {
        binding: "engine/graph".to_string(),
        ..Default::default()
    };
    assert!(!state.has_durable_exclusions());
    state
        .entity_exclusions
        .insert("engine--about".to_string(), "no source".to_string());
    assert!(state.has_durable_exclusions());
    state.entity_exclusions.clear();
    state
        .exclusions
        .insert("gen.rs".to_string(), "generated".to_string());
    assert!(state.has_durable_exclusions());
}

/// `subtract_disposed` removes disposed ids from every class.
#[test]
fn subtract_disposed_removes_disposed_from_every_class() {
    let frozen = slice(&["c.rs"], &["a.rs"], &["b.rs"]);
    let out = subtract_disposed(&frozen, &disp(&[("a.rs", "worked"), ("c.rs", "skipped")]));
    assert_eq!(out, slice(&[], &[], &["b.rs"]));
}

// ── AC9 — full engine advance over a moving HEAD ─────────────────────

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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

/// A discovery-mode resolved binding whose one primary source is a git
/// codebase rooted at the workspace root (medium pointer `""`), scoped to
/// `**/*.rs`, keyed `engine/graph` → dest mem `engine`.
fn resolved_engine_graph() -> ResolvedIngest {
    use memstead_base::binding_run::{ResolvedSource, Source};
    ResolvedIngest {
        name: "engine/graph".to_string(),
        mode: BuildMode::Discovery,
        trigger: IngestTrigger::Loop,
        batch_size: 20,
        deny_paths: vec![],
        projection_ref: "engine/graph".to_string(),
        projection_mem: "engine".to_string(),
        projection_name: "graph".to_string(),
        intent: None,
        sources: vec![ResolvedSource::Primary(Source {
            name: "source-tree".to_string(),
            medium_type: MediumType::Codebase,
            pointer: String::new(),
            change_detection: Some("git".to_string()),
            scope: vec![PatternEntry {
                path: "**/*.rs".to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: None,
        })],
        destination_mem: "engine".to_string(),
        rules: None,
        post_actions: None,
    }
}

/// Build an engine over one writable folder mem `engine` rooted at `root`
/// (which is also the git source tree), with a `.memstead/config.json` so
/// `sync_state` can be read/written.
fn engine_at(root: &Path) -> Engine {
    // Seed the mem config **once** — a later rebuild must not clobber the
    // `sync_state` a prior engine persisted (that is what makes the
    // resumability leg meaningful: each `engine_at` models a fresh process).
    let config_path = root.join(".memstead").join("config.json");
    if !config_path.exists() {
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(&config_path, br#"{"format":1,"schema":"default@1.0.0"}"#).unwrap();
    }
    let mount = Mount {
        mem: "engine".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::Folder {
            path: root.to_path_buf(),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    Engine::from_mounts(vec![(
        mount,
        Box::new(FilesystemBackend::new(root.to_path_buf()))
            as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap()
}

fn synced_key() -> &'static str {
    "engine/graph/source-tree#synced"
}

/// AC9 — `projection advance` is non-stalling under a moving HEAD, and its
/// gate + resumability hold:
///
/// 1. freeze a slice, dispose part → the remainder is the rest;
/// 2. an unknown artifact id refuses the whole call **atomically** (the
///    store is byte-identical after the refusal);
/// 3. a fresh process (new engine) honors the on-disk dispositions
///    (resumability is on-disk, not in-memory);
/// 4. the source HEAD advances mid-pass → the re-presented slice equals
///    (old remainder + new deltas) with disposed artifacts absent;
/// 5. disposing the rest empties the remainder → the `#synced` token
///    advances via the engine writer to the current HEAD.
#[test]
fn advance_is_non_stalling_under_a_moving_head_with_gate_and_resumability() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Source git tree: baseline commit with a.rs + b.rs.
    git(root, &["init", "-q"]);
    std::fs::write(root.join("a.rs"), "one").unwrap();
    std::fs::write(root.join("b.rs"), "bee").unwrap();
    git(root, &["add", "a.rs", "b.rs"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = head_sha(root);

    // Move to head1: modify a.rs, delete b.rs.
    std::fs::write(root.join("a.rs"), "one-longer").unwrap();
    std::fs::remove_file(root.join("b.rs")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "head1"]);

    let resolved = resolved_engine_graph();

    // Seed the `#synced` baseline so the source shows a real moved slice.
    {
        let mut engine = engine_at(root);
        engine
            .set_mem_sync_state("engine", synced_key(), &baseline, None)
            .unwrap();
    }

    // (1) Freeze + dispose part (a.rs). Remainder = the rest (b.rs deleted).
    {
        let mut engine = engine_at(root);
        let out =
            advance_baseline(&mut engine, root, &resolved, &input(&[("a.rs", "worked")])).unwrap();
        assert!(!out.completed, "one artifact still pending");
        assert_eq!(out.remainder, slice(&[], &[], &["b.rs"]));
        assert_eq!(out.pending, 1);
        assert_eq!(out.disposed, 1);
    }
    // The dispositions persisted to disk.
    let on_disk = read_advance_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    assert_eq!(on_disk.dispositions, disp(&[("a.rs", "worked")]));

    // (2) An unknown artifact id refuses the whole call atomically — the
    // store is byte-identical afterwards (no partial write).
    let before = std::fs::read(advance_store_path(root, "engine", "graph")).unwrap();
    {
        let mut engine = engine_at(root);
        let err = advance_baseline(
            &mut engine,
            root,
            &resolved,
            &input(&[("never-presented.rs", "worked")]),
        )
        .unwrap_err();
        assert!(
            matches!(err, AdvanceError::UnknownArtifact { .. }),
            "expected UnknownArtifact, got {err:?}"
        );
    }
    let after = std::fs::read(advance_store_path(root, "engine", "graph")).unwrap();
    assert_eq!(before, after, "refused call must not touch the store");

    // (4) Source moves mid-pass → add c.rs at head2.
    std::fs::write(root.join("c.rs"), "cee").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "head2"]);

    // (3)+(4) A fresh engine (new process) honors the on-disk a.rs
    // disposition, and re-presents (old remainder [b.rs] + new delta [c.rs])
    // with the disposed a.rs absent. Empty dispositions = pure re-present.
    {
        let mut engine = engine_at(root);
        let out = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new()).unwrap();
        assert!(!out.completed);
        assert_eq!(
            out.remainder,
            slice(&["c.rs"], &[], &["b.rs"]),
            "re-present = old remainder (b.rs) + new delta (c.rs); disposed a.rs absent"
        );
        assert_eq!(out.disposed, 1, "no new disposition this call");
    }

    // (5) Dispose the rest → remainder empties → the token advances.
    let head2 = head_sha(root);
    {
        let mut engine = engine_at(root);
        let out = advance_baseline(
            &mut engine,
            root,
            &resolved,
            &input(&[("b.rs", "worked"), ("c.rs", "worked")]),
        )
        .unwrap();
        assert!(out.completed, "every artifact disposed → complete");
        assert_eq!(out.pending, 0);
        assert_eq!(out.tokens_written, vec![synced_key().to_string()]);

        // The `#synced` baseline advanced to the current HEAD (head2).
        let token = engine
            .mem_config_for("engine")
            .and_then(|c| c.sync_state.get(synced_key()).cloned());
        assert_eq!(token.as_deref(), Some(head2.as_str()));
    }
    // The durable store was dropped on completion.
    assert!(
        read_advance_store(root, "engine", "graph")
            .unwrap()
            .is_none()
    );
}

/// The durable authored-exclusion ledger survives completion (unlike the
/// transient dispositions/frozen slice), and a later non-excluded verdict for
/// the same artifact clears it — dropping the store when nothing durable is
/// left. This is the persistence the fidelity report relies on so an
/// excluded-on-purpose artifact stops re-surfacing as `uncovered`.
#[test]
fn advance_holds_authored_exclusions_against_bare_verdicts_and_lifts_on_reasoned_rejudge() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Baseline a.rs; move to head1 (modify a.rs) so the slice = {modified a.rs}.
    git(root, &["init", "-q"]);
    std::fs::write(root.join("a.rs"), "one").unwrap();
    git(root, &["add", "a.rs"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = head_sha(root);
    std::fs::write(root.join("a.rs"), "one-longer").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "head1"]);

    let resolved = resolved_engine_graph();
    {
        let mut engine = engine_at(root);
        engine
            .set_mem_sync_state("engine", synced_key(), &baseline, None)
            .unwrap();
    }

    // Dispose a.rs as EXCLUDED with a rationale → the only slice artifact is
    // disposed → the advance completes. Exclusions are non-empty, so the
    // store is RETAINED (not dropped) holding only the exclusion.
    let excluded = {
        let mut m = BTreeMap::new();
        m.insert(
            "a.rs".to_string(),
            DispositionInput::Reasoned {
                disposition: EXCLUDED_VERDICT.to_string(),
                rationale: "mined; warrants no destination entity".to_string(),
            },
        );
        m
    };
    {
        let mut engine = engine_at(root);
        let out = advance_baseline(&mut engine, root, &resolved, &excluded).unwrap();
        assert!(out.completed, "the sole slice artifact was disposed");
    }
    let retained = read_advance_store(root, "engine", "graph")
        .unwrap()
        .expect("an authored exclusion keeps the store alive past completion");
    assert!(
        retained.frozen_slice == Slice::default() && retained.dispositions.is_empty(),
        "transient progress is dropped on completion"
    );
    assert_eq!(
        retained.exclusions.get("a.rs").map(String::as_str),
        Some("mined; warrants no destination entity"),
        "the durable exclusion + its rationale persist"
    );

    // Move to head2 (modify a.rs again) → a.rs re-enters the slice. A bare
    // `worked` over the ledgered artifact REFUSES before any write, naming
    // the artifact and its recorded rationale (the 2026-09-10 case: a
    // blanket `worked` silently dropped the row, and the artifact came
    // back as `uncovered` with its reasoning gone).
    std::fs::write(root.join("a.rs"), "one-longer-still").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "head2"]);
    {
        let mut engine = engine_at(root);
        let err = advance_baseline(&mut engine, root, &resolved, &input(&[("a.rs", "worked")]))
            .unwrap_err();
        match err {
            AdvanceError::ExclusionHeld { artifacts } => assert_eq!(
                artifacts,
                vec![(
                    "a.rs".to_string(),
                    "mined; warrants no destination entity".to_string()
                )]
            ),
            other => panic!("expected ExclusionHeld, got {other:?}"),
        }
    }
    let after_refusal = read_advance_store(root, "engine", "graph")
        .unwrap()
        .expect("the refusal left the store untouched");
    assert!(
        after_refusal.dispositions.is_empty(),
        "a refused call writes no disposition"
    );
    assert_eq!(
        after_refusal.exclusions.get("a.rs").map(String::as_str),
        Some("mined; warrants no destination entity"),
        "the exclusion row survives the refused call"
    );

    // With no verdict named, the ledger disposes the artifact: the pass
    // completes on the standing row and the exclusion persists.
    {
        let mut engine = engine_at(root);
        let out = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new()).unwrap();
        assert!(out.completed, "the ledger disposed the sole slice artifact");
    }
    let still_excluded = read_advance_store(root, "engine", "graph")
        .unwrap()
        .expect("the exclusion keeps the store alive");
    assert_eq!(
        still_excluded.exclusions.get("a.rs").map(String::as_str),
        Some("mined; warrants no destination entity"),
        "an undisposed ledgered artifact keeps its row"
    );

    // Move to head3 → a REASONED `worked` lifts the exclusion: the
    // re-judgement carries its own rationale, and with nothing durable
    // left the store is dropped.
    std::fs::write(root.join("a.rs"), "one-longer-still-and-more").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "head3"]);
    let lifted = {
        let mut m = BTreeMap::new();
        m.insert(
            "a.rs".to_string(),
            DispositionInput::Reasoned {
                disposition: "worked".to_string(),
                rationale: "now defines a concept the model names".to_string(),
            },
        );
        m
    };
    {
        let mut engine = engine_at(root);
        let out = advance_baseline(&mut engine, root, &resolved, &lifted).unwrap();
        assert!(out.completed);
    }
    assert!(
        read_advance_store(root, "engine", "graph")
            .unwrap()
            .is_none(),
        "the reasoned re-judgement lifted the exclusion; nothing durable remains"
    );
}

/// Criterion: a **medium-relative** artifact id (the form agents naturally
/// type — `a.rs` when the engine printed `sub/a.rs`) refuses with a typed,
/// remedy-bearing message that names the workspace-relative dialect and
/// the concrete corrected id when derivable. REFUSALS: the gate never
/// widens — the medium-relative form is never accepted, nothing is
/// written; an unknown id with no derivable correction carries no
/// suggestion.
#[test]
fn advance_unknown_artifact_names_dialect_and_suggests_corrected_id() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Source files live under the medium subtree `sub/` — artifact ids in
    // the slice are workspace-relative (`sub/a.rs`).
    git(root, &["init", "-q"]);
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("a.rs"), "one").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = head_sha(root);
    std::fs::write(root.join("sub").join("a.rs"), "one-longer").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "head1"]);

    let mut resolved = resolved_engine_graph();
    if let ResolvedSource::Primary(p) = &mut resolved.sources[0] {
        p.pointer = "sub".to_string();
    }
    {
        let mut engine = engine_at(root);
        engine
            .set_mem_sync_state("engine", synced_key(), &baseline, None)
            .unwrap();
    }

    // The medium-relative id refuses; the message names the dialect and
    // the corrected id; the details pair maps supplied → corrected. An id
    // with no derivable correction rides the same refusal suggestion-free.
    {
        let mut engine = engine_at(root);
        let err = advance_baseline(
            &mut engine,
            root,
            &resolved,
            &input(&[("a.rs", "worked"), ("zzz.rs", "worked")]),
        )
        .unwrap_err();
        let AdvanceError::UnknownArtifact {
            artifacts,
            suggestions,
            ..
        } = &err
        else {
            panic!("expected UnknownArtifact, got {err:?}");
        };
        assert_eq!(artifacts, &vec!["a.rs".to_string(), "zzz.rs".to_string()]);
        assert_eq!(
            suggestions,
            &vec![("a.rs".to_string(), "sub/a.rs".to_string())],
            "only the medium-relative id gets a corrected form; zzz.rs has none"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("workspace-relative"),
            "names the dialect: {msg}"
        );
        assert!(
            msg.contains("`a.rs` → `sub/a.rs`"),
            "carries the concrete corrected id: {msg}"
        );
        assert!(
            msg.contains("never accepted"),
            "states the dialect does not widen: {msg}"
        );
    }
    // The refusal wrote nothing (the gate stayed atomic).
    assert!(
        read_advance_store(root, "engine", "graph")
            .unwrap()
            .is_none(),
        "a refused call must not create the advance store"
    );

    // The corrected workspace-relative id is the one the gate accepts.
    {
        let mut engine = engine_at(root);
        let out = advance_baseline(
            &mut engine,
            root,
            &resolved,
            &input(&[("sub/a.rs", "worked")]),
        )
        .unwrap();
        assert!(out.completed, "the sole slice artifact was disposed");
    }
}

/// `record_exclusions` gates on enumerable `S(D)` membership (not the changed
/// slice), so a **stable, unchanged** in-scope artifact can be declared
/// excluded — the direct write path the option-(a) migration needs. A
/// non-member refuses the whole call atomically; a re-declare merges.
#[test]
fn record_exclusions_gates_on_source_membership_and_merges() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // A source tree with two in-scope `.rs` members. No commits move after
    // this — the artifacts are stable, never in a changed slice.
    git(root, &["init", "-q"]);
    std::fs::write(root.join("a.rs"), "one").unwrap();
    std::fs::write(root.join("b.rs"), "two").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);

    let resolved = resolved_engine_graph();

    // Declare a.rs excluded with a rationale — accepted (S(D) member).
    let out = record_exclusions(
        &Engine::from_mounts(Vec::new()).unwrap(),
        root,
        &resolved,
        &BTreeMap::from([("a.rs".to_string(), "mined; no entity".to_string())]),
    )
    .unwrap();
    assert_eq!((out.added, out.excluded), (1, 1));
    let state = read_advance_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    assert_eq!(
        state.exclusions.get("a.rs").map(String::as_str),
        Some("mined; no entity")
    );

    // An artifact outside S(D) refuses the whole call — the store is untouched.
    let err = record_exclusions(
        &Engine::from_mounts(Vec::new()).unwrap(),
        root,
        &resolved,
        &BTreeMap::from([("does-not-exist.rs".to_string(), "x".to_string())]),
    )
    .unwrap_err();
    assert!(
        matches!(err, ExcludeError::NotSourceMember { .. }),
        "got {err:?}"
    );
    assert_eq!(
        read_advance_store(root, "engine", "graph")
            .unwrap()
            .unwrap()
            .exclusions
            .len(),
        1,
        "refused call left the ledger unchanged"
    );

    // Re-declaring merges (b.rs added alongside a.rs).
    let out2 = record_exclusions(
        &Engine::from_mounts(Vec::new()).unwrap(),
        root,
        &resolved,
        &BTreeMap::from([("b.rs".to_string(), "also mined".to_string())]),
    )
    .unwrap();
    assert_eq!((out2.added, out2.excluded), (1, 2));
}

/// A PARTIAL enumeration refuses the membership gate outright: under a
/// legacy-dialect scope pattern the enumerated set is not the population,
/// so the gate can neither refuse a genuinely in-scope artifact nor state
/// the short count as if it were `S(D)`. Typed refusal, nothing written.
#[test]
fn record_exclusions_refuses_partial_enumeration() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    git(root, &["init", "-q"]);
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("a.rs"), "one").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);

    // Pointer `sub`, MIXED scope: the prefix-free pattern enumerates
    // `sub/a.rs`, the retired-dialect pattern's share is silently absent.
    let mut resolved = resolved_engine_graph();
    if let ResolvedSource::Primary(p) = &mut resolved.sources[0] {
        p.pointer = "sub".to_string();
        p.scope.push(PatternEntry {
            path: "sub/nested.rs".to_string(),
            mode: PatternMode::Allow,
        });
    }

    // Even a genuine member of the surviving subset refuses: membership in
    // a set that is not the population is not membership in the population.
    let err = record_exclusions(
        &Engine::from_mounts(Vec::new()).unwrap(),
        root,
        &resolved,
        &BTreeMap::from([("sub/a.rs".to_string(), "mined; no entity".to_string())]),
    )
    .unwrap_err();
    assert!(
        matches!(err, ExcludeError::PartialEnumeration { .. }),
        "got {err:?}"
    );
    assert!(
        err.to_string().contains("incomplete"),
        "the refusal names the partiality: {err}"
    );
    assert!(
        read_advance_store(root, "engine", "graph")
            .unwrap()
            .is_none(),
        "a refused call must not create the advance store"
    );
}

/// `DispositionInput` parses both the bare-verdict and the reasoned forms
/// from one `--dispositions` payload (serde `untagged`).
#[test]
fn disposition_input_parses_bare_and_reasoned_forms() {
    let map: BTreeMap<String, DispositionInput> = serde_json::from_str(
        r#"{"a.rs": "worked", "b.rs": {"disposition": "excluded", "rationale": "generated"}}"#,
    )
    .unwrap();
    assert_eq!(map["a.rs"].verdict(), "worked");
    assert_eq!(map["a.rs"].rationale(), None);
    assert_eq!(map["b.rs"].verdict(), EXCLUDED_VERDICT);
    assert_eq!(map["b.rs"].rationale(), Some("generated"));
}

/// Criterion 4 (an earlier plana): the auto-`worked` matching
/// understands the SOURCE dialect — an anchor written source-relative
/// (`f.rs` + `source` name, decision 26) marks the pointer-joined slice
/// artifact (`srcdir/f.rs`) worked, exactly as a workspace-relative
/// anchor would. Requires a workspace root + pipeline store so the
/// source name resolves to its pointer.
#[test]
fn advance_auto_worked_matches_source_dialect_anchors() {
    use indexmap::IndexMap;
    use memstead_base::binding::{BINDING_VERSION, Binding, Operations};
    use memstead_base::vcs::Actor;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    git(root, &["init", "-q"]);
    std::fs::create_dir_all(root.join("srcdir")).unwrap();
    std::fs::write(root.join(".keep"), "x").unwrap();
    git(root, &["add", ".keep"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = head_sha(root);

    // The pipeline store carries the binding that maps source name
    // `source-tree` → pointer `srcdir` for mem `engine`.
    let binding = Binding {
        version: BINDING_VERSION,
        intent: None,
        sources: vec![memstead_base::pipeline::Source {
            name: "source-tree".to_string(),
            medium_type: memstead_base::pipeline::MediumType::Codebase,
            pointer: "srcdir".to_string(),
            change_detection: Some("git".to_string()),
            scope: vec![PatternEntry {
                path: "**/*.rs".to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: None,
        }],
        reference_mems: Vec::new(),
        destination_mem: "engine".to_string(),
        deny_paths: Vec::new(),
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: Operations {
            build: None,
            sync: None,
            verify: None,
        },
    };
    let dir = root.join(".memstead").join("projections").join("engine");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("graph.json"),
        serde_json::to_string_pretty(&binding).unwrap(),
    )
    .unwrap();

    // The resolved ingest's source points at `srcdir`, so slice
    // artifact ids come out pointer-joined (`srcdir/f.rs`).
    let mut resolved = resolved_engine_graph();
    if let [ResolvedSource::Primary(p)] = resolved.sources.as_mut_slice() {
        p.pointer = "srcdir".to_string();
    } else {
        panic!("fixture shape");
    }

    {
        let mut engine = engine_at(root);
        engine
            .set_mem_sync_state("engine", synced_key(), &baseline, None)
            .unwrap();
    }

    std::fs::write(root.join("srcdir").join("f.rs"), "fn f() {}").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "head1"]);

    // Anchored write in the SOURCE dialect: artifact `f.rs`, source
    // `source-tree` — no pointer prefix.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "Covers f.".to_string());
    sections.insert("purpose".to_string(), "Track f.rs.".to_string());
    {
        let mut engine = engine_at(root);
        engine.set_workspace_root(root.to_path_buf());
        engine
            .create_entity(
                memstead_base::CreateEntityArgs {
                    mem: "engine".to_string(),
                    title: "Covers F".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    anchors: vec![memstead_base::anchor::AnchorInput {
                        artifact: Some("f.rs".to_string()),
                        grain: Some("file".to_string()),
                        class: Some("anchored".to_string()),
                        content: Some("fn f() {}".to_string()),
                        hash_stability: Some("stable".to_string()),
                        source: Some("source-tree".to_string()),
                        ..Default::default()
                    }],
                    dry_run: false,
                },
                Actor::Agent,
                None,
                Some("source-dialect anchored write"),
            )
            .unwrap();
    }

    // Advance with NO explicit dispositions: `srcdir/f.rs` (the slice
    // form) auto-works from the source-dialect anchor.
    let mut engine = engine_at(root);
    engine.set_workspace_root(root.to_path_buf());
    let out = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new()).unwrap();
    assert!(
        out.completed,
        "the source-dialect anchor auto-worked the joined slice artifact: {out:?}"
    );
    assert_eq!(out.disposed, 1);
}

/// AC9a — an anchored write auto-marks its referenced frozen-slice
/// artifacts `worked`, so `advance` needs an explicit disposition only for
/// the residue, held across a HEAD move. Refusals: an artifact with no
/// anchor is never auto-worked, and an anchor referencing an artifact
/// OUTSIDE the presented slice fabricates no slice entry.
#[test]
fn advance_auto_worked_from_anchors_subtracts_slice_and_never_fabricates() {
    use indexmap::IndexMap;
    use memstead_base::vcs::Actor;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Baseline: a commit carrying no `.rs` files. `#synced` pins it, so the
    // moved slice below is purely the added `.rs` sources.
    git(root, &["init", "-q"]);
    std::fs::write(root.join(".keep"), "x").unwrap();
    git(root, &["add", ".keep"]);
    git(root, &["commit", "-qm", "base"]);
    let baseline = head_sha(root);

    let resolved = resolved_engine_graph();
    {
        let mut engine = engine_at(root);
        engine
            .set_mem_sync_state("engine", synced_key(), &baseline, None)
            .unwrap();
    }

    // head1: add a.rs + b.rs → slice = added [a.rs, b.rs].
    std::fs::write(root.join("a.rs"), "one").unwrap();
    std::fs::write(root.join("b.rs"), "bee").unwrap();
    git(root, &["add", "a.rs", "b.rs"]);
    git(root, &["commit", "-qm", "head1"]);

    // An anchored write into the destination mem `engine`: entity
    // `covers-a` file-anchors `a.rs` (inside the slice) AND `zzz.rs`
    // (outside it — must fabricate nothing).
    // Pin the true prepared-content hash (the engine computes it from
    // `content`): a fake hash would read as drifted and trip gate two.
    let make_anchor = |artifact: &str, content: &str| memstead_base::anchor::AnchorInput {
        artifact: Some(artifact.to_string()),
        grain: Some("file".to_string()),
        class: Some("anchored".to_string()),
        content: Some(content.to_string()),
        hash_stability: Some("stable".to_string()),
        ..Default::default()
    };
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "Covers a.".to_string());
    sections.insert("purpose".to_string(), "Track a.rs.".to_string());
    {
        let mut engine = engine_at(root);
        engine
            .create_entity(
                memstead_base::CreateEntityArgs {
                    mem: "engine".to_string(),
                    title: "Covers A".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    anchors: vec![make_anchor("a.rs", "one"), make_anchor("zzz.rs", "")],
                    dry_run: false,
                },
                Actor::Agent,
                None,
                Some("anchored write"),
            )
            .unwrap();
    }

    // (1) Advance with NO explicit dispositions → a.rs auto-worked from the
    // anchor; b.rs (no anchor) stays pending; zzz.rs (outside the slice)
    // fabricates nothing.
    {
        let mut engine = engine_at(root);
        let out = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new()).unwrap();
        assert!(!out.completed, "b.rs still pending");
        assert_eq!(
            out.remainder,
            slice(&["b.rs"], &[], &[]),
            "a.rs auto-worked from its anchor; zzz.rs never became a slice member"
        );
        assert_eq!(out.disposed, 1, "only a.rs auto-worked");
        assert_eq!(out.pending, 1);
    }

    // (2) HEAD moves (add c.rs). Re-present with no dispositions → old
    // remainder [b.rs] + new delta [c.rs]; a.rs stays absent (its
    // auto-`worked` persisted); c.rs is unanchored so it is NOT auto-worked.
    std::fs::write(root.join("c.rs"), "cee").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "head2"]);
    {
        let mut engine = engine_at(root);
        let out = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new()).unwrap();
        assert_eq!(
            out.remainder,
            slice(&["b.rs", "c.rs"], &[], &[]),
            "auto-worked a.rs absent; unanchored b.rs + new c.rs pending"
        );
        assert_eq!(
            out.disposed, 1,
            "still only a.rs auto-worked; c.rs unanchored"
        );
        assert!(!out.completed);
    }
}

/// Gate two: a `worked` disposition on an artifact whose anchor rows on
/// the destination mem still resolve `drifted` refuses atomically (the
/// store stays absent), naming the artifact and the entities; once the
/// rows are re-pinned to the current content the same advance completes.
/// Pins the 2026-09-09 flagship case: the change absorbed in prose, the
/// sidecar left on the old hash, the baseline advanced over it.
#[test]
fn advance_refuses_a_worked_artifact_whose_anchor_rows_still_drift() {
    use indexmap::IndexMap;
    use memstead_base::vcs::Actor;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    std::fs::write(root.join("a.rs"), "one").unwrap();
    git(root, &["add", "a.rs"]);
    git(root, &["commit", "-qm", "head1"]);
    let head1 = head_sha(root);
    let resolved = resolved_engine_graph();

    // The entity anchors a.rs at its head1 content; `#synced` sits at head1.
    let anchor = |content: &str| memstead_base::anchor::AnchorInput {
        artifact: Some("a.rs".to_string()),
        grain: Some("file".to_string()),
        class: Some("anchored".to_string()),
        content: Some(content.to_string()),
        hash_stability: Some("stable".to_string()),
        ..Default::default()
    };
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "Covers a.".to_string());
    sections.insert("purpose".to_string(), "Track a.rs.".to_string());
    {
        let mut engine = engine_at(root);
        engine
            .create_entity(
                memstead_base::CreateEntityArgs {
                    mem: "engine".to_string(),
                    title: "Covers A".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    anchors: vec![anchor("one")],
                    dry_run: false,
                },
                Actor::Agent,
                None,
                Some("anchored write"),
            )
            .unwrap();
        engine
            .set_mem_sync_state("engine", synced_key(), &head1, None)
            .unwrap();
    }

    // head2 changes a.rs: the slice presents it as modified, the row drifts.
    std::fs::write(root.join("a.rs"), "two").unwrap();
    git(root, &["add", "a.rs"]);
    git(root, &["commit", "-qm", "head2"]);

    {
        let mut engine = engine_at(root);
        engine.set_workspace_root(root.to_path_buf());
        let err = advance_baseline(&mut engine, root, &resolved, &input(&[("a.rs", "worked")]))
            .expect_err("a worked artifact with a drifted row is refused");
        match err {
            AdvanceError::AnchorsStillDrifted { artifacts } => {
                assert_eq!(
                    artifacts,
                    vec![("a.rs".to_string(), vec!["engine--covers-a".to_string()])]
                );
            }
            other => panic!("expected AnchorsStillDrifted, got {other:?}"),
        }
        assert!(
            read_advance_store(root, "engine", "graph")
                .unwrap()
                .is_none(),
            "a refused advance leaves no store behind"
        );
        // Auto-derivation is gated the same way: with no explicit
        // disposition the anchored artifact would be auto-worked, and the
        // drifted row refuses that too.
        let err = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new())
            .expect_err("auto-worked over a drifted row is refused");
        assert!(matches!(err, AdvanceError::AnchorsStillDrifted { .. }));
    }

    // Re-pin the row to the current content: the same advance completes.
    {
        let mut engine = engine_at(root);
        engine.set_workspace_root(root.to_path_buf());
        engine
            .update_entity(
                memstead_base::UpdateEntityArgs {
                    id: memstead_base::EntityId("engine--covers-a".into()),
                    expected_hash: None,
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    dry_run: false,
                    declare_relations: Vec::new(),
                    anchors: vec![anchor("two")],
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                Actor::Agent,
                None,
                Some("re-pin"),
            )
            .unwrap();
        let out = advance_baseline(&mut engine, root, &resolved, &input(&[("a.rs", "worked")]))
            .expect("re-pinned rows admit the worked disposition");
        assert!(out.completed, "{out:?}");
    }
}

/// A workspace with one folder mem `engine` (default@1.0.0), a git source
/// tree at the root, and `files` written relative to the root. Returns the
/// root; the caller writes the binding.
fn a3_workspace(root: &std::path::Path, files: &[&str]) {
    let mem_dir = root.join("mem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    let mount = memstead_base::workspace::Mount {
        mem: "engine".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::workspace::MountStorage::Folder {
            path: mem_dir.clone(),
        },
        capability: memstead_base::workspace::MountCapability::Write,
        lifecycle: memstead_base::workspace::MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    memstead_base::workspace_store::WorkspaceStoreAdapter::save_state(
        &memstead_base::FileWorkspaceStore::new(),
        root,
        &memstead_base::workspace::Workspace {
            mounts: vec![mount],
            settings: memstead_base::workspace::WorkspaceSettings::default(),
        },
    )
    .unwrap();
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());
    for f in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "fn x() {}\n").unwrap();
    }
}

fn a3_binding(
    sources: &[(&str, &str)],
    deny: &[&str],
    batch_size: u32,
) -> memstead_base::binding::Binding {
    memstead_base::binding::Binding {
        version: memstead_base::binding::BINDING_VERSION,
        intent: None,
        sources: sources
            .iter()
            .map(|(name, glob)| memstead_base::pipeline::Source {
                name: name.to_string(),
                medium_type: memstead_base::pipeline::MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
                scope: vec![memstead_base::pipeline::PatternEntry {
                    path: glob.to_string(),
                    mode: memstead_base::pipeline::PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            })
            .collect(),
        reference_mems: Vec::new(),
        destination_mem: "engine".to_string(),
        deny_paths: deny.iter().map(|d| d.to_string()).collect(),
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: memstead_base::binding::Operations {
            build: Some(memstead_base::binding::BuildOperation {
                mode: memstead_base::binding::BuildMode::Discovery,
                trigger: memstead_base::pipeline::IngestTrigger::Loop,
                batch_size,
                post_actions: None,
            }),
            sync: None,
            verify: Some(memstead_base::binding::VerifyOperation {
                trigger: memstead_base::pipeline::IngestTrigger::Manual,
                batch_size,
                adjudication_cap: memstead_base::binding::DEFAULT_ADJUDICATION_CAP,
                // Scheduled full walks off: the sampled path is under test.
                full_resync_every: 0,
            }),
        },
    }
}

/// A3 AC3: an exclusion survives a binding edit that changes an unrelated
/// field, still carrying its source and rationale; removing its source
/// from the declaration drops it, reported once with the source named.
#[test]
fn exclusions_survive_edits_and_drop_with_their_source() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    a3_workspace(root, &["src/a.rs", "docs/x.md"]);
    let engine = Engine::from_workspace_root(root).unwrap();
    let two = |batch: u32| {
        a3_binding(
            &[("graph", "src/**/*.rs"), ("docs", "docs/**/*.md")],
            &[],
            batch,
        )
    };

    let b = two(20);
    memstead_base::pipeline_store::write_binding(root, "engine", "graph", &b).unwrap();
    let resolved = memstead_base::binding_run::resolve_binding_run("engine/graph", &b).unwrap();
    let mut ex = BTreeMap::new();
    ex.insert("docs/x.md".to_string(), "index page, mined".to_string());
    record_exclusions(&engine, root, &resolved, &ex).unwrap();
    let ledger = reconcile_exclusions(&engine, root, &resolved).unwrap();
    assert_eq!(ledger.active.len(), 1);
    assert_eq!(ledger.active[0].source, "docs");
    assert_eq!(ledger.active[0].rationale, "index page, mined");
    assert!(ledger.dropped.is_empty());

    // The edit: batch size only. Same source, same rationale.
    let edited = two(5);
    memstead_base::pipeline_store::write_binding(root, "engine", "graph", &edited).unwrap();
    let resolved =
        memstead_base::binding_run::resolve_binding_run("engine/graph", &edited).unwrap();
    let ledger = reconcile_exclusions(&engine, root, &resolved).unwrap();
    assert_eq!(ledger.active.len(), 1, "{ledger:?}");
    assert_eq!(ledger.active[0].artifact, "docs/x.md");
    assert_eq!(ledger.active[0].rationale, "index page, mined");
    assert!(ledger.dropped.is_empty());

    // The source leaves the declaration: dropped, named, once.
    let without = a3_binding(&[("graph", "src/**/*.rs")], &[], 5);
    memstead_base::pipeline_store::write_binding(root, "engine", "graph", &without).unwrap();
    let resolved =
        memstead_base::binding_run::resolve_binding_run("engine/graph", &without).unwrap();
    let ledger = reconcile_exclusions(&engine, root, &resolved).unwrap();
    assert!(ledger.active.is_empty(), "{ledger:?}");
    assert_eq!(ledger.dropped.len(), 1);
    assert_eq!(ledger.dropped[0].artifact, "docs/x.md");
    assert_eq!(ledger.dropped[0].source, "docs");
    assert_eq!(ledger.dropped[0].rationale, "index page, mined");
    let again = reconcile_exclusions(&engine, root, &resolved).unwrap();
    assert!(
        again.active.is_empty() && again.dropped.is_empty(),
        "reported once: {again:?}"
    );
    assert!(
        read_advance_store(root, "engine", "graph")
            .unwrap()
            .is_none()
    );
}
