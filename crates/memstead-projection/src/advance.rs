//! `projection advance` — the disposition-gated, resumable baseline advance.
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
//! ## Auto-`worked` from anchors
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

use memstead_base::Engine;
use memstead_base::workspace_store::{StoreError, WORKSPACE_STORE_DIR};

use super::cursor::{
    compute_source_cursor, enumerate_source_artifacts, enumerate_source_artifacts_reported,
    medium_base, normalize_lexical, relative_path,
};
use super::slice::Slice;
use memstead_base::binding_run::{ResolvedIngest, ResolvedSource};

/// The engine-owned state directory for advance stores, under the workspace
/// store: `<root>/.memstead/state/advance/`.
const STATE_DIR: &str = "state";
/// See [`STATE_DIR`].
const ADVANCE_DIR: &str = "advance";

/// One binding's durable advance state — the frozen presented slice and
/// the dispositions accumulated against it. Persisted at
/// `.memstead/state/advance/<mem>/<name>.json`, read fresh per call.
///
/// The frozen slice is the **union** of every slice the engine has presented
/// for this advance session (the initial freeze plus any new-HEAD deltas
/// appended as the source moved). Its member ids are exactly the artifact ids
/// the advance gate accepts.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdvanceState {
    /// The canonical binding id `<mem>/<stem>` this state belongs to.
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
    /// artifact id → the source facet the exclusion was recorded under
    /// (2026-09-02, basket line 3). An exclusion keys on the artifact and
    /// its source, never on the binding hash: a re-declared source keeps
    /// its exclusions across `projection edit`, a removed source drops
    /// them ([`reconcile_exclusions`]). Entries recorded before the field
    /// existed are attributed on the next reconcile.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exclusion_sources: BTreeMap<String, String>,
    /// The **entity-side exclusion ledger**: destination entity id → the
    /// rationale for it deliberately carrying no anchor (a withdrawn claim
    /// kept as record, an "about this graph" entity, a synthesis no single
    /// artifact backs). The fidelity report's coverage reading names every
    /// destination entity without an anchor; one declared here is named as
    /// excluded with its reason instead of as owed. Keyed by entity, so a
    /// binding edit never moves it; an entity later deleted simply stops
    /// being read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entity_exclusions: BTreeMap<String, String>,
    /// Exclusions the reconcile dropped because their source left the
    /// declaration — kept so the next sync brief reports them with the
    /// source named, then cleared once reported.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_exclusions: Vec<DroppedExclusion>,
}

impl AdvanceState {
    /// Whether the store carries authored exclusions that must outlive a
    /// completed pass: the artifact ledger or the entity ledger. Both guards
    /// that may delete the store ask this, so a ledger added later cannot
    /// be dropped by a guard that still tests only the older one (the
    /// entity ledger was, from 2026-09-05 to 2026-09-06).
    pub fn has_durable_exclusions(&self) -> bool {
        !self.exclusions.is_empty() || !self.entity_exclusions.is_empty()
    }
}

/// One authored exclusion dropped by [`reconcile_exclusions`]: the source it
/// was recorded under is no longer declared on the binding.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DroppedExclusion {
    pub artifact: String,
    /// The facet name the exclusion was recorded under; `unattributed` for
    /// a pre-field entry no declared source enumerates.
    pub source: String,
    pub rationale: String,
    pub dropped_at: String,
}

/// One authored exclusion in force, as the sync brief lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ActiveExclusion {
    pub artifact: String,
    pub source: String,
    pub rationale: String,
}

/// The exclusion ledger after reconciliation against the binding as declared
/// now: what is in force, and what was dropped (reported once).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExclusionLedger {
    pub active: Vec<ActiveExclusion>,
    pub dropped: Vec<DroppedExclusion>,
}

/// Reconcile a binding's authored exclusions against its declared sources:
/// an exclusion whose recorded source is still declared stays in force; one
/// whose source left the declaration is dropped (moved to
/// `dropped_exclusions`, reported by the next brief and cleared after); a
/// pre-field entry with no recorded source is attributed to the declared
/// source that enumerates it, or dropped as `unattributed` when none does.
/// Nothing here keys on the binding hash, so `projection edit` of any other
/// field leaves every exclusion untouched. Writes the store only when
/// something changed; a binding with no store yields an empty ledger.
pub fn reconcile_exclusions(
    engine: &Engine,
    workspace_root: &Path,
    resolved: &ResolvedIngest,
) -> Result<ExclusionLedger, StoreError> {
    let Ok((mem, name)) = split_binding_id(&resolved.name) else {
        return Ok(ExclusionLedger::default());
    };
    let Some(mut state) = read_advance_store(workspace_root, &mem, &name)? else {
        return Ok(ExclusionLedger::default());
    };
    let declared: BTreeSet<String> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(p.name.clone()),
            ResolvedSource::Reference { .. } => None,
        })
        .collect();
    let mut changed = false;
    let mut membership: Option<BTreeMap<String, String>> = None;
    let mut dropped_now: Vec<DroppedExclusion> = Vec::new();
    let mut active: Vec<ActiveExclusion> = Vec::new();
    for (artifact, rationale) in state.exclusions.clone() {
        let recorded = state.exclusion_sources.get(&artifact).cloned();
        let source = match recorded {
            Some(s) if declared.contains(&s) => Some(s),
            Some(s) => {
                dropped_now.push(DroppedExclusion {
                    artifact: artifact.clone(),
                    source: s,
                    rationale: rationale.clone(),
                    dropped_at: memstead_base::engine::mutation::iso_now(),
                });
                None
            }
            None => {
                let facets = membership.get_or_insert_with(|| {
                    let mut m = BTreeMap::new();
                    for s in &resolved.sources {
                        if let ResolvedSource::Primary(p) = s {
                            for f in enumerate_source_artifacts(
                                engine,
                                p,
                                &resolved.deny_paths,
                                workspace_root,
                            ) {
                                m.entry(f).or_insert_with(|| p.name.clone());
                            }
                        }
                    }
                    m
                });
                match facets.get(&artifact).cloned() {
                    Some(f) => {
                        state.exclusion_sources.insert(artifact.clone(), f.clone());
                        changed = true;
                        Some(f)
                    }
                    None => {
                        dropped_now.push(DroppedExclusion {
                            artifact: artifact.clone(),
                            source: "unattributed".to_string(),
                            rationale: rationale.clone(),
                            dropped_at: memstead_base::engine::mutation::iso_now(),
                        });
                        None
                    }
                }
            }
        };
        match source {
            Some(source) => active.push(ActiveExclusion {
                artifact,
                source,
                rationale,
            }),
            None => {
                state.exclusions.remove(&artifact);
                state.exclusion_sources.remove(&artifact);
                changed = true;
            }
        }
    }
    // Report what was dropped now plus what an earlier reconcile dropped and
    // no brief has reported yet; the report clears the record.
    let mut dropped = std::mem::take(&mut state.dropped_exclusions);
    if !dropped.is_empty() {
        changed = true;
    }
    dropped.extend(dropped_now);
    if changed {
        if !state.has_durable_exclusions()
            && state.frozen_slice == Slice::default()
            && state.dispositions.is_empty()
        {
            delete_advance_store(workspace_root, &mem, &name)?;
        } else {
            write_advance_store(workspace_root, &mem, &name, &state)?;
        }
    }
    Ok(ExclusionLedger { active, dropped })
}

/// The verdict marking an artifact **deliberately excluded** from coverage —
/// mined, warrants no destination entity. When supplied with a rationale (the
/// [`DispositionInput::Reasoned`] form) it lands in the durable authored
/// exclusion ledger ([`AdvanceState::exclusions`]) and persists past advance
/// completion; only a reasoned non-excluded verdict lifts it.
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
    /// gate refuses the whole call (no partial write). Names each offending
    /// id, states the expected id dialect (workspace-relative, exactly as the
    /// slice printed), and — when prefixing a supplied id with its medium
    /// root yields an id that IS in the presented slice — carries the
    /// concrete corrected id. The medium-relative form is never accepted:
    /// one id dialect holds across enumeration, anchors, coverage, and
    /// advance.
    #[error(
        "disposition names {} artifact id(s) the engine did not present: {}; the advance gate \
         accepts only ids from the presented slice, verbatim in their workspace-relative form \
         ({printed} presented){}",
        artifacts.len(),
        fmt_list(artifacts),
        fmt_suggestions(suggestions)
    )]
    UnknownArtifact {
        /// The offending, never-presented ids (sorted).
        artifacts: Vec<String>,
        /// How many ids the engine did present (the accepted set size).
        printed: usize,
        /// `(supplied, corrected)` pairs for supplied ids that look
        /// medium-relative: prefixing the binding's medium root yields an id
        /// the slice DID present. The remedy — never an acceptance.
        suggestions: Vec<(String, String)>,
    },
    /// A `worked` disposition (explicit or auto-derived) names an artifact
    /// whose anchor rows on the destination mem still resolve `drifted`:
    /// the sidecar says the entities describe an older version of the
    /// artifact than the one the pass presents, so "worked" would advance
    /// the baseline over a lie. Refused atomically before any write; the
    /// remedy is to re-pin (or rewrite) every named entity's anchor on the
    /// artifact, in the same update call that repairs its claims.
    #[error(
        "{} worked artifact(s) still carry drifted anchor rows on the destination mem: {}; \
         re-pin the named entities' anchors (or rewrite their claims) before disposing the \
         artifact as worked",
        artifacts.len(),
        fmt_drifted(artifacts)
    )]
    AnchorsStillDrifted {
        /// Each offending artifact with the entity ids whose rows drift (sorted).
        artifacts: Vec<(String, Vec<String>)>,
    },
    /// A bare (rationale-less) non-`excluded` disposition names an artifact
    /// the durable authored-exclusion ledger holds. The ledger row carries
    /// the agent's reasoning for keeping the artifact out of the model;
    /// silently dropping it on a blanket `worked` is how the artifact came
    /// back as `uncovered` on the next exhaustive verify with its rationale
    /// gone (measured 2026-09-10 on the engine binding). Refused atomically
    /// before any write; lifting an exclusion is a re-judgement and takes
    /// the reasoned disposition form, or the artifact is left undisposed
    /// and the ledger disposes it.
    #[error(
        "{} disposition(s) would silently lift an authored exclusion: {}; leave the artifact \
         undisposed (the exclusion ledger disposes it) or lift the exclusion explicitly with a \
         reasoned disposition ({{\"disposition\": \"worked\", \"rationale\": \"…\"}})",
        artifacts.len(),
        fmt_excluded(artifacts)
    )]
    ExclusionHeld {
        /// Each offending artifact with the recorded exclusion rationale (sorted).
        artifacts: Vec<(String, String)>,
    },
    /// Reading or writing the durable advance store failed.
    #[error("advance store error: {0}")]
    Store(#[source] StoreError),
    /// The `set_mem_sync_state` baseline write failed on completion.
    #[error("could not advance baseline token: {0}")]
    Engine(String),
}

/// Render `artifact (rationale)` pairs for the held-exclusion refusal.
fn fmt_excluded(items: &[(String, String)]) -> String {
    items
        .iter()
        .map(|(a, r)| format!("{a} (excluded: {r})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render `artifact (entity, entity)` pairs for the drifted-anchors refusal.
fn fmt_drifted(items: &[(String, Vec<String>)]) -> String {
    items
        .iter()
        .map(|(art, ents)| format!("{art} ({})", ents.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Render an id list for an error message: `a, b, c` or `(none)`.
fn fmt_list(names: &[String]) -> String {
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

/// Render the medium-relative-dialect remedy for an unknown-artifact refusal:
/// empty when no correction is derivable, else a `supplied → corrected` list
/// telling the agent the exact ids to retry with.
fn fmt_suggestions(suggestions: &[(String, String)]) -> String {
    if suggestions.is_empty() {
        return String::new();
    }
    let pairs = suggestions
        .iter()
        .map(|(supplied, corrected)| format!("`{supplied}` → `{corrected}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        ". Some supplied ids look medium-relative; the slice presents them workspace-relative — \
         retry with {pairs} (the medium-relative form is never accepted)"
    )
}

/// For each unknown disposition id, derive the corrected workspace-relative id
/// when possible: prefix the id with a primary source's medium root and accept
/// the candidate iff it is in the presented set (`printed`). Purely a remedy
/// computation — it never widens the gate.
fn derive_corrected_ids(
    unknown: &[String],
    resolved: &ResolvedIngest,
    printed: &BTreeSet<String>,
) -> Vec<(String, String)> {
    let medium_roots: Vec<&str> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) if !p.pointer.is_empty() => Some(p.pointer.as_str()),
            _ => None,
        })
        .collect();
    unknown
        .iter()
        .filter_map(|id| {
            medium_roots.iter().find_map(|root| {
                let candidate = format!("{}/{id}", root.trim_end_matches('/'));
                printed
                    .contains(candidate.as_str())
                    .then(|| (id.clone(), candidate))
            })
        })
        .collect()
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

/// The disposition-gated baseline advance.
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
/// `resolved.name` must be the canonical binding id `<mem>/<stem>`, as
/// produced by [`memstead_base::binding_run::resolve_binding_run`]; `dispositions` maps each
/// judged artifact id to an agent-supplied [`DispositionInput`] — a bare verdict
/// or a verdict with an authored rationale (in E2 the agent supplies one for
/// **every** artifact — see the module docs). An `excluded` verdict with a
/// rationale is recorded in the durable authored-exclusion ledger; a presented
/// artifact the ledger holds is disposed `excluded` by its row when the agent
/// names no verdict for it; a bare non-excluded verdict over such an artifact
/// refuses ([`AdvanceError::ExclusionHeld`]), and only the reasoned form lifts
/// the exclusion.
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
        // Remedy, not acceptance: when a supplied id resolves to a presented
        // one once prefixed with its medium root (the medium-relative-dialect
        // mistake agents naturally make), the refusal carries the corrected
        // id — the gate itself never widens.
        let suggestions = derive_corrected_ids(&unknown, resolved, &printed);
        return Err(AdvanceError::UnknownArtifact {
            artifacts: unknown,
            printed: printed.len(),
            suggestions,
        });
    }

    // Gate (atomic): a bare non-`excluded` verdict over an artifact the
    // durable exclusion ledger holds is refused before any write. The ledger
    // row is the agent's recorded judgement; only a reasoned re-judgement
    // may replace it.
    let mut held: Vec<(String, String)> = dispositions
        .iter()
        .filter(|(_, input)| input.verdict() != EXCLUDED_VERDICT && input.rationale().is_none())
        .filter_map(|(artifact, _)| {
            state
                .exclusions
                .get(artifact)
                .map(|rationale| (artifact.clone(), rationale.clone()))
        })
        .collect();
    if !held.is_empty() {
        held.sort();
        return Err(AdvanceError::ExclusionHeld { artifacts: held });
    }

    // Accumulate the new (agent-supplied) dispositions. An `excluded` verdict
    // with a rationale lands in the durable exclusion ledger (survives
    // completion); a reasoned non-excluded verdict lifts a prior exclusion for
    // that artifact (a re-judged artifact must not keep stale "excluded"
    // reasoning, and the re-judgement carries its own).
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
            state.exclusion_sources.remove(artifact);
        }
    }

    // The ledger disposes: a presented artifact the agent left undisposed and
    // the exclusion ledger holds is `excluded` by its standing row. The agent
    // judged it once with a rationale; a later change to the artifact does
    // not reopen that judgement on its own, and a pass never stalls on it.
    for art in printed.iter() {
        if !state.dispositions.contains_key(art.as_str())
            && state.exclusions.contains_key(art.as_str())
        {
            state
                .dispositions
                .insert(art.clone(), EXCLUDED_VERDICT.to_string());
        }
    }

    // Auto-`worked`: mark every frozen-slice artifact that an anchor in
    // the destination mem now references. Reads the anchors sidecar, never a
    // commit diff (D7's rejected mechanism stays rejected); scoped to the
    // frozen slice (`printed`) so an anchored write outside the slice
    // fabricates no entry; skips artifacts already carrying an explicit
    // disposition (the agent's judgement wins).
    // An artifact with mention-steered entities (the claims-in-sight move:
    // entities naming it, or a symbol its change defines or removes, without
    // anchoring it) is never auto-worked: its anchoring entity's write says
    // nothing about the claims the other entities hold, so the pass cannot
    // close on their behalf. The agent disposes it after walking them.
    let steered =
        super::cursor::steered_entities(engine, resolved, workspace_root, &state.frozen_slice);
    let mention_steered = |art: &str| {
        let (base, _) = memstead_base::preparation::split_unit_id(art);
        steered.get(base).is_some_and(|e| !e.mentioned.is_empty())
    };
    let auto_worked: Vec<String> = printed
        .iter()
        .filter(|art| !state.dispositions.contains_key(art.as_str()))
        .filter(|art| !mention_steered(art))
        .filter(|art| {
            // A unit id (`<path>#<key>`, touchpoint B) is disposed by an
            // anchor over exactly that unit; a file id by any anchor
            // referencing the path. A file-level anchor never disposes a
            // unit — reading the file is not reading every unit of it.
            let (base, key) = memstead_base::preparation::split_unit_id(art);
            engine
                .anchors_referencing_artifact(base)
                .iter()
                .any(|(eid, a)| {
                    eid.mem() == resolved.destination_mem.as_str()
                        && (key.is_none() || a.artifact == **art)
                })
        })
        .cloned()
        .collect();
    for art in auto_worked {
        state.dispositions.insert(art, "worked".to_string());
    }

    // Gate two (atomic, before any write): a `worked` artifact whose anchor
    // rows on the destination mem still resolve `drifted` is not worked. The
    // rows say the entities describe the artifact as it was before this
    // slice; advancing over them would record the change as absorbed while
    // the sidecar keeps the old hash, which is exactly how a graph-health
    // lane later reports drift for a change the sync had "absorbed"
    // (measured 2026-09-09 on the flagship binding: two of sixteen rows
    // re-pinned, the baseline advanced, fourteen rows left drifted).
    let drifted = drifted_worked_artifacts(engine, resolved, &state.dispositions, &printed);
    if !drifted.is_empty() {
        return Err(AdvanceError::AnchorsStillDrifted { artifacts: drifted });
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
        // the write's commit note — no new channel.
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
        // If any durable authored exclusions accumulated, on either ledger
        // (artifacts or entities), retain a slimmed store holding only them
        // (empty slice, no transient dispositions) so the fidelity report
        // keeps consulting them; otherwise drop the store entirely
        // (completion idempotent — the no-exclusion path is unchanged).
        if !state.has_durable_exclusions() {
            delete_advance_store(workspace_root, &mem, &name).map_err(AdvanceError::Store)?;
        } else {
            let durable = AdvanceState {
                binding: binding_id.clone(),
                frozen_slice: Slice::default(),
                dispositions: BTreeMap::new(),
                exclusions: state.exclusions.clone(),
                exclusion_sources: state.exclusion_sources.clone(),
                dropped_exclusions: state.dropped_exclusions.clone(),
                entity_exclusions: state.entity_exclusions.clone(),
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

/// The `worked` artifacts among `dispositions` (restricted to the presented
/// slice) that still have at least one anchor row on the destination mem
/// resolving [`memstead_base::anchor::AnchorState::Drifted`], each with the entity ids
/// whose rows drift. One sidecar read and one observation pass per call; a
/// unit id (`<path>#<key>`) matches rows over exactly that unit, a file id any
/// row referencing the path, mirroring the auto-`worked` rule.
fn drifted_worked_artifacts(
    engine: &Engine,
    resolved: &ResolvedIngest,
    dispositions: &BTreeMap<String, String>,
    printed: &BTreeSet<String>,
) -> Vec<(String, Vec<String>)> {
    let worked: Vec<&String> = dispositions
        .iter()
        .filter(|(art, verdict)| verdict.as_str() == "worked" && printed.contains(art.as_str()))
        .map(|(art, _)| art)
        .collect();
    if worked.is_empty() {
        return Vec::new();
    }
    let dest = resolved.destination_mem.as_str();
    let supplied = memstead_base::engine::query::SuppliedObservations::default();
    let states: BTreeMap<(String, String), Option<memstead_base::anchor::AnchorState>> = engine
        .mem_anchors_resolved_with(dest, &supplied)
        .into_iter()
        .map(|(eid, r)| ((eid.to_string(), r.anchor.artifact.clone()), r.state))
        .collect();
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for art in worked {
        let (base, key) = memstead_base::preparation::split_unit_id(art);
        let mut entities: BTreeSet<String> = BTreeSet::new();
        for (eid, a) in engine.anchors_referencing_artifact(base) {
            if eid.mem() != dest || !(key.is_none() || a.artifact == *art) {
                continue;
            }
            if let Some(Some(memstead_base::anchor::AnchorState::Drifted)) =
                states.get(&(eid.to_string(), a.artifact.clone()))
            {
                entities.insert(eid.to_string());
            }
        }
        if !entities.is_empty() {
            out.push((art.clone(), entities.into_iter().collect()));
        }
    }
    out
}

/// The outcome of a [`record_exclusions`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludeOutcome {
    /// The canonical (workspace-relative) id each requested id resolved to,
    /// in request order: what the ledger now holds, so an agent sees the
    /// spelling that took effect when it passed the source-relative form.
    pub recorded: Vec<(String, String)>,
    /// The binding id whose exclusion ledger was written.
    pub binding: String,
    /// Total authored exclusions in the ledger after this call (this call + prior).
    pub excluded: usize,
    /// How many supplied artifacts were newly added (not already in the ledger).
    pub added: usize,
}

/// Why [`record_exclusions`] or [`record_entity_exclusions`] could not
/// complete.
#[derive(Debug, thiserror::Error)]
pub enum ExcludeError {
    /// The binding id is not the canonical `<mem>/<stem>` shape.
    #[error("malformed binding id '{0}': expected `<mem>/<stem>`")]
    MalformedId(String),
    /// One or more ids name no entity of the binding's destination mem (an
    /// entity of another mem, a stub, or nothing at all) — the gate refuses
    /// the whole call, nothing written. An exclusion is a statement about an
    /// entity that exists and deliberately anchors nothing; there is nothing
    /// to state about one that does not.
    #[error(
        "entity exclusion names {} id(s) that are not entities of the destination mem `{mem}`: {}",
        entities.len(),
        fmt_list(entities)
    )]
    NotDestinationEntity {
        /// The offending ids (sorted).
        entities: Vec<String>,
        /// The binding's destination mem.
        mem: String,
    },
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
        /// For each offending id, the nearest known ids of `S(D)` (by
        /// shared path suffix, then name similarity), so the agent can
        /// repair the spelling instead of guessing.
        nearest: BTreeMap<String, Vec<String>>,
    },
    /// The enumeration of `S(D)` is known-incomplete (a malformed or
    /// retired-dialect scope pattern), so membership cannot be decided: the
    /// gate would refuse genuinely in-scope artifacts and state the short
    /// count as if it were the population. Refused whole, nothing written.
    #[error(
        "the binding's source enumeration is incomplete — {reason} — so `S(D)` membership \
         cannot be decided; fix the named scope pattern(s), then re-declare the exclusions"
    )]
    PartialEnumeration {
        /// The facet whose enumeration is partial.
        facet: String,
        /// Why the enumeration is incomplete, naming the offending patterns.
        reason: String,
    },
    /// A source-relative id resolves under MORE THAN ONE of the binding's
    /// primary sources, so recording it would pick a source the caller never
    /// named. Refused whole, nothing written; the canonical ids are the
    /// recovery, and either one is unambiguous.
    #[error(
        "exclusion id {} is ambiguous: it resolves under {} of the binding's sources, as {}; \
         re-declare it with one of those workspace-relative ids",
        fmt_list(&ambiguous.keys().cloned().collect::<Vec<_>>()),
        ambiguous.values().map(|c| c.len()).max().unwrap_or(0),
        fmt_list(&ambiguous.values().flatten().cloned().collect::<Vec<_>>())
    )]
    AmbiguousArtifact {
        /// For each ambiguous requested id, every canonical id it resolves
        /// to, sorted. The caller re-declares with one of them.
        ambiguous: BTreeMap<String, Vec<String>>,
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
    engine: &Engine,
    workspace_root: &Path,
    resolved: &ResolvedIngest,
    exclusions: &BTreeMap<String, String>,
) -> Result<ExcludeOutcome, ExcludeError> {
    let binding_id = resolved.name.clone();
    let (mem, name) =
        split_binding_id(&binding_id).map_err(|_| ExcludeError::MalformedId(binding_id.clone()))?;

    // Enumerate S(D) — the in-scope source-artifact set, the same enumeration the
    // fidelity report uses for its coverage denominator. The REPORTED form: a
    // partial enumeration (a malformed or retired-dialect scope pattern) is not
    // the population, so deciding membership over it would refuse genuinely
    // in-scope artifacts and state the short count as if it were `S(D)` —
    // refuse the call instead, naming the cause.
    let mut s_d: BTreeSet<String> = BTreeSet::new();
    // artifact → the facet it enumerates under (the first, in declaration
    // order): the source half of the exclusion's identity.
    let mut facet_of: BTreeMap<String, String> = BTreeMap::new();
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source {
            let walked = enumerate_source_artifacts_reported(
                engine,
                p,
                &resolved.deny_paths,
                workspace_root,
            );
            if let Some(reason) = walked.partiality_reason() {
                return Err(ExcludeError::PartialEnumeration {
                    facet: p.name.clone(),
                    reason,
                });
            }
            for f in &walked.files {
                facet_of.entry(f.clone()).or_insert_with(|| p.name.clone());
            }
            s_d.extend(walked.files);
        }
    }

    // Gate (atomic): every exclusion id must be an S(D) member. Validate BEFORE
    // any disk write so a refusal leaves the store untouched.
    // Resolve each requested id to the canonical (workspace-relative) form
    // `S(D)` is keyed by: the canonical form itself, or the source-relative
    // form joined onto a primary source's medium base, the way the anchor
    // write gate resolves an artifact path.
    // Stored ids are always canonical, so a ledger written before this
    // resolution keeps working unchanged.
    let bases: Vec<PathBuf> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(medium_base(&p.pointer, workspace_root)),
            ResolvedSource::Reference { .. } => None,
        })
        .collect();
    let mut canonical: BTreeMap<String, String> = BTreeMap::new();
    let mut not_member: Vec<String> = Vec::new();
    let mut ambiguous: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for requested in exclusions.keys() {
        if s_d.contains(requested.as_str()) {
            canonical.insert(requested.clone(), requested.clone());
            continue;
        }
        // Every source that resolves it, not the first (C8). Taking the first
        // meant the source listed earliest in the binding silently won, which
        // records an exclusion against an artifact the caller never named.
        // The cross-source rule itself lives beside the within-source one in
        // `engine::query`; the membership predicate stays here, because what
        // counts as resolved is this surface's business.
        match memstead_base::engine::query::resolve_across_sources(
            bases.iter(),
            requested,
            |base, id| {
                let candidate = relative_path(workspace_root, &normalize_lexical(&base.join(id)))
                    .to_string_lossy()
                    .to_string();
                s_d.contains(candidate.as_str()).then_some(candidate)
            },
        ) {
            memstead_base::engine::query::CrossSourceArtifact::Unique(c) => {
                canonical.insert(requested.clone(), c);
            }
            memstead_base::engine::query::CrossSourceArtifact::Ambiguous(cands) => {
                ambiguous.insert(requested.clone(), cands);
            }
            memstead_base::engine::query::CrossSourceArtifact::Unresolved => {
                not_member.push(requested.clone())
            }
        }
    }
    // Ambiguity before non-membership: an id that resolves several ways is a
    // spelling the caller must narrow, not one they got wrong.
    if !ambiguous.is_empty() {
        return Err(ExcludeError::AmbiguousArtifact { ambiguous });
    }
    if !not_member.is_empty() {
        not_member.sort();
        not_member.dedup();
        let nearest = not_member
            .iter()
            .map(|id| (id.clone(), nearest_known_ids(id, &s_d)))
            .collect();
        return Err(ExcludeError::NotSourceMember {
            artifacts: not_member,
            printed: s_d.len(),
            nearest,
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
    let mut recorded: Vec<(String, String)> = Vec::new();
    for (requested, rationale) in exclusions {
        let artifact = &canonical[requested];
        if state
            .exclusions
            .insert(artifact.clone(), rationale.clone())
            .is_none()
        {
            added += 1;
        }
        if let Some(facet) = facet_of.get(artifact) {
            state
                .exclusion_sources
                .insert(artifact.clone(), facet.clone());
        }
        recorded.push((requested.clone(), artifact.clone()));
    }
    write_advance_store(workspace_root, &mem, &name, &state).map_err(ExcludeError::Store)?;

    Ok(ExcludeOutcome {
        recorded,
        binding: binding_id,
        excluded: state.exclusions.len(),
        added,
    })
}

/// Declare authored **entity exclusions**: destination entities that
/// deliberately carry no anchor, each with its rationale. The gate is
/// existence in the destination mem (a non-stub entity the store holds), and
/// it is atomic: one id that is not such an entity refuses the whole call and
/// writes nothing. Re-declaring merges into the ledger and updates the
/// rationale. The fidelity report's coverage reading consults the ledger:
/// a declared entity is named as excluded with its reason rather than as an
/// entity without anchors.
pub fn record_entity_exclusions(
    engine: &Engine,
    workspace_root: &Path,
    resolved: &ResolvedIngest,
    exclusions: &BTreeMap<String, String>,
) -> Result<ExcludeOutcome, ExcludeError> {
    let binding_id = resolved.name.clone();
    let (mem, name) =
        split_binding_id(&binding_id).map_err(|_| ExcludeError::MalformedId(binding_id.clone()))?;
    let dest = resolved.destination_mem.as_str();

    let mut canonical: BTreeMap<String, String> = BTreeMap::new();
    let mut not_member: Vec<String> = Vec::new();
    for requested in exclusions.keys() {
        let id = memstead_base::EntityId::canonical(requested);
        if id.mem() == dest && !engine.entity_is_absent(&id) {
            canonical.insert(requested.clone(), id.to_string());
        } else {
            not_member.push(requested.clone());
        }
    }
    if !not_member.is_empty() {
        not_member.sort();
        not_member.dedup();
        return Err(ExcludeError::NotDestinationEntity {
            entities: not_member,
            mem: dest.to_string(),
        });
    }

    let mut state = read_advance_store(workspace_root, &mem, &name)
        .map_err(ExcludeError::Store)?
        .unwrap_or_else(|| AdvanceState {
            binding: binding_id.clone(),
            ..Default::default()
        });
    let mut added = 0usize;
    let mut recorded: Vec<(String, String)> = Vec::new();
    for (requested, rationale) in exclusions {
        let entity = &canonical[requested];
        if state
            .entity_exclusions
            .insert(entity.clone(), rationale.clone())
            .is_none()
        {
            added += 1;
        }
        recorded.push((requested.clone(), entity.clone()));
    }
    write_advance_store(workspace_root, &mem, &name, &state).map_err(ExcludeError::Store)?;

    Ok(ExcludeOutcome {
        recorded,
        binding: binding_id,
        excluded: state.entity_exclusions.len(),
        added,
    })
}

/// The known ids of `S(D)` nearest to an unknown one: those sharing its
/// file name first, then those sharing its last path components, at most
/// five, sorted. A repair hint, never a match.
fn nearest_known_ids(unknown: &str, known: &BTreeSet<String>) -> Vec<String> {
    let name = unknown.rsplit('/').next().unwrap_or(unknown);
    let tail: Vec<&str> = unknown.rsplit('/').take(2).collect();
    let mut scored: Vec<(usize, &String)> = known
        .iter()
        .filter_map(|k| {
            let kname = k.rsplit('/').next().unwrap_or(k);
            let ktail: Vec<&str> = k.rsplit('/').take(2).collect();
            let same_dir = tail.len() > 1 && ktail.get(1) == tail.get(1);
            let score = if kname == name && same_dir {
                3
            } else if kname == name {
                2
            } else if same_dir || k.contains(name) || name.contains(kname) {
                1
            } else {
                0
            };
            (score > 0).then_some((score, k))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored.into_iter().take(5).map(|(_, k)| k.clone()).collect()
}

#[cfg(test)]
mod tests;
