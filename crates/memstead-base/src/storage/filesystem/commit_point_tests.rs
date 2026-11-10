//! The folder commit around its commit point: every step faulted or
//! cut off in turn, and the outcome compared with a commit nobody
//! disturbed.

#![cfg(test)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

use super::fault_io::{FaultIo, IoOp};
use super::*;

const OLD: &[u8] = b"# old\n";
const NEW: &[u8] = b"# old, rewritten\n";
const FRESH: &[u8] = b"# fresh\n";
const ANCHORS: &[u8] = b"{\"v\":1,\"entities\":{}}\n";

/// The files the commit under test brings into place, in the order
/// its record names them.
const COMMIT_FILES: [&str; 4] = [
    ".memstead/anchors.json",
    "fresh/deep/new.md",
    "gone.md",
    "notes/old.md",
];

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn record(ts: SystemTime, kind: ProvenanceKind, entity: &str) -> Provenance {
    Provenance::new(
        ts,
        kind,
        Some(entity.to_string()),
        Actor::Agent,
        None,
        Some(format!("{entity} note")),
    )
}

/// A mem with history: three files and one ledger line.
fn seed(root: &Path) {
    let backend = FilesystemBackend::new(root.to_path_buf());
    backend
        .write_entity(Path::new("keep.md"), b"# keep\n")
        .unwrap();
    backend
        .write_entity(Path::new("gone.md"), b"# gone\n")
        .unwrap();
    backend
        .write_entity(Path::new("notes/old.md"), OLD)
        .unwrap();
    backend
        .stage_provenance(&record(
            at(1_700_000_000),
            ProvenanceKind::Create,
            "m--keep",
        ))
        .unwrap();
    backend.commit("seed", &CommitContext::internal()).unwrap();
}

/// A mem directory nothing was ever written to.
fn no_seed(_root: &Path) {}

/// Stage the commit under test: an overwrite in an existing directory,
/// a new file two new directories deep, a delete, the anchors sidecar,
/// and two provenance records. The records carry fixed, equal
/// timestamps, so every run renders the same two ledger lines (the
/// second one clamped a millisecond on) and whole trees compare byte
/// for byte.
fn stage(backend: &FilesystemBackend) {
    backend
        .write_entity(Path::new("notes/old.md"), NEW)
        .unwrap();
    backend
        .write_entity(Path::new("fresh/deep/new.md"), FRESH)
        .unwrap();
    backend.delete_entity(Path::new("gone.md")).unwrap();
    backend.write_anchors_sidecar(ANCHORS).unwrap();
    backend
        .stage_provenance(&record(at(1_700_000_010), ProvenanceKind::Update, "m--old"))
        .unwrap();
    backend
        .stage_provenance(&record(at(1_700_000_010), ProvenanceKind::Create, "m--new"))
        .unwrap();
}

/// Every file with its bytes and every directory under a mem root.
#[derive(Debug, PartialEq, Eq)]
struct Tree {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
}

fn tree(root: &Path) -> Tree {
    fn walk(root: &Path, dir: &Path, out: &mut Tree) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // The write lock's files are no part of the mem's state.
            if rel == ".memstead/locks" {
                continue;
            }
            if path.is_dir() {
                out.dirs.insert(rel);
                walk(root, &path, out);
            } else {
                out.files.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = Tree {
        files: BTreeMap::new(),
        dirs: BTreeSet::new(),
    };
    walk(root, root, &mut out);
    out
}

fn temp_files(tree: &Tree) -> Vec<&String> {
    tree.files
        .keys()
        .filter(|path| is_temp_name(path.rsplit('/').next().unwrap()))
        .collect()
}

/// One undisturbed run of the commit under test over `setup`: the tree
/// before it, the tree after it, and the steps it took.
struct Control {
    before: Tree,
    after: Tree,
    trace: Vec<IoOp>,
    commit_point: usize,
    /// The mem root the trace's paths sit under.
    root: PathBuf,
}

fn control(setup: fn(&Path)) -> Control {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());
    let before = tree(tmp.path());
    let io = FaultIo::new();
    let backend = FilesystemBackend::with_io(tmp.path().to_path_buf(), io.clone());
    stage(&backend);
    backend
        .commit("under test", &CommitContext::internal())
        .unwrap();
    let after = tree(tmp.path());
    assert!(
        temp_files(&after).is_empty(),
        "an undisturbed commit leaves no temp"
    );
    assert!(
        !after.files.contains_key(COMMIT_RECORD_REL),
        "the record of an applied commit does not remain"
    );
    Control {
        before,
        after,
        trace: io.trace(),
        commit_point: io.commit_point().expect("the trace holds a commit"),
        root: tmp.path().to_path_buf(),
    }
}

/// An IO error at every step in turn. At or before the commit point
/// the call fails as a plain IO error, storage is what it was and the
/// same staging sent again commits; after it the call reports a
/// decided commit, and the completion at load ends in the undisturbed
/// state.
fn fail_at_every_step(setup: fn(&Path)) {
    let control = control(setup);
    for n in 0..control.trace.len() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        setup(root);
        let backend = FilesystemBackend::with_io(root.to_path_buf(), FaultIo::fail_at(n));
        stage(&backend);
        let result = backend.commit("under test", &CommitContext::internal());
        let step = &control.trace[n];

        if n <= control.commit_point {
            assert!(
                matches!(result, Err(BackendError::Io(_))),
                "step {n} ({step:?}) is before the commit point: {result:?}"
            );
            assert_eq!(
                tree(root),
                control.before,
                "step {n} ({step:?}): storage is what it was"
            );
            // Nothing staged survives the failed commit.
            assert_eq!(
                backend.read_entity(Path::new("fresh/deep/new.md")).unwrap(),
                None,
                "step {n}"
            );
            assert_eq!(
                backend.read_entity(Path::new("notes/old.md")).unwrap(),
                control.before.files.get("notes/old.md").cloned(),
                "step {n}"
            );
            assert_eq!(
                backend.read_anchors_sidecar().unwrap(),
                control.before.files.get(".memstead/anchors.json").cloned(),
                "step {n}"
            );
            // The same call sent again behaves as a first call.
            stage(&backend);
            backend
                .commit("under test", &CommitContext::internal())
                .unwrap_or_else(|e| panic!("step {n}: the resent commit failed: {e}"));
            assert_eq!(
                tree(root),
                control.after,
                "step {n}: the resent commit ends where an undisturbed one does"
            );
        } else {
            assert!(
                matches!(result, Err(BackendError::CommitNotApplied { .. })),
                "step {n} ({step:?}) is after the commit point: {result:?}"
            );
            complete_and_compare(root, &control, n);
        }
    }
}

/// Finish the cut-off commit under `root` with a backend that never
/// saw it, and hold the result against the undisturbed commit.
fn complete_and_compare(root: &Path, control: &Control, n: usize) {
    let record_on_disk = root.join(COMMIT_RECORD_REL).exists();
    let fresh = FilesystemBackend::new(root.to_path_buf());
    let completed = fresh
        .complete_interrupted_commit()
        .unwrap_or_else(|e| panic!("step {n}: completion failed: {e}"));
    if record_on_disk {
        assert_eq!(
            completed.map(|c| c.files),
            Some(COMMIT_FILES.iter().map(|s| s.to_string()).collect()),
            "step {n}: the completion names the commit's files"
        );
    } else {
        // Only the very last step, the sync after the record's removal,
        // can fail with the record already gone and nothing left to do.
        assert_eq!(n, control.trace.len() - 1, "step {n}");
        assert_eq!(completed, None, "step {n}");
    }
    assert_eq!(
        tree(root),
        control.after,
        "step {n}: the completed commit equals the undisturbed one"
    );
    assert_eq!(
        fresh.complete_interrupted_commit().unwrap(),
        None,
        "step {n}: a second completion finds nothing"
    );
}

#[test]
fn an_io_error_at_any_step_fails_whole_or_is_completed_at_load() {
    fail_at_every_step(seed);
}

#[test]
fn an_io_error_at_any_step_of_a_first_commit_fails_whole_or_is_completed_at_load() {
    // A mem with no `.memstead/` yet: the directory the attempt creates
    // for its record goes away again with a failed attempt.
    fail_at_every_step(no_seed);
}

/// The process dying at every step in turn. Before the commit point a
/// fresh load shows the mem as it was and takes the attempt's temps
/// away; after it the load completes the commit.
fn kill_at_every_step(setup: fn(&Path)) {
    let control = control(setup);
    for n in 0..control.trace.len() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        setup(root);
        let backend = FilesystemBackend::with_io(root.to_path_buf(), FaultIo::kill_at(n));
        stage(&backend);
        let died = catch_unwind(AssertUnwindSafe(|| {
            let _ = backend.commit("under test", &CommitContext::internal());
        }));
        assert!(died.is_err(), "step {n}: the kill must cut the commit off");
        drop(backend);
        let step = &control.trace[n];

        if n <= control.commit_point {
            let fresh = FilesystemBackend::new(root.to_path_buf());
            // Even before the recovery runs, no entity of the attempt
            // is listed: its contents sit in temps, which are no
            // entities.
            let listed: BTreeSet<PathBuf> = fresh.list_entities().unwrap().into_iter().collect();
            let expected: BTreeSet<PathBuf> = control
                .before
                .files
                .keys()
                .filter(|p| p.ends_with(".md") && !p.starts_with(".memstead/"))
                .map(PathBuf::from)
                .collect();
            assert_eq!(listed, expected, "step {n} ({step:?})");

            assert_eq!(
                fresh.complete_interrupted_commit().unwrap(),
                None,
                "step {n} ({step:?}): nothing was decided"
            );
            let now = tree(root);
            assert_eq!(
                now.files, control.before.files,
                "step {n} ({step:?}): every file is what it was and the temps are gone"
            );
            // A directory the dead attempt created may stay behind,
            // empty: nothing says who made an empty directory.
            assert!(
                control.before.dirs.is_subset(&now.dirs),
                "step {n} ({step:?})"
            );
        } else {
            complete_and_compare(root, &control, n);
        }
    }
}

#[test]
fn a_kill_at_any_step_leaves_the_prior_state_or_is_completed_at_load() {
    kill_at_every_step(seed);
}

#[test]
fn a_kill_at_any_step_of_a_first_commit_leaves_the_prior_state_or_is_completed_at_load() {
    kill_at_every_step(no_seed);
}

/// The order of the steps is the durability argument: every content
/// and every name is synced before the record is, the record is synced
/// before anything moves into place, and the final syncs come before
/// success.
#[test]
fn every_sync_precedes_the_step_that_relies_on_it() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    seed(&root);
    let io = FaultIo::new();
    let backend = FilesystemBackend::with_io(root.clone(), io.clone());
    stage(&backend);
    backend
        .commit("under test", &CommitContext::internal())
        .unwrap();
    let trace = io.trace();
    let point = io.commit_point().unwrap();
    assert_eq!(io.op_count(), trace.len());

    let meta = root.join(".memstead");
    let record = root.join(COMMIT_RECORD_REL);
    let ledger = meta.join("changes.jsonl");
    let position = |op: &IoOp| trace.iter().position(|t| t == op);
    let last_position = |op: &IoOp| trace.iter().rposition(|t| t == op);

    // Before the commit point: each temp is written, synced, and its
    // directory synced after it.
    let temps: Vec<&PathBuf> = trace[..point]
        .iter()
        .filter_map(|op| match op {
            IoOp::Write(p) => Some(p),
            _ => None,
        })
        .collect();
    // Three contents (two entities, the sidecar) and the record's temp.
    assert_eq!(temps.len(), 4, "{trace:#?}");
    for tmp_path in &temps {
        let written = position(&IoOp::Write((*tmp_path).clone())).unwrap();
        let synced = position(&IoOp::SyncFile((*tmp_path).clone())).unwrap();
        assert!(written < synced && synced < point, "{tmp_path:?}");
        if !is_record_temp(tmp_path) {
            let dir = tmp_path.parent().unwrap().to_path_buf();
            let dir_synced = trace[..point]
                .iter()
                .rposition(|t| t == &IoOp::SyncDir(dir.clone()))
                .unwrap_or_else(|| panic!("{dir:?} is synced before the commit point"));
            assert!(written < dir_synced, "{tmp_path:?}");
        }
    }
    // The directories the attempt created are named durably too.
    for created in [root.join("fresh"), root.join("fresh/deep")] {
        let made = position(&IoOp::CreateDir(created.clone())).unwrap();
        let parent_synced = trace[..point]
            .iter()
            .rposition(|t| t == &IoOp::SyncDir(created.parent().unwrap().to_path_buf()))
            .unwrap();
        assert!(made < parent_synced, "{created:?}");
    }

    // The commit point is the record's rename; the very next step
    // makes it durable, and nothing moves into place before that.
    assert!(matches!(&trace[point], IoOp::Rename { to, .. } if to == &record));
    assert_eq!(trace[point + 1], IoOp::SyncDir(meta.clone()));
    let first_move = trace
        .iter()
        .position(|op| match op {
            IoOp::Rename { to, .. } => to != &record,
            IoOp::RemoveFile(p) => p == &root.join("gone.md"),
            _ => false,
        })
        .unwrap();
    assert!(first_move > point + 1);

    // After the moves: their directories, then the ledger and its
    // sync, then the record's removal and the last sync.
    let last_move = trace
        .iter()
        .rposition(|op| match op {
            IoOp::Rename { to, .. } => to != &record,
            IoOp::RemoveFile(p) => p == &root.join("gone.md"),
            _ => false,
        })
        .unwrap();
    for dir in [
        root.clone(),
        meta.clone(),
        root.join("notes"),
        root.join("fresh/deep"),
    ] {
        let synced = trace[last_move..]
            .iter()
            .position(|t| t == &IoOp::SyncDir(dir.clone()))
            .unwrap_or_else(|| panic!("{dir:?} is synced after the moves"));
        assert!(synced > 0);
    }
    let appended = position(&IoOp::Append(ledger.clone())).unwrap();
    let ledger_synced = last_position(&IoOp::SyncFile(ledger.clone())).unwrap();
    let record_removed = last_position(&IoOp::RemoveFile(record.clone())).unwrap();
    assert!(last_move < appended);
    assert!(appended < ledger_synced);
    assert!(ledger_synced < record_removed);
    assert_eq!(record_removed, trace.len() - 2);
    assert_eq!(trace[trace.len() - 1], IoOp::SyncDir(meta));
}

fn is_record_temp(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with(".commit.json.tmp."))
}

/// While a decided commit waits to be applied the drift cursor is one
/// no ledger line carries, so the engine's probe sees the mem change
/// when the commit is cut off and again when it is completed.
#[test]
fn an_interrupted_commit_moves_the_drift_cursor_twice() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    seed(&root);
    let before = FilesystemBackend::new(root.clone()).current_head().unwrap();
    assert!(before.is_some());

    let point = control(seed).commit_point;
    let backend = FilesystemBackend::with_io(root.clone(), FaultIo::fail_at(point + 2));
    stage(&backend);
    let result = backend.commit("under test", &CommitContext::internal());
    assert!(matches!(result, Err(BackendError::CommitNotApplied { .. })));

    let during = backend.current_head().unwrap();
    assert_eq!(during.as_deref(), Some(COMMIT_PENDING_HEAD));
    assert_ne!(during, before);

    // A decided commit blocks the next one until it is completed.
    backend
        .write_entity(Path::new("later.md"), b"# later\n")
        .unwrap();
    let blocked = backend.commit("later", &CommitContext::internal());
    assert!(
        matches!(blocked, Err(BackendError::Other(_))),
        "{blocked:?}"
    );
    assert!(!root.join("later.md").exists());

    assert!(backend.complete_interrupted_commit().unwrap().is_some());
    let after = backend.current_head().unwrap();
    assert_ne!(after, during);
    assert_ne!(after, before);
    // The cursor is the second of the commit's two ledger lines.
    assert_eq!(after.as_deref(), Some("2023-11-14T22:13:30.001Z"));
}

/// Provenance staged for a commit lands with it and never without it.
#[test]
fn a_staged_record_lands_with_its_commit_and_is_dropped_with_a_discard() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let backend = FilesystemBackend::new(root.clone());
    let ledger = root.join(".memstead/changes.jsonl");

    backend.write_entity(Path::new("a.md"), b"a").unwrap();
    backend
        .stage_provenance(&record(at(1_700_000_000), ProvenanceKind::Create, "m--a"))
        .unwrap();
    assert!(!ledger.exists(), "staging writes nothing");
    backend.discard_pending().unwrap();
    backend.commit("empty", &CommitContext::internal()).unwrap();
    assert!(!ledger.exists(), "a discarded record never lands");
    assert!(!root.join("a.md").exists());

    // A commit of records alone is a commit: the lines land, in order,
    // with strictly increasing timestamps.
    for entity in ["m--a", "m--b", "m--c"] {
        backend
            .stage_provenance(&record(at(1_700_000_000), ProvenanceKind::Delete, entity))
            .unwrap();
    }
    backend
        .commit("ledger only", &CommitContext::internal())
        .unwrap();
    let read = backend.read_provenance(None).unwrap();
    assert_eq!(
        read.iter().map(|r| r.entity.as_deref()).collect::<Vec<_>>(),
        vec![Some("m--a"), Some("m--b"), Some("m--c")]
    );
    assert!(read.windows(2).all(|w| w[0].timestamp < w[1].timestamp));
    assert_eq!(read[0].note.as_deref(), Some("m--a note"));
    assert_eq!(tree(&root).files.len(), 1, "only the ledger");
}

/// A completion that finds the ledger cut off inside one of the
/// commit's lines closes the fragment and writes the missing lines
/// whole, once.
#[test]
fn a_cut_off_ledger_append_is_completed_without_a_double_line() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    seed(&root);
    let control = control(seed);
    let append = control
        .trace
        .iter()
        .position(|op| matches!(op, IoOp::Append(_)))
        .unwrap();

    // Cut the commit off after the ledger append, then make the append
    // look torn: keep the first line and half of the second.
    let backend = FilesystemBackend::with_io(root.clone(), FaultIo::fail_at(append + 1));
    stage(&backend);
    let result = backend.commit("under test", &CommitContext::internal());
    assert!(matches!(result, Err(BackendError::CommitNotApplied { .. })));
    let ledger = root.join(".memstead/changes.jsonl");
    let whole = std::fs::read(&ledger).unwrap();
    assert_eq!(whole, control.after.files[".memstead/changes.jsonl"]);
    std::fs::write(&ledger, &whole[..whole.len() - 40]).unwrap();

    let fresh = FilesystemBackend::new(root.clone());
    assert!(fresh.complete_interrupted_commit().unwrap().is_some());
    let entities: Vec<Option<String>> = fresh
        .read_provenance(None)
        .unwrap()
        .into_iter()
        .map(|r| r.entity)
        .collect();
    assert_eq!(
        entities,
        vec![
            Some("m--keep".to_string()),
            Some("m--old".to_string()),
            Some("m--new".to_string())
        ],
        "each line once, the torn fragment skipped"
    );
    assert_eq!(
        fresh.current_head().unwrap().as_deref(),
        Some("2023-11-14T22:13:30.001Z")
    );
}

/// A record on disk that cannot be read is an error at load, never a
/// mem served as if nothing were pending.
#[test]
fn an_unreadable_commit_record_is_an_error_not_a_skip() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    seed(&root);
    std::fs::write(root.join(COMMIT_RECORD_REL), b"{ not a record").unwrap();
    let backend = FilesystemBackend::new(root.clone());
    assert!(matches!(
        backend.complete_interrupted_commit(),
        Err(BackendError::Other(_))
    ));

    // A record whose paths leave the mem is refused before anything
    // is touched.
    std::fs::write(
        root.join(COMMIT_RECORD_REL),
        br#"{"v":1,"ops":[{"op":"delete","path":"../outside.md"}],"ledger":[]}"#,
    )
    .unwrap();
    assert!(matches!(
        backend.complete_interrupted_commit(),
        Err(BackendError::Other(_))
    ));
    assert!(root.join(COMMIT_RECORD_REL).exists());
}

#[test]
fn only_this_backends_temp_names_are_swept() {
    assert!(is_temp_name(".a.md.tmp.18a3f-0"));
    assert!(is_temp_name(".commit.json.tmp.ff-1c"));
    assert!(!is_temp_name("a.md"));
    assert!(!is_temp_name(".gitignore"));
    assert!(!is_temp_name(".a.md.tmp."));
    assert!(!is_temp_name(".a.md.tmp.zz-0"));
    assert!(!is_temp_name("a.md.tmp.18a3f-0"));
    assert!(!is_temp_name(".tmp.18a3f-0"));

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    seed(&root);
    std::fs::write(root.join("notes/.old.md.tmp.1f-0"), b"left behind").unwrap();
    std::fs::write(root.join(".memstead/.commit.json.tmp.1f-1"), b"left behind").unwrap();
    std::fs::write(root.join("notes/.hidden"), b"not ours").unwrap();
    let before = tree(&root);
    let backend = FilesystemBackend::new(root.clone());
    assert_eq!(backend.complete_interrupted_commit().unwrap(), None);
    let after = tree(&root);
    assert!(temp_files(&after).is_empty());
    assert_eq!(after.files.len(), before.files.len() - 2);
    assert!(after.files.contains_key("notes/.hidden"));
}

/// A sync that fails once after the commit point is made up for by the
/// completion: every sync the undisturbed commit runs after its commit
/// point has run successfully by the time the commit is complete, on a
/// seeded mem and on a mem's first commit. A completion that finds a
/// move already made cannot know whether its sync happened, so it
/// syncs again.
#[test]
fn a_completion_redoes_every_sync_the_cut_off_commit_owed() {
    for setup in [seed as fn(&Path), no_seed as fn(&Path)] {
        let control = control(setup);
        let is_sync = |op: &IoOp| matches!(op, IoOp::SyncDir(_) | IoOp::SyncFile(_));
        // The syncs the commit owes after its commit point, the sync
        // of the record's own name and of its removal included.
        let after_point = control.commit_point + 1;
        let owed: Vec<(PathBuf, bool)> = control.trace[after_point..]
            .iter()
            .filter(|op| is_sync(op))
            .map(|op| match op {
                IoOp::SyncDir(p) => (p.clone(), true),
                IoOp::SyncFile(p) => (p.clone(), false),
                _ => unreachable!(),
            })
            .collect();
        assert!(!owed.is_empty());

        for n in after_point..control.trace.len() {
            let tmp = TempDir::new().unwrap();
            setup(tmp.path());
            let io = FaultIo::fail_at(n);
            let backend = FilesystemBackend::with_io(tmp.path().to_path_buf(), io.clone());
            // Paths in the control trace sit under the control's own
            // directory; compare by path relative to each mem root.
            let rel = |p: &Path, root: &Path| p.strip_prefix(root).unwrap().to_path_buf();
            stage(&backend);
            backend
                .commit("under test", &CommitContext::internal())
                .expect_err("the commit is cut off");
            backend
                .complete_interrupted_commit()
                .unwrap_or_else(|e| panic!("step {n}: completion failed: {e}"));

            let trace = io.trace();
            let done: Vec<(PathBuf, bool)> = trace
                .iter()
                .enumerate()
                .skip(control.commit_point + 1)
                .filter(|(i, op)| *i != n && is_sync(op))
                .map(|(_, op)| match op {
                    IoOp::SyncDir(p) => (rel(p, tmp.path()), true),
                    IoOp::SyncFile(p) => (rel(p, tmp.path()), false),
                    _ => unreachable!(),
                })
                .collect();
            for (path, is_dir) in &owed {
                let want = (rel(path, &control.root), *is_dir);
                assert!(
                    done.contains(&want),
                    "step {n} ({:?}): the sync of {:?} was never made good",
                    control.trace[n],
                    want.0
                );
            }
        }
    }
}

/// The first commit of a mem creates its ledger, and stages nothing
/// else under `.memstead/` here. The ledger's name is durable when
/// success is returned: `.memstead/` is synced after the file exists.
#[test]
fn a_ledger_the_commit_creates_has_its_name_synced() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let io = FaultIo::new();
    let backend = FilesystemBackend::with_io(root.clone(), io.clone());
    backend.write_entity(Path::new("a.md"), NEW).unwrap();
    backend
        .stage_provenance(&record(at(1_700_000_010), ProvenanceKind::Create, "m--a"))
        .unwrap();
    backend.commit("first", &CommitContext::internal()).unwrap();

    let trace = io.trace();
    let appended = trace
        .iter()
        .position(|op| op == &IoOp::Append(changelog_path(&root)))
        .expect("the commit appends its ledger line");
    assert!(
        trace[appended..]
            .iter()
            .any(|op| op == &IoOp::SyncDir(root.join(".memstead"))),
        "the new ledger's directory entry is not synced: {:?}",
        &trace[appended..]
    );
}

/// A read that goes to storage never returns part of a cut-off commit:
/// while the commit record is on disk, every read of a file a commit
/// can move refuses, whether the fault persists or has passed. The
/// completion is a load's, and after it the reads return the committed
/// bytes.
#[test]
fn a_read_refuses_a_cut_off_commit_until_a_load_completes_it() {
    let control = control(seed);
    // The last step syncs the record's removal: the record is gone by
    // then and the files are whole, so that step is not in this sweep.
    for n in control.commit_point + 1..control.trace.len() - 1 {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        seed(root);
        let io = FaultIo::new();
        let backend = FilesystemBackend::with_io(root.to_path_buf(), io.clone());
        stage(&backend);
        io.reset();
        io.arm_fail_from(n);
        let result = backend.commit("under test", &CommitContext::internal());
        assert!(
            matches!(result, Err(BackendError::CommitNotApplied { .. })),
            "step {n}: {result:?}"
        );

        for fault_persists in [true, false] {
            if !fault_persists {
                io.reset();
            }
            let label = format!("step {n}, fault persists: {fault_persists}");
            assert!(
                backend.read_entity(Path::new("notes/old.md")).is_err(),
                "{label}"
            );
            assert!(
                backend.entity_exists(Path::new("gone.md")).is_err(),
                "{label}"
            );
            assert!(backend.list_entities().is_err(), "{label}");
            assert!(backend.read_all_entities().is_err(), "{label}");
            assert!(backend.read_anchors_sidecar().is_err(), "{label}");
            assert!(backend.read_provenance(None).is_err(), "{label}");
            assert!(
                crate::entity::source::EntitySource::Directory {
                    root: root.to_path_buf()
                }
                .read_all()
                .is_err(),
                "{label}: the directory reader"
            );
        }

        // The load's completion, then every read serves the commit.
        assert!(
            backend.complete_interrupted_commit().unwrap().is_some(),
            "step {n}"
        );
        assert_eq!(tree(root), control.after, "step {n}");
        assert_eq!(
            backend.read_anchors_sidecar().unwrap(),
            Some(ANCHORS.to_vec()),
            "step {n}"
        );
        assert_eq!(
            backend.read_entity(Path::new("notes/old.md")).unwrap(),
            Some(NEW.to_vec()),
            "step {n}"
        );
        assert_eq!(
            backend.read_entity(Path::new("gone.md")).unwrap(),
            None,
            "step {n}"
        );
    }
}

/// A commit that is applied except for the sync of its record's removal
/// is unfinished, and the backend says so on every surface until a
/// completion makes the sync good: the drift cursor reads as pending
/// (also for a commit that carried no ledger line, whose cursor would
/// otherwise not move at all), reads refuse, a further commit refuses,
/// and the completion names the commit's files.
#[test]
fn a_commit_whose_removal_sync_is_owed_stays_unfinished_until_completed() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    seed(&root);
    let io = FaultIo::new();
    let backend = FilesystemBackend::with_io(root.clone(), io.clone());
    // A backend's first commit carries one sync more (the mem root, for
    // the name of `.memstead/`); get it out of the way so the commit
    // probed below and the commit under test take the same steps.
    backend.write_entity(Path::new("warm-up.md"), NEW).unwrap();
    backend
        .commit("warm-up", &CommitContext::internal())
        .unwrap();
    let before = backend.current_head().unwrap();

    // A commit with no provenance record: one file, no ledger line.
    io.reset();
    backend
        .write_entity(Path::new("notes/old.md"), NEW)
        .unwrap();
    backend.commit("probe", &CommitContext::internal()).unwrap();
    let last_step = io.op_count() - 1;
    assert_eq!(io.trace()[last_step], IoOp::SyncDir(root.join(".memstead")));
    assert_eq!(
        backend.current_head().unwrap(),
        before,
        "no ledger line was written"
    );

    io.reset();
    io.arm_fail_at(last_step);
    backend
        .write_entity(Path::new("notes/old.md"), FRESH)
        .unwrap();
    let result = backend.commit("under test", &CommitContext::internal());
    assert!(
        matches!(result, Err(BackendError::CommitNotApplied { .. })),
        "{result:?}"
    );
    assert!(
        !root.join(COMMIT_RECORD_REL).exists(),
        "the record is removed"
    );

    assert_eq!(
        backend.current_head().unwrap().as_deref(),
        Some(COMMIT_PENDING_HEAD)
    );
    assert!(backend.read_entity(Path::new("notes/old.md")).is_err());
    backend.write_entity(Path::new("later.md"), NEW).unwrap();
    assert!(backend.commit("later", &CommitContext::internal()).is_err());
    assert!(!root.join("later.md").exists());

    let completed = backend.complete_interrupted_commit().unwrap();
    assert_eq!(
        completed.map(|c| c.files),
        Some(vec!["notes/old.md".to_string()])
    );
    assert_eq!(backend.current_head().unwrap(), before);
    assert_eq!(
        backend.read_entity(Path::new("notes/old.md")).unwrap(),
        Some(FRESH.to_vec())
    );
    assert_eq!(backend.complete_interrupted_commit().unwrap(), None);
}

/// A provenance record appended on its own is a commit of one ledger
/// line: an IO error before its commit point leaves the ledger as it
/// was, an IO error after it is the decided-and-not-applied condition,
/// and the completion lands the line exactly once.
#[test]
fn a_record_appended_on_its_own_goes_through_the_commit_point() {
    let line = || record(at(1_700_000_030), ProvenanceKind::Delete, "m--stub");
    let ledger_of = |root: &Path| std::fs::read(changelog_path(root)).unwrap_or_default();

    let tmp = TempDir::new().unwrap();
    seed(tmp.path());
    let before = ledger_of(tmp.path());
    let io = FaultIo::new();
    let backend = FilesystemBackend::with_io(tmp.path().to_path_buf(), io.clone());
    backend.append_provenance(&line()).unwrap();
    let after = ledger_of(tmp.path());
    assert_ne!(after, before);
    let point = io.commit_point().expect("the append has a commit point");
    let steps = io.op_count();

    for n in 0..steps {
        let tmp = TempDir::new().unwrap();
        seed(tmp.path());
        let backend = FilesystemBackend::with_io(tmp.path().to_path_buf(), FaultIo::fail_at(n));
        let result = backend.append_provenance(&line());
        if n <= point {
            assert!(
                matches!(result, Err(BackendError::Io(_))),
                "step {n}: {result:?}"
            );
            assert_eq!(ledger_of(tmp.path()), before, "step {n}");
            assert!(temp_files(&tree(tmp.path())).is_empty(), "step {n}");
        } else {
            assert!(
                matches!(result, Err(BackendError::CommitNotApplied { .. })),
                "step {n}: {result:?}"
            );
            backend
                .complete_interrupted_commit()
                .unwrap_or_else(|e| panic!("step {n}: {e}"));
            assert_eq!(ledger_of(tmp.path()), after, "step {n}");
        }
    }
}

/// `.memstead/` can exist before a mem's first commit without ever
/// having been synced: the write lock creates it for its lock file. The
/// first commit of a backend therefore syncs the mem root before it
/// puts a record into the directory, whoever made it, and also when
/// the commit is a ledger line and no file (the delete of a stub).
#[test]
fn the_first_commit_names_the_metadata_directory_durably() {
    let record_rename = |trace: &[IoOp], root: &Path| {
        trace
            .iter()
            .position(
                |op| matches!(op, IoOp::Rename { to, .. } if to == &root.join(COMMIT_RECORD_REL)),
            )
            .expect("the commit puts its record in place")
    };

    // The directory as the write lock leaves it.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join(".memstead/locks")).unwrap();
    let io = FaultIo::new();
    let backend = FilesystemBackend::with_io(root.clone(), io.clone());
    backend
        .append_provenance(&record(
            at(1_700_000_030),
            ProvenanceKind::Delete,
            "m--stub",
        ))
        .unwrap();
    let trace = io.trace();
    let point = record_rename(&trace, &root);
    assert!(
        trace[..point].contains(&IoOp::SyncDir(root.clone())),
        "the mem root is not synced before the commit point: {:?}",
        &trace[..point]
    );

    // Once per backend: the next commit has no name to make durable.
    io.reset();
    backend.write_entity(Path::new("a.md"), NEW).unwrap();
    backend
        .commit("second", &CommitContext::internal())
        .unwrap();
    let trace = io.trace();
    let point = record_rename(&trace, &root);
    assert!(
        trace[..point].contains(&IoOp::SyncDir(root.clone())),
        "a.md's own directory"
    );
    io.reset();
    backend.write_entity(Path::new("sub/b.md"), NEW).unwrap();
    backend.commit("third", &CommitContext::internal()).unwrap();
    let trace = io.trace();
    // `sub/` is new, so the root is synced for its name; nothing else is.
    assert_eq!(
        trace
            .iter()
            .filter(|op| **op == IoOp::SyncDir(root.clone()))
            .count(),
        1
    );
}

/// A commit into a directory that an earlier, cut-off attempt left
/// behind: the directory exists, so this commit does not create it, and
/// nobody ever synced its name. The commit syncs every directory from
/// the file's own up to the mem root before its commit point.
#[test]
fn a_commit_into_a_directory_a_cut_off_attempt_left_behind_names_it_durably() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    seed(&root);
    // What a kill during the attempt's first temp write leaves: the
    // directories, made and never synced.
    std::fs::create_dir_all(root.join("fresh/deep")).unwrap();

    let io = FaultIo::new();
    let backend = FilesystemBackend::with_io(root.clone(), io.clone());
    // Not this backend's first commit, which syncs the root anyway.
    backend.write_entity(Path::new("warm-up.md"), NEW).unwrap();
    backend
        .commit("warm-up", &CommitContext::internal())
        .unwrap();

    io.reset();
    backend
        .write_entity(Path::new("fresh/deep/new.md"), FRESH)
        .unwrap();
    backend
        .commit("under test", &CommitContext::internal())
        .unwrap();
    let trace = io.trace();
    let point = io.commit_point().unwrap();
    for dir in [root.join("fresh/deep"), root.join("fresh"), root.clone()] {
        assert!(
            trace[..point].contains(&IoOp::SyncDir(dir.clone())),
            "{dir:?} is not synced before the commit point: {:?}",
            &trace[..point]
        );
    }
}
