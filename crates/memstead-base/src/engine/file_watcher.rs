//! Cross-process file-watcher convenience for the change-event
//! surface.
//!
//! [`watch_mem_repo`] starts a `notify`-backed file-system watcher
//! against `<gitdir>/refs/heads/` and surfaces a
//! [`std::sync::mpsc::Receiver`] of [`MemChangedEvent`]s. Consumers
//! that do not share the writer's [`crate::Engine`] instance (the
//! bridge HEAD-watcher, the macOS live-update path, audit-log workers,
//! webhook notifiers) consume events through this surface; the wire
//! shape matches the in-process callback API exactly so the same
//! downstream code paths work for both sources.
//!
//! Gated behind the `file-watcher` Cargo feature — `notify` is a
//! non-trivial dependency and consumers that only need the in-process
//! [`crate::Engine::subscribe_mem_changes`] path should not pay for
//! it. Without the feature enabled the module disappears entirely.
//!
//! ## Design
//!
//! * `watch_mem_repo` spawns a background thread running the
//!   notify-event loop and returns a [`MemRepoWatcher`] handle that
//!   owns the `notify::PollWatcher` (chosen over `RecommendedWatcher`
//!   for cross-platform determinism — see the rationale at the
//!   `PollWatcher::new` call site). A per-mem
//!   `HashMap<mem_name, last_seen_sha>` (a shared `Arc<Mutex<…>>`
//!   seeded before the thread starts) lets each emitted event's
//!   `previous` field reflect the actual transition.
//! * On startup the thread scans the existing `refs/heads/` tree and
//!   seeds the SHA map without emitting synthetic history events —
//!   consumers see changes from this point forward, never replay.
//! * Per file-system event the thread re-reads the touched ref file
//!   (or the directory holding it for hierarchical layouts) and emits
//!   a `MemChangedEvent` if the SHA changed. Notify can deliver
//!   bursts; the SHA-comparison gate deduplicates them.
//! * The returned [`MemRepoWatcher`] handle owns the notify watcher;
//!   dropping it cancels the watch and the background thread joins on
//!   its event channel disconnecting.
//!
//! ## Limitations
//!
//! * Reads loose refs only (the typical case for an actively-written
//!   mem-repo). Packed refs (`<gitdir>/packed-refs`) are not parsed
//!   in v1 — production deployments that compact refs need a
//!   follow-up. Loose refs created after compaction still surface
//!   normally.
//! * Read-only consumers must poll [`crate::ops::changes_since`] for
//!   any history that landed before the watcher started.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use notify::{
    Config, Event, PollWatcher, RecursiveMode, Watcher,
};

use super::events::MemChangedEvent;

/// Poll interval for the underlying `notify` watcher. Chosen so the
/// observed change-event latency stays in the 10–50 ms band the AC
/// targets (typical) while bounded by `< 1 s` (worst case). Higher
/// values reduce CPU cost on idle workspaces; lower values shrink the
/// observation window for active write loops. 50 ms is a comfortable
/// middle ground for the v1 cross-process surface.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Errors surfaced by [`watch_mem_repo`].
#[derive(Debug, thiserror::Error)]
pub enum FileWatcherError {
    /// `<gitdir>/refs/heads` does not exist. The mem-repo has not
    /// been initialised, or the path was wrong.
    #[error("refs/heads directory not found under gitdir: {0}")]
    RefsHeadsMissing(PathBuf),
    /// `notify` failed to start the underlying watcher (permission
    /// denied, FS not supported on the platform, resource exhaustion).
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
    /// IO error while reading an existing ref file during the
    /// initial SHA-map seeding pass.
    #[error("io error reading initial refs/heads state: {0}")]
    Io(#[from] std::io::Error),
}

/// RAII handle returned by [`watch_mem_repo`]. Dropping the handle
/// stops the watcher (the underlying notify watcher is dropped,
/// cancelling the OS-level subscription, and the background thread
/// exits when its event channel disconnects).
///
/// The handle is `Send` but not `Sync` — the consumer threads receive
/// events through the `mpsc::Receiver` returned alongside the handle,
/// not by sharing the handle itself.
pub struct MemRepoWatcher {
    _watcher: PollWatcher,
    // The background-thread join handle is kept so panics inside the
    // event loop surface eventually (on drop the thread's panic
    // propagates if the consumer joins). For v1 we let the OS reap
    // the thread on watcher drop; the panic surfaces through tracing.
    _thread: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for MemRepoWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemRepoWatcher").finish()
    }
}

/// Start a file-system watcher on `<gitdir>/refs/heads/` and surface a
/// receiver of [`MemChangedEvent`]s. See the module docs for the
/// design and the limitations (loose-refs only, no synthetic replay).
///
/// `gitdir` is the bare-repo directory typically named `mem-repo`
/// inside a Memstead workspace. Pass the path that contains the
/// `refs/heads/` tree directly — the function does not auto-discover
/// from a workspace root.
pub fn watch_mem_repo(
    gitdir: &Path,
) -> Result<(MemRepoWatcher, Receiver<MemChangedEvent>), FileWatcherError> {
    let refs_heads = gitdir.join("refs").join("heads");
    if !refs_heads.is_dir() {
        return Err(FileWatcherError::RefsHeadsMissing(refs_heads));
    }

    // Seed the per-mem SHA map from the current state of
    // `refs/heads/` so the first emit per mem carries a correct
    // `previous` value (instead of always empty).
    let state: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(scan_initial_state(&refs_heads)?));

    let (event_tx, event_rx) = channel::<MemChangedEvent>();
    let (notify_tx, notify_rx) = channel::<notify::Result<Event>>();

    // Use `PollWatcher` rather than `RecommendedWatcher` for v1: it is
    // deterministic across platforms (no FSEvents/inotify backend
    // quirks under tempdir / sandbox / network volumes), the poll cost
    // is negligible for the small `refs/heads/` tree of a typical
    // mem-repo, and the 10–50 ms latency band the AC targets is
    // achievable with a 50 ms interval. Production deployments that
    // ever need lower latency can switch to `RecommendedWatcher`
    // behind a config knob — left out of v1 to keep the surface small.
    let mut watcher = PollWatcher::new(
        move |res: notify::Result<Event>| {
            // The notify thread sends results through `notify_tx`;
            // ignore send failures (consumer thread already exited).
            let _ = notify_tx.send(res);
        },
        Config::default()
            .with_poll_interval(POLL_INTERVAL)
            // Compare file contents rather than just mtime so writes
            // within the same OS-level mtime tick (1 s on some
            // filesystems / macOS HFS+ legacy paths) still surface as
            // events. Cheap on the small `refs/heads/` tree.
            .with_compare_contents(true),
    )?;
    watcher.watch(&refs_heads, RecursiveMode::Recursive)?;

    let refs_heads_for_thread = refs_heads.clone();
    let state_for_thread = state.clone();
    let event_tx_for_thread = event_tx;
    let join = thread::Builder::new()
        .name("memstead-mem-repo-watcher".to_string())
        .spawn(move || {
            run_event_loop(
                &refs_heads_for_thread,
                state_for_thread,
                notify_rx,
                event_tx_for_thread,
            );
        })
        .expect("spawning file-watcher thread must succeed");

    Ok((
        MemRepoWatcher {
            _watcher: watcher,
            _thread: Some(join),
        },
        event_rx,
    ))
}

/// Walk the initial `refs/heads/` tree and read each loose ref's
/// 40-char SHA. The map this returns seeds the per-mem state so the
/// first event emitted for any mem carries the right `previous`
/// value (rather than always an empty string).
fn scan_initial_state(refs_heads: &Path) -> Result<HashMap<String, String>, std::io::Error> {
    let mut out = HashMap::new();
    scan_dir(refs_heads, refs_heads, &mut out)?;
    Ok(out)
}

fn scan_dir(
    base: &Path,
    dir: &Path,
    out: &mut HashMap<String, String>,
) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            scan_dir(base, &path, out)?;
        } else if file_type.is_file()
            && let Some(name) = mem_name_for_ref_path(base, &path)
            && let Some(sha) = read_ref_sha(&path)
        {
            out.insert(name, sha);
        }
    }
    Ok(())
}

/// Derive the mem name from a `refs/heads/<...>` path. Returns
/// `None` when the path is outside `base` (defensive — should not
/// happen in practice). Hierarchical layouts (`refs/heads/path/leaf`)
/// produce `path/leaf` as the mem name; flat layouts (the typical
/// case) produce the file basename.
fn mem_name_for_ref_path(base: &Path, ref_path: &Path) -> Option<String> {
    let rel = ref_path.strip_prefix(base).ok()?;
    Some(
        rel.components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Read a loose-ref file's content and parse it as a 40-character
/// hex SHA. Returns `None` for unexpected shapes (symbolic refs like
/// `ref: refs/heads/main`, empty files mid-write, content longer than
/// 41 bytes). Loose refs in a healthy git repo are always either a
/// raw SHA or a `ref:` line — symbolic refs in `refs/heads/` are not
/// emitted as mem changes.
fn read_ref_sha(ref_path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(ref_path).ok()?;
    let trimmed = raw.trim();
    if trimmed.len() != 40 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Background-thread event loop. Reads notify events off `notify_rx`,
/// re-reads the touched ref files to determine the new SHA, and emits
/// a [`MemChangedEvent`] to `event_tx` whenever a SHA changes.
fn run_event_loop(
    refs_heads: &Path,
    state: Arc<Mutex<HashMap<String, String>>>,
    notify_rx: Receiver<notify::Result<Event>>,
    event_tx: Sender<MemChangedEvent>,
) {
    while let Ok(item) = notify_rx.recv() {
        let Ok(event) = item else { continue };
        // No kind filter — `PollWatcher`'s event kinds are
        // backend-defined and we re-read every touched file's SHA
        // anyway. The dedupe gate on `(previous, new_sha)` below is
        // what guarantees idempotent re-writes don't surface as
        // changes. Remove-style events trip the early-exit when the
        // path is no longer a file.
        for path in event.paths {
            if !path.is_file() {
                continue;
            }
            let Some(mem) = mem_name_for_ref_path(refs_heads, &path) else {
                continue;
            };
            let Some(new_sha) = read_ref_sha(&path) else {
                continue;
            };
            let previous = {
                let mut map = state.lock().unwrap();
                let prev = map.get(&mem).cloned().unwrap_or_default();
                if prev == new_sha {
                    continue;
                }
                map.insert(mem.clone(), new_sha.clone());
                prev
            };
            let emission = MemChangedEvent {
                mem,
                head: new_sha,
                previous,
                n_commits: 1,
            };
            if event_tx.send(emission).is_err() {
                // Consumer dropped the receiver — nothing left to do.
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// Wait up to `timeout` for an event whose mem matches
    /// `expected_mem`. Drains and forwards any unrelated event the
    /// watcher may emit during the wait (notify can fire for unrelated
    /// fs noise inside the gitdir tree).
    fn recv_event_for(
        rx: &Receiver<MemChangedEvent>,
        expected_mem: &str,
        timeout: Duration,
    ) -> Option<MemChangedEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.checked_duration_since(Instant::now())?;
            match rx.recv_timeout(remaining) {
                Ok(ev) if ev.mem == expected_mem => return Some(ev),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    fn make_refs_heads(tmp: &TempDir) -> PathBuf {
        let gitdir = tmp.path().join("mem-repo.git");
        std::fs::create_dir_all(gitdir.join("refs").join("heads")).unwrap();
        gitdir
    }

    fn write_ref(refs_heads: &Path, mem: &str, sha: &str) {
        let p = refs_heads.join(mem);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        // Git writes refs as `<sha>\n`. Replicate that so the parser's
        // trim normalises consistently with real-world inputs.
        std::fs::write(p, format!("{sha}\n")).unwrap();
    }

    #[test]
    fn refs_heads_missing_returns_typed_error() {
        let tmp = TempDir::new().unwrap();
        let err = watch_mem_repo(&tmp.path().join("nope")).unwrap_err();
        match err {
            FileWatcherError::RefsHeadsMissing(_) => {}
            other => panic!("expected RefsHeadsMissing, got {other:?}"),
        }
    }

    #[test]
    fn modifying_ref_file_emits_mem_changed_event() {
        let tmp = TempDir::new().unwrap();
        let gitdir = make_refs_heads(&tmp);
        let refs_heads = gitdir.join("refs").join("heads");

        let initial = "1234567890abcdef1234567890abcdef12345678";
        write_ref(&refs_heads, "specs", initial);

        let (_watcher, rx) = watch_mem_repo(&gitdir).unwrap();

        // Update the ref to a new SHA — the watcher should fire.
        let updated = "fedcba9876543210fedcba9876543210fedcba98";
        write_ref(&refs_heads, "specs", updated);

        let event = recv_event_for(&rx, "specs", Duration::from_secs(2))
            .expect("event must arrive within 2s");
        assert_eq!(event.mem, "specs");
        assert_eq!(event.head, updated);
        assert_eq!(event.previous, initial);
        assert_eq!(event.n_commits, 1);
    }

    #[test]
    fn creating_new_ref_file_emits_event_with_empty_previous() {
        let tmp = TempDir::new().unwrap();
        let gitdir = make_refs_heads(&tmp);
        let refs_heads = gitdir.join("refs").join("heads");

        let (_watcher, rx) = watch_mem_repo(&gitdir).unwrap();

        let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        write_ref(&refs_heads, "newmem", sha);

        let event = recv_event_for(&rx, "newmem", Duration::from_secs(2))
            .expect("event must arrive within 2s");
        assert_eq!(event.head, sha);
        assert_eq!(event.previous, "");
    }

    #[test]
    fn idempotent_writes_do_not_emit_duplicate_events() {
        let tmp = TempDir::new().unwrap();
        let gitdir = make_refs_heads(&tmp);
        let refs_heads = gitdir.join("refs").join("heads");

        write_ref(&refs_heads, "specs", "1111111111111111111111111111111111111111");
        let (_watcher, rx) = watch_mem_repo(&gitdir).unwrap();

        // Re-write the same SHA — notify will fire, but the SHA gate
        // in the event loop suppresses the duplicate emission.
        write_ref(&refs_heads, "specs", "1111111111111111111111111111111111111111");

        // Wait briefly; no event for "specs" should arrive.
        assert!(
            recv_event_for(&rx, "specs", Duration::from_millis(200)).is_none(),
            "idempotent re-write must not surface as a change event",
        );
    }

    #[test]
    fn hierarchical_branch_paths_produce_compound_mem_names() {
        let tmp = TempDir::new().unwrap();
        let gitdir = make_refs_heads(&tmp);
        let refs_heads = gitdir.join("refs").join("heads");

        let (_watcher, rx) = watch_mem_repo(&gitdir).unwrap();

        let sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        write_ref(&refs_heads, "team/specs", sha);

        let event = recv_event_for(&rx, "team/specs", Duration::from_secs(2))
            .expect("hierarchical event must arrive");
        assert_eq!(event.mem, "team/specs");
        assert_eq!(event.head, sha);
    }

    #[test]
    fn dropping_watcher_stops_event_delivery() {
        let tmp = TempDir::new().unwrap();
        let gitdir = make_refs_heads(&tmp);
        let refs_heads = gitdir.join("refs").join("heads");

        let (watcher, rx) = watch_mem_repo(&gitdir).unwrap();
        drop(watcher);

        // After the watcher drops, new ref writes should not surface.
        write_ref(&refs_heads, "specs", "cccccccccccccccccccccccccccccccccccccccc");
        assert!(
            recv_event_for(&rx, "specs", Duration::from_millis(200)).is_none(),
            "dropped watcher must not deliver further events",
        );
    }
}
