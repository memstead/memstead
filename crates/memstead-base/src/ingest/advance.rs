//! `projection advance` — the disposition-gated, resumable baseline advance
//! (bundle plan `03-projection-promotion`, decision D7).
//!
//! An ingest/sync agent works the changed slice a brief presented, then records
//! a **disposition** for every artifact it judged. `advance_baseline` is the
//! engine primitive behind `memstead projection advance`: it freezes the
//! presented slice, subtracts already-disposed artifacts on re-presentation,
//! appends new-HEAD deltas when the source moves mid-pass, and — when the
//! remainder empties — advances the destination mem's `#synced` baseline token
//! through the existing [`Engine::set_mem_sync_state`] writer.
//!
//! ## Durability (why not `.memstead.cache/`)
//!
//! Dispositions are **not** disposable: losing them recreates the stall the
//! redesign exists to kill. The frozen-slice snapshot + accumulated dispositions
//! live under engine-owned **workspace state**,
//! `.memstead/state/advance/<mem>/<name>.json` — a sibling of `state/mounts.json`
//! and valid on both backends — read fresh from disk per call, so resumability
//! is on-disk, not in-memory: a disposition recorded in one process is honored
//! by the next.
//!
//! ## The gate (atomic, engine-printed ids only)
//!
//! The advance gate accepts **only** artifact ids the engine itself printed
//! (the frozen slice, grown by any new-HEAD deltas). A disposition naming an id
//! the engine never presented refuses the **whole call atomically** — validated
//! before any disk write, so a refused call leaves the store byte-identical.
//!
//! ## Auto-`worked` from anchors (E3a — closes plan 03 D7's deferral)
//!
//! With anchors live, a mutation that carried `anchors[]` during a run records,
//! in the destination mem's anchors sidecar, which source artifacts an entity
//! now describes. [`advance_baseline`] reads that sidecar and marks any
//! **frozen-slice** artifact referenced by such an anchor `worked`
//! automatically, so the advance gate requires an explicit disposition only for
//! the residue. Two invariants keep this honest:
//!
//! - the derivation reads **anchors, never a commit diff** — the inference-from-
//!   diffs mechanism D7 rejected stays rejected; a write without `anchors[]`
//!   marks nothing;
//! - only the intersection with the frozen slice is ever marked — an anchored
//!   write referencing an artifact outside the presented slice fabricates no
//!   slice entry.
//!
//! AC9's non-stalling property still rests on the persisted dispositions + slice
//! subtraction; auto-`worked` only removes the explicit-disposition burden for
//! artifacts anchored during the same pass.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::Engine;
use crate::workspace_store::{StoreError, WORKSPACE_STORE_DIR};

use super::cursor::{compute_source_cursor, enumerate_facet_files};
use super::resolve::{ResolvedIngest, ResolvedSource};
use super::slice::Slice;

/// The engine-owned state directory for advance stores, under the workspace
/// store: `<root>/.memstead/state/advance/`.
const STATE_DIR: &str = "state";
/// See [`STATE_DIR`].
const ADVANCE_DIR: &str = "advance";

/// One binding's durable advance state (D7) — the frozen presented slice and
/// the dispositions accumulated against it. Persisted at
/// `.memstead/state/advance/<mem>/<name>.json`, read fresh per call.
///
/// The frozen slice is the **union** of every slice the engine has presented
/// for this advance session (the initial freeze plus any new-HEAD deltas
/// appended as the source moved). Its member ids are exactly the artifact ids
/// the advance gate accepts.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdvanceState {
    /// The canonical binding id `<mem>/<stem>` (D3) this state belongs to.
    pub binding: String,
    /// The frozen presented slice (union of freeze + appended new-HEAD deltas).
    pub frozen_slice: Slice,
    /// artifact id → agent-supplied disposition, accumulated across calls.
    pub dispositions: BTreeMap<String, String>,
    /// The **durable authored-exclusion ledger**: artifact id → the agent's
    /// rationale for deliberately excluding it (mined, warrants no destination
    /// entity). Unlike [`Self::dispositions`] and [`Self::frozen_slice`] — the
    /// transient advance progress dropped on completion — this survives
    /// completion so the fidelity report consults it under exhaustive coverage:
    /// an excluded-on-purpose artifact stops re-surfacing as `uncovered` and
    /// keeps its reasoning. Generic across every binding and medium.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exclusions: BTreeMap<String, String>,
}

/// The verdict marking an artifact **deliberately excluded** from coverage —
/// mined, warrants no destination entity. When supplied with a rationale (the
/// [`DispositionInput::Reasoned`] form) it lands in the durable authored
/// exclusion ledger ([`AdvanceState::exclusions`]) and persists past advance
/// completion; any other verdict clears a prior exclusion for that artifact.
pub const EXCLUDED_VERDICT: &str = "excluded";

/// An agent-supplied disposition for one artifact: either a bare verdict
/// (`"worked"`, `"skipped"`, …) or a verdict carrying an authored rationale.
///
/// The rationale-bearing form exists for the durable authored-exclusion record
/// the option-(a) design names — `(artifact, disposition = "excluded",
/// rationale)`. It is generic: any verdict may carry reasoning, but only the
/// [`EXCLUDED_VERDICT`] one is retained past completion (an excluded artifact
/// has no anchor, so under exhaustive coverage it would otherwise re-surface as
/// `uncovered` on every subsequent verify). Serde is `untagged` so the common
/// `"worked"` form and the `{"disposition": "...", "rationale": "..."}` form
/// both parse from the same `--dispositions` payload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum DispositionInput {
    /// A bare verdict string, e.g. `"worked"`.
    Verdict(String),
    /// A verdict with an authored rationale.
    Reasoned {
        /// The verdict proper (e.g. `"excluded"`).
        disposition: String,
        /// The agent's reasoning for this disposition.
        rationale: String,
    },
}

impl DispositionInput {
    /// The verdict string (the disposition proper).
    pub fn verdict(&self) -> &str {
        match self {
            DispositionInput::Verdict(v) => v,
            DispositionInput::Reasoned { disposition, .. } => disposition,
        }
    }

    /// The authored rationale, if the reasoned form was supplied.
    pub fn rationale(&self) -> Option<&str> {
        match self {
            DispositionInput::Verdict(_) => None,
            DispositionInput::Reasoned { rationale, .. } => Some(rationale),
        }
    }
}

impl AdvanceState {
    /// Count of accumulated dispositions — the `disposed` figure `memstead
    /// status` reports for this binding (D11).
    pub fn disposed(&self) -> usize {
        self.dispositions.len()
    }

    /// Count of frozen-slice artifacts not yet disposed — the `pending`
    /// remainder `memstead status` reports (D11). Same subtraction the
    /// re-presentation applies ([`subtract_disposed`]), collapsed to a count.
    pub fn pending(&self) -> usize {
        artifact_set(&self.frozen_slice)
            .iter()
            .filter(|a| !self.dispositions.contains_key(a.as_str()))
            .count()
    }
}

/// The outcome of an [`advance_baseline`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvanceOutcome {
    /// The binding id advanced.
    pub binding: String,
    /// The re-presented remainder — the frozen slice with every disposed
    /// artifact removed (disposed artifacts absent, D7). Empty when complete.
    pub remainder: Slice,
    /// Total dispositions accumulated (this call + prior, persisted).
    pub disposed: usize,
    /// Remaining (undisposed) artifact count — `remainder`'s total size.
    pub pending: usize,
    /// True when the remainder emptied this call: the `#synced` token(s)
    /// advanced through the engine writer and the durable store was dropped.
    pub completed: bool,
    /// The `sync_state` keys whose baseline token advanced on completion
    /// (empty on a non-completing call, or when the source had not moved).
    pub tokens_written: Vec<String>,
    /// Warnings surfaced by the underlying `set_mem_sync_state` writes (e.g.
    /// `MEM_RELOADED` drift notices), rendered to strings.
    pub warnings: Vec<String>,
}

/// Why [`advance_baseline`] could not complete.
#[derive(Debug, thiserror::Error)]
pub enum AdvanceError {
    /// The binding id is not the canonical `<mem>/<stem>` shape.
    #[error("malformed binding id '{0}': expected `<mem>/<stem>`")]
    MalformedId(String),
    /// One or more disposition ids were never presented by the engine — the
    /// gate refuses the whole call (no partial write). Names each offending id.
    #[error(
        "disposition names {} artifact id(s) the engine did not present: {}; the advance gate \
         accepts only ids from the presented slice ({printed} presented)",
        artifacts.len(),
        fmt_list(artifacts)
    )]
    UnknownArtifact {
        /// The offending, never-presented ids (sorted).
        artifacts: Vec<String>,
        /// How many ids the engine did present (the accepted set size).
        printed: usize,
    },
    /// Reading or writing the durable advance store failed.
    #[error("advance store error: {0}")]
    Store(#[source] StoreError),
    /// The `set_mem_sync_state` baseline write failed on completion.
    #[error("could not advance baseline token: {0}")]
    Engine(String),
}

/// Render an id list for an error message: `a, b, c` or `(none)`.
fn fmt_list(names: &[String]) -> String {
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

/// Split a canonical binding id `<mem>/<stem>` into its two single-component
/// halves, or refuse. Mirrors the store's component guard so a caller-supplied
/// id can never escape the `.memstead/state/advance/` tier.
fn split_binding_id(binding_id: &str) -> Result<(String, String), AdvanceError> {
    binding_id
        .split_once('/')
        .filter(|(m, n)| is_single_component(m) && is_single_component(n))
        .map(|(m, n)| (m.to_string(), n.to_string()))
        .ok_or_else(|| AdvanceError::MalformedId(binding_id.to_string()))
}

/// Is `value` a single, plain path component — safe as a `<mem>` / `<name>`
/// directory or file segment? (No separators, traversal segments, drive/stream
/// colon, or NUL.) Shared with the findings store's identical path guard.
pub(crate) fn is_single_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(':')
        && !value.contains('\0')
}

/// The durable store path for a binding: `.memstead/state/advance/<mem>/<name>.json`.
pub fn advance_store_path(workspace_root: &Path, mem: &str, name: &str) -> PathBuf {
    workspace_root
        .join(WORKSPACE_STORE_DIR)
        .join(STATE_DIR)
        .join(ADVANCE_DIR)
        .join(mem)
        .join(format!("{name}.json"))
}

/// Read the durable advance state for a binding, or `None` when none exists
/// (never advanced, or completed and dropped). A malformed file surfaces a
/// typed [`StoreError::Parse`] naming the path.
pub fn read_advance_store(
    workspace_root: &Path,
    mem: &str,
    name: &str,
) -> Result<Option<AdvanceState>, StoreError> {
    let path = advance_store_path(workspace_root, mem, name);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| StoreError::Parse {
                path,
                message: e.to_string(),
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StoreError::Io { path, source: e }),
    }
}

/// Persist the durable advance state for a binding (pretty JSON), creating
/// parent directories.
pub fn write_advance_store(
    workspace_root: &Path,
    mem: &str,
    name: &str,
    state: &AdvanceState,
) -> Result<(), StoreError> {
    // Self-ignoring subtree: this store is per-checkout engine state
    // inside a possibly-tracked workspace (see the findings twin).
    super::findings::ensure_selfignoring_store_dir(
        &workspace_root
            .join(WORKSPACE_STORE_DIR)
            .join(STATE_DIR)
            .join(ADVANCE_DIR),
    )?;
    let path = advance_store_path(workspace_root, mem, name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| StoreError::Parse {
        path: path.clone(),
        message: e.to_string(),
    })?;
    std::fs::write(&path, bytes).map_err(|e| StoreError::Io { path, source: e })
}

/// Drop the durable advance store for a binding (called on completion). A
/// missing file is a successful no-op — completion is idempotent.
pub fn delete_advance_store(
    workspace_root: &Path,
    mem: &str,
    name: &str,
) -> Result<(), StoreError> {
    let path = advance_store_path(workspace_root, mem, name);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::Io { path, source: e }),
    }
}

/// Union `from` into `into`, keeping each class sorted + de-duplicated.
fn union_slice(into: &mut Slice, from: &Slice) {
    into.added.extend(from.added.iter().cloned());
    into.modified.extend(from.modified.iter().cloned());
    into.deleted.extend(from.deleted.iter().cloned());
    for v in [&mut into.added, &mut into.modified, &mut into.deleted] {
        v.sort();
        v.dedup();
    }
}

/// The full set of artifact ids a slice presents (across all three classes) —
/// the accepted set for the advance gate.
fn artifact_set(slice: &Slice) -> BTreeSet<String> {
    slice
        .added
        .iter()
        .chain(slice.modified.iter())
        .chain(slice.deleted.iter())
        .cloned()
        .collect()
}

/// The remainder slice: the frozen slice with every disposed id removed from
/// each class (disposed artifacts absent, D7).
fn subtract_disposed(frozen: &Slice, dispositions: &BTreeMap<String, String>) -> Slice {
    let keep = |v: &[String]| -> Vec<String> {
        v.iter()
            .filter(|a| !dispositions.contains_key(*a))
            .cloned()
            .collect()
    };
    Slice {
        added: keep(&frozen.added),
        modified: keep(&frozen.modified),
        deleted: keep(&frozen.deleted),
    }
}

/// The disposition-gated baseline advance (D7).
///
/// Freezes the currently-presented slice (or reloads a frozen one), appends any
/// new-HEAD deltas, gates the supplied dispositions against the presented ids
/// (atomic — an unknown id refuses before any write), accumulates them, and
/// re-presents the remainder with disposed artifacts absent. When the remainder
/// empties, the destination mem's `#synced` baseline token(s) advance through
/// the engine's [`Engine::set_mem_sync_state`] writer — the provenance
/// piggybacks that write's commit note, adding no new channel — and the durable
/// store is dropped.
///
/// `resolved.name` must be the canonical binding id `<mem>/<stem>` (D3), as
/// produced by [`super::resolve::resolve_binding_run`]; `dispositions` maps each
/// judged artifact id to an agent-supplied [`DispositionInput`] — a bare verdict
/// or a verdict with an authored rationale (in E2 the agent supplies one for
/// **every** artifact — see the module docs). An `excluded` verdict with a
/// rationale is recorded in the durable authored-exclusion ledger; any other
/// verdict clears a prior exclusion for that artifact.
pub fn advance_baseline(
    engine: &mut Engine,
    workspace_root: &Path,
    resolved: &ResolvedIngest,
    dispositions: &BTreeMap<String, DispositionInput>,
) -> Result<AdvanceOutcome, AdvanceError> {
    let binding_id = resolved.name.clone();
    let (mem, name) = split_binding_id(&binding_id)?;

    // Current source cursor (immutable borrow ends before the mutating writes).
    // Its union is the slice relative to the *unchanged* `#synced` baseline, so
    // when the source moves mid-pass this already reflects freeze + new deltas.
    let cursor = compute_source_cursor(engine, resolved, workspace_root);

    // Load-or-init the durable store (resumability is on-disk, not in-memory).
    let mut state = read_advance_store(workspace_root, &mem, &name)
        .map_err(AdvanceError::Store)?
        .unwrap_or_else(|| AdvanceState {
            binding: binding_id.clone(),
            ..Default::default()
        });

    // Freeze / append: union the currently-presented slice into the frozen one.
    union_slice(&mut state.frozen_slice, &cursor.union);
    let printed = artifact_set(&state.frozen_slice);

    // Gate (atomic): every disposition id must be one the engine presented.
    // Validate BEFORE any disk write so a refusal leaves the store untouched.
    let mut unknown: Vec<String> = dispositions
        .keys()
        .filter(|a| !printed.contains(a.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        unknown.dedup();
        return Err(AdvanceError::UnknownArtifact {
            artifacts: unknown,
            printed: printed.len(),
        });
    }

    // Accumulate the new (agent-supplied) dispositions. An `excluded` verdict
    // with a rationale lands in the durable exclusion ledger (survives
    // completion); any other verdict clears a prior exclusion for that artifact
    // (a re-judged artifact must not keep stale "excluded" reasoning).
    for (artifact, input) in dispositions {
        state
            .dispositions
            .insert(artifact.clone(), input.verdict().to_string());
        if input.verdict() == EXCLUDED_VERDICT {
            state.exclusions.insert(
                artifact.clone(),
                input.rationale().unwrap_or("").to_string(),
            );
        } else {
            state.exclusions.remove(artifact);
        }
    }

    // Auto-`worked` (E3a): mark every frozen-slice artifact that an anchor in
    // the destination mem now references. Reads the anchors sidecar, never a
    // commit diff (D7's rejected mechanism stays rejected); scoped to the
    // frozen slice (`printed`) so an anchored write outside the slice
    // fabricates no entry; skips artifacts already carrying an explicit
    // disposition (the agent's judgement wins).
    let auto_worked: Vec<String> = printed
        .iter()
        .filter(|art| !state.dispositions.contains_key(art.as_str()))
        .filter(|art| {
            engine
                .anchors_referencing_artifact(art)
                .iter()
                .any(|(eid, _)| eid.mem() == resolved.destination_mem.as_str())
        })
        .cloned()
        .collect();
    for art in auto_worked {
        state.dispositions.insert(art, "worked".to_string());
    }

    // Re-present the remainder (disposed absent).
    let remainder = subtract_disposed(&state.frozen_slice, &state.dispositions);
    let pending = remainder.added.len() + remainder.modified.len() + remainder.deleted.len();
    let completed = pending == 0;

    let mut warnings: Vec<String> = Vec::new();
    let mut tokens_written: Vec<String> = Vec::new();
    if completed {
        // Advance the baseline token for every facet that moved (current cursor
        // tokens = the latest HEAD) via the engine writer. Provenance piggybacks
        // the write's commit note — no new channel (D7).
        let note = format!(
            "projection advance {binding_id}: {} artifact(s) disposed, baseline advanced",
            state.dispositions.len()
        );
        for c in cursor.write_commands.iter().chain(cursor.reseed.iter()) {
            let outcome = engine
                .set_mem_sync_state(&resolved.destination_mem, &c.key, &c.token, Some(&note))
                .map_err(|e| AdvanceError::Engine(e.to_string()))?;
            warnings.extend(outcome.warnings.iter().map(ToString::to_string));
            tokens_written.push(c.key.clone());
        }
        // Transient progress (frozen slice + per-run dispositions) is consumed.
        // If any durable authored exclusions accumulated, retain a slimmed store
        // holding only them (empty slice, no transient dispositions) so the
        // fidelity report keeps consulting them; otherwise drop the store
        // entirely (completion idempotent — the no-exclusion path is unchanged).
        if state.exclusions.is_empty() {
            delete_advance_store(workspace_root, &mem, &name).map_err(AdvanceError::Store)?;
        } else {
            let durable = AdvanceState {
                binding: binding_id.clone(),
                frozen_slice: Slice::default(),
                dispositions: BTreeMap::new(),
                exclusions: state.exclusions.clone(),
            };
            write_advance_store(workspace_root, &mem, &name, &durable)
                .map_err(AdvanceError::Store)?;
        }
    } else {
        // Persist the accumulated frozen slice + dispositions for resumability.
        write_advance_store(workspace_root, &mem, &name, &state).map_err(AdvanceError::Store)?;
    }

    Ok(AdvanceOutcome {
        binding: binding_id,
        remainder,
        disposed: state.dispositions.len(),
        pending,
        completed,
        tokens_written,
        warnings,
    })
}

/// The outcome of a [`record_exclusions`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludeOutcome {
    /// The binding id whose exclusion ledger was written.
    pub binding: String,
    /// Total authored exclusions in the ledger after this call (this call + prior).
    pub excluded: usize,
    /// How many supplied artifacts were newly added (not already in the ledger).
    pub added: usize,
}

/// Why [`record_exclusions`] could not complete.
#[derive(Debug, thiserror::Error)]
pub enum ExcludeError {
    /// The binding id is not the canonical `<mem>/<stem>` shape.
    #[error("malformed binding id '{0}': expected `<mem>/<stem>`")]
    MalformedId(String),
    /// One or more artifacts are not members of the binding's enumerable source
    /// `S(D)` — the gate refuses the whole call (no partial write). Names each.
    #[error(
        "exclusion names {} artifact id(s) not in the binding's enumerable source S(D): {}; \
         only an in-scope source member can be declared excluded ({printed} enumerated)",
        artifacts.len(),
        fmt_list(artifacts)
    )]
    NotSourceMember {
        /// The offending, non-member ids (sorted).
        artifacts: Vec<String>,
        /// How many artifacts `S(D)` did enumerate (the accepted set size).
        printed: usize,
    },
    /// Reading or writing the durable advance store failed.
    #[error("advance store error: {0}")]
    Store(#[source] StoreError),
}

/// Declare **authored exclusions** for in-scope source artifacts — the direct
/// write path for the durable exclusion ledger [`advance_baseline`] also feeds.
///
/// Unlike the advance gate (which accepts only artifacts in the *changed slice*),
/// this gates on **enumerable `S(D)` membership**: an artifact must be a real
/// in-scope member of the binding's source, and a *stable, unchanged* artifact
/// qualifies. That is what a deliberate editorial exclusion is — "this in-scope
/// artifact is mined and warrants no destination entity, because …" — a decision
/// independent of change detection. Each accepted `(artifact, rationale)` lands
/// in the ledger the fidelity report consults, so the artifact stops re-surfacing
/// as `uncovered` under exhaustive coverage and keeps its reasoning. Atomic: an
/// artifact outside `S(D)` refuses the whole call before any write. Merges into
/// any in-flight advance store rather than clobbering it. Generic across every
/// enumerable binding and medium.
pub fn record_exclusions(
    workspace_root: &Path,
    resolved: &ResolvedIngest,
    exclusions: &BTreeMap<String, String>,
) -> Result<ExcludeOutcome, ExcludeError> {
    let binding_id = resolved.name.clone();
    let (mem, name) =
        split_binding_id(&binding_id).map_err(|_| ExcludeError::MalformedId(binding_id.clone()))?;

    // Enumerate S(D) — the in-scope source-artifact set, the same enumeration the
    // fidelity report uses for its coverage denominator.
    let mut s_d: BTreeSet<String> = BTreeSet::new();
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source {
            for f in enumerate_facet_files(p, &resolved.deny_paths, workspace_root) {
                s_d.insert(f);
            }
        }
    }

    // Gate (atomic): every exclusion id must be an S(D) member. Validate BEFORE
    // any disk write so a refusal leaves the store untouched.
    let mut not_member: Vec<String> = exclusions
        .keys()
        .filter(|a| !s_d.contains(a.as_str()))
        .cloned()
        .collect();
    if !not_member.is_empty() {
        not_member.sort();
        not_member.dedup();
        return Err(ExcludeError::NotSourceMember {
            artifacts: not_member,
            printed: s_d.len(),
        });
    }

    // Merge into the durable exclusion ledger, preserving any in-flight advance
    // progress already in the same store.
    let mut state = read_advance_store(workspace_root, &mem, &name)
        .map_err(ExcludeError::Store)?
        .unwrap_or_else(|| AdvanceState {
            binding: binding_id.clone(),
            ..Default::default()
        });
    let mut added = 0usize;
    for (artifact, rationale) in exclusions {
        if state
            .exclusions
            .insert(artifact.clone(), rationale.clone())
            .is_none()
        {
            added += 1;
        }
    }
    write_advance_store(workspace_root, &mem, &name, &state).map_err(ExcludeError::Store)?;

    Ok(ExcludeOutcome {
        binding: binding_id,
        excluded: state.exclusions.len(),
        added,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::BuildMode;
    use crate::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode};
    use crate::storage::FilesystemMemWriter;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
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
        };
        write_advance_store(root, "engine", "graph", &state).unwrap();
        assert!(
            advance_store_path(root, "engine", "graph")
                .ends_with("state/advance/engine/graph.json")
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
        use super::super::resolve::{ResolvedPrimarySource, ResolvedSource};
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
            sources: vec![ResolvedSource::Primary(ResolvedPrimarySource {
                facet_ref: "source-tree".to_string(),
                medium: "src".to_string(),
                medium_type: MediumType::Codebase,
                medium_pointer: String::new(),
                declared_change_detection: Some("git".to_string()),
                scope: vec![PatternEntry {
                    path: "**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
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
            Box::new(FilesystemMemWriter::new(root.to_path_buf()))
                as Box<dyn crate::backend::MemBackend>,
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
            let out = advance_baseline(&mut engine, root, &resolved, &input(&[("a.rs", "worked")]))
                .unwrap();
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
    fn advance_retains_authored_exclusions_past_completion_and_clears_on_rejudge() {
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

        // Move to head2 (modify a.rs again) → a.rs re-enters the slice → re-judge
        // it as `worked`. The non-excluded verdict clears the stale exclusion, and
        // with nothing durable left the store is dropped.
        std::fs::write(root.join("a.rs"), "one-longer-still").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "head2"]);
        {
            let mut engine = engine_at(root);
            let out = advance_baseline(&mut engine, root, &resolved, &input(&[("a.rs", "worked")]))
                .unwrap();
            assert!(out.completed);
        }
        assert!(
            read_advance_store(root, "engine", "graph")
                .unwrap()
                .is_none(),
            "re-judging the artifact cleared the exclusion; nothing durable remains"
        );
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
            root,
            &resolved,
            &BTreeMap::from([("b.rs".to_string(), "also mined".to_string())]),
        )
        .unwrap();
        assert_eq!((out2.added, out2.excluded), (1, 2));
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

    /// AC9a — an anchored write auto-marks its referenced frozen-slice
    /// artifacts `worked`, so `advance` needs an explicit disposition only for
    /// the residue, held across a HEAD move. Refusals: an artifact with no
    /// anchor is never auto-worked, and an anchor referencing an artifact
    /// OUTSIDE the presented slice fabricates no slice entry.
    #[test]
    fn advance_auto_worked_from_anchors_subtracts_slice_and_never_fabricates() {
        use crate::vcs::Actor;
        use indexmap::IndexMap;

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
        let make_anchor = |artifact: &str| crate::anchor::AnchorInput {
            artifact: Some(artifact.to_string()),
            grain: Some("file".to_string()),
            class: Some("anchored".to_string()),
            hash: Some("h".to_string()),
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
                    crate::CreateEntityArgs {
                        mem: "engine".to_string(),
                        title: "Covers A".to_string(),
                        entity_type: "spec".to_string(),
                        sections,
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        anchors: vec![make_anchor("a.rs"), make_anchor("zzz.rs")],
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
}
