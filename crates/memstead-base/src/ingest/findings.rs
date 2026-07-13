//! The engine-owned durable **findings store** and the thin `projection verify`
//! write path that populates it (bundle plan `05-verify-sync-engine`, group A).
//!
//! Verify **measures** fidelity and records durable findings; it never mutates
//! the destination mem. The store is the real home behind plan 03's findings
//! schema stub ([`crate::binding`]'s removed `FindingKey` / `FindingRecord`):
//! findings are keyed `(hash(D), source_head)` so a binding-declaration edit or
//! a source-head move **mechanically** partitions them into a fresh keyspace —
//! prior findings are never presented as current, only segregated as superseded
//! (A3).
//!
//! ## Durability & location (A1, engine-state convention)
//!
//! The store is engine-owned state, **not a mem**. It lives at
//! `<workspace>/.memstead/state/findings/<mem>/<name>.json` — a sibling of the
//! durable advance store (`state/advance/`) and `state/mounts.json`, under the
//! `.memstead/state/` tier every engine-state consumer shares. It is read fresh
//! from disk per call, so findings survive a process restart and a later
//! sync-brief render (a fresh process) reads them back. This is deliberately the
//! `state/` tier, **not** the ephemeral `.memstead.cache/` tier the mtime memo,
//! backoff, and the `next_batch` rotation use — those are recomputable; findings
//! are not.
//!
//! ## One writer (A4/A5)
//!
//! Only the engine verify/sync/advance code paths write this store. There is no
//! CLI/skill/temp-file side channel: the refinement scout/writer temp-findings
//! handover (a `.md` file under `.memstead.cache/ingest/refinement/` with a
//! 10-minute-staleness contract) is gone — [`super::refinement`] retains only
//! the `next_batch` rotation machinery, consumed here solely to **schedule**
//! verify samples. [`verify_binding`] takes `&Engine` (shared, not mutable): it
//! is structurally incapable of a destination-mem mutation. Any repair routes
//! through the sync brief (group C), never through findings recording/reading.
//! The one sanctioned post-run write is the **verified baseline**: after a
//! pass returns `Ok`, the caller records `<binding>/<facet>#verified` per
//! observed facet head via [`record_verified_baseline`] (the lifecycle
//! sync-state writer) — a separate, explicit step, so an aborted or failed
//! run never advances the token.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Engine;
use crate::anchor::{Anchor, AnchorState};
use crate::binding::{
    BindingV1, DEFAULT_ADJUDICATION_CAP, DEFAULT_FULL_RESYNC_EVERY, ResolvedBinding, hash_binding,
    medium_capabilities,
};
use crate::workspace_store::{StoreError, WORKSPACE_STORE_DIR};

use super::advance::is_single_component;
use super::cursor::{compute_source_cursor, enumerate_facet_files};
use super::refinement::{
    ROTATION_ANCHOR_ADJUDICATION, bump_verify_runs, next_batch, next_rotation_batch,
};
use super::resolve::{ResolvedIngest, ResolvedSource};

/// The engine-owned state directory root, under the workspace store:
/// `<root>/.memstead/state/`. Mirrors [`super::advance`]'s `STATE_DIR`.
const STATE_DIR: &str = "state";
/// The findings store's subtree: `<root>/.memstead/state/findings/`.
const FINDINGS_DIR: &str = "findings";

// ---------------------------------------------------------------------------
// Key
// ---------------------------------------------------------------------------

/// The key a batch of findings is recorded under: a binding's `hash(D)` plus
/// the `source_head` the findings were observed at. A changed `hash(D)` (a
/// binding-declaration edit) or a moved `source_head` (the source advanced)
/// yields a different key, so prior findings are invalidated by construction —
/// segregated as superseded, never silently mixed into the current view (A3).
///
/// The real key behind plan 03's schema stub (which lived, IO-less, in
/// [`crate::binding`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FindingKey {
    /// The binding's `hash(D)` (lowercase hex SHA-256; see
    /// [`crate::binding::hash_binding`]).
    pub binding_hash: String,
    /// The composite source-head token the findings were observed at — the
    /// current per-facet baseline tokens, so it moves iff any source moves.
    pub source_head: String,
}

// ---------------------------------------------------------------------------
// Finding
// ---------------------------------------------------------------------------

/// The class of a verify finding (A2). A closed vocabulary: `drifted` and
/// `queued-for-adjudication` come only from **hash-drift adjudication** (over
/// hash-bearing anchors — never `authored` / `informed-by`, see
/// [`adjudicate_anchor`]); `unresolvable-anchor` is an existence failure;
/// `uncovered` marks a source artifact with no anchor; `wrong` is reserved for
/// an adjudicated content mismatch the group-B report renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingClass {
    /// A hash-bearing anchor's prepared-content hash drifted from the recorded
    /// one on a `stable` medium.
    Drifted,
    /// An adjudicated content mismatch (reserved for the group-B report path).
    Wrong,
    /// A source artifact in scope carries no anchor in the destination mem.
    Uncovered,
    /// An anchor's referenced artifact is no longer present in the medium.
    UnresolvableAnchor,
    /// Hash adjudication is deferred (capped, or `recheck`) and queued in the
    /// store; the remainder is the tier-3 backlog.
    QueuedForAdjudication,
}

impl FindingClass {
    /// Every wire string, in declaration order.
    pub const WIRE_VALUES: &'static [&'static str] = &[
        "drifted",
        "wrong",
        "uncovered",
        "unresolvable-anchor",
        "queued-for-adjudication",
    ];

    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            FindingClass::Drifted => "drifted",
            FindingClass::Wrong => "wrong",
            FindingClass::Uncovered => "uncovered",
            FindingClass::UnresolvableAnchor => "unresolvable-anchor",
            FindingClass::QueuedForAdjudication => "queued-for-adjudication",
        }
    }

    /// Inverse of [`Self::as_wire`]; `None` for an unknown string.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "drifted" => Some(FindingClass::Drifted),
            "wrong" => Some(FindingClass::Wrong),
            "uncovered" => Some(FindingClass::Uncovered),
            "unresolvable-anchor" => Some(FindingClass::UnresolvableAnchor),
            "queued-for-adjudication" => Some(FindingClass::QueuedForAdjudication),
            _ => None,
        }
    }
}

/// What a finding is about (A2): an anchor reference, or — for an uncovered
/// artifact that has no anchor — the source artifact id itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FindingTarget {
    /// An anchor reference: the entity id carrying the anchor and the artifact
    /// the anchor points at.
    Anchor {
        /// The entity id (`mem--slug`) the anchor belongs to.
        entity: String,
        /// The anchor's artifact reference (path / `path@commit` / url / entity id).
        artifact: String,
    },
    /// An uncovered source artifact — no anchor references it, so there is no
    /// anchor to name (A2's "artifact ID for uncovered artifacts").
    Artifact {
        /// The source-side artifact id.
        artifact: String,
    },
}

/// A single durable verify finding (A2). Carries its target, its class, and —
/// self-describingly — the [`FindingKey`] it was recorded under, so a finding
/// pulled out of the store always states which `(hash(D), source_head)` it
/// belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// The key this finding was recorded under (A2). Redundant with its
    /// enclosing [`FindingsBatch::key`], carried on the finding so it stays
    /// self-describing when detached.
    pub key: FindingKey,
    /// The source facet the finding concerns (best-effort label in the thin
    /// verify — the group-B report refines per-facet attribution).
    pub facet: String,
    /// What the finding is about.
    pub target: FindingTarget,
    /// The finding class.
    pub class: FindingClass,
    /// Human/agent-readable detail.
    pub detail: String,
    /// When the finding was recorded (opaque timestamp string — unix seconds).
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// One batch of findings recorded under a single [`FindingKey`] in one verify
/// pass. A new pass under the same key replaces the batch; a pass under a
/// different key (changed `hash(D)` or moved `source_head`) lands as a separate
/// batch — the prior one is retained, segregated, never overwritten (A3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingsBatch {
    /// The key this batch was recorded under.
    pub key: FindingKey,
    /// When the batch was last recorded (opaque timestamp string).
    pub recorded_at: String,
    /// The findings in this batch.
    pub findings: Vec<Finding>,
}

/// One binding's durable findings store (A1). Persisted at
/// `.memstead/state/findings/<mem>/<name>.json`, read fresh per call. Holds
/// findings grouped by the key they were recorded under so invalidation is
/// mechanical: [`Self::current`] presents only the batch under the current key;
/// [`Self::superseded`] surfaces everything under prior keys, segregated (A3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingsStore {
    /// The canonical binding id `<mem>/<stem>` this store belongs to.
    pub binding: String,
    /// Findings grouped by recording key, most-recent recording order not
    /// guaranteed — look up by key.
    #[serde(default)]
    pub batches: Vec<FindingsBatch>,
}

impl FindingsStore {
    /// Record `findings` under `key`, replacing any prior batch recorded under
    /// the **exact** same key and leaving every other key's batch untouched
    /// (A3 segregation — a changed key never overwrites the old batch).
    pub fn record(&mut self, key: FindingKey, recorded_at: String, findings: Vec<Finding>) {
        if let Some(batch) = self.batches.iter_mut().find(|b| b.key == key) {
            batch.recorded_at = recorded_at;
            batch.findings = findings;
        } else {
            self.batches.push(FindingsBatch {
                key,
                recorded_at,
                findings,
            });
        }
    }

    /// The findings recorded under `key` — the **only** findings ever presented
    /// as current (A3). Empty when nothing was recorded under this exact key.
    pub fn current(&self, key: &FindingKey) -> &[Finding] {
        self.batches
            .iter()
            .find(|b| &b.key == key)
            .map(|b| b.findings.as_slice())
            .unwrap_or(&[])
    }

    /// Every finding recorded under a key **other** than `key` — superseded by
    /// a `hash(D)` change or a source-head move, segregated so a consumer can
    /// show them as stale without mixing them into the current view (A3).
    pub fn superseded(&self, key: &FindingKey) -> Vec<&Finding> {
        self.batches
            .iter()
            .filter(|b| &b.key != key)
            .flat_map(|b| b.findings.iter())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Store IO — mirrors `super::advance`'s durable-store shape
// ---------------------------------------------------------------------------

/// The durable store path for a binding:
/// `.memstead/state/findings/<mem>/<name>.json`.
pub fn findings_store_path(workspace_root: &Path, mem: &str, name: &str) -> PathBuf {
    workspace_root
        .join(WORKSPACE_STORE_DIR)
        .join(STATE_DIR)
        .join(FINDINGS_DIR)
        .join(mem)
        .join(format!("{name}.json"))
}

/// Read the durable findings store for a binding, or `None` when none exists.
/// A malformed file surfaces a typed [`StoreError::Parse`] naming the path.
pub fn read_findings_store(
    workspace_root: &Path,
    mem: &str,
    name: &str,
) -> Result<Option<FindingsStore>, StoreError> {
    let path = findings_store_path(workspace_root, mem, name);
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

/// Create an engine-owned store subtree and drop a self-ignoring
/// `.gitignore` (`*`) at its root if none exists. The `state/findings/`
/// and `state/advance/` stores are per-checkout ephemeral engine state
/// living inside a possibly-tracked workspace (where `state/mounts.json`
/// IS tracked) — without the ignore they surface as untracked noise and
/// would churn if committed. Best-effort: an ignore-write failure never
/// fails the store write itself.
pub(crate) fn ensure_selfignoring_store_dir(subtree_root: &Path) -> Result<(), StoreError> {
    std::fs::create_dir_all(subtree_root).map_err(|e| StoreError::Io {
        path: subtree_root.to_path_buf(),
        source: e,
    })?;
    let gitignore = subtree_root.join(".gitignore");
    if !gitignore.exists() {
        let _ = std::fs::write(&gitignore, "*\n");
    }
    Ok(())
}

/// Persist the durable findings store for a binding (pretty JSON), creating
/// parent directories.
pub fn write_findings_store(
    workspace_root: &Path,
    mem: &str,
    name: &str,
    store: &FindingsStore,
) -> Result<(), StoreError> {
    ensure_selfignoring_store_dir(
        &workspace_root
            .join(WORKSPACE_STORE_DIR)
            .join(STATE_DIR)
            .join(FINDINGS_DIR),
    )?;
    let path = findings_store_path(workspace_root, mem, name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(store).map_err(|e| StoreError::Parse {
        path: path.clone(),
        message: e.to_string(),
    })?;
    std::fs::write(&path, bytes).map_err(|e| StoreError::Io { path, source: e })
}

/// Drop the durable findings store for a binding. A missing file is a
/// successful no-op.
pub fn delete_findings_store(
    workspace_root: &Path,
    mem: &str,
    name: &str,
) -> Result<(), StoreError> {
    let path = findings_store_path(workspace_root, mem, name);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::Io { path, source: e }),
    }
}

// ---------------------------------------------------------------------------
// Verify write path
// ---------------------------------------------------------------------------

/// Why [`verify_binding`] could not complete.
#[derive(Debug, thiserror::Error)]
pub enum FindingsError {
    /// The binding id is not the canonical `<mem>/<stem>` shape.
    #[error("malformed binding id '{0}': expected `<mem>/<stem>`")]
    MalformedId(String),
    /// Reading or writing the durable findings store failed.
    #[error("findings store error: {0}")]
    Store(#[source] StoreError),
    /// A path-based primary source's base directory does not exist — a
    /// vanished or unmounted source. Verify refuses rather than measures:
    /// enumerating a missing tree yields an empty stat map whose aggregate
    /// (the hash of nothing) is indistinguishable from a genuinely empty
    /// source and would overwrite a real `#verified` baseline with fake
    /// state. Typed and visible, mirroring the D3 non-enumerable refusal.
    #[error("source unreachable for facet '{facet}' (medium '{medium}'): `{path}` does not exist")]
    SourceUnreachable {
        /// The facet whose medium resolved to the missing path.
        facet: String,
        /// The medium's name.
        medium: String,
        /// The resolved base path that does not exist.
        path: String,
    },
}

/// The outcome of a [`verify_binding`] pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyOutcome {
    /// The binding id verified.
    pub binding: String,
    /// The key the findings were recorded under this pass.
    pub key: FindingKey,
    /// How many findings were recorded under the current key.
    pub recorded: usize,
    /// How many findings remain under prior (superseded) keys (A3).
    pub superseded: usize,
    /// The tier-3 backlog depth — findings queued for adjudication.
    pub backlog: usize,
    /// The full-enumeration scheduling decision for this run (D3) — whether a
    /// scheduled full walk fired, is not yet due, is disabled, and any typed
    /// non-enumerable refusals. Surfaced (never a silent skip) to the caller.
    pub full_resync: FullResyncDecision,
    /// Each source facet's current head token as observed by this run — the
    /// per-facet decomposition of `key.source_head`. The completed-run
    /// baseline [`record_verified_baseline`] writes as `#verified`.
    pub facet_heads: BTreeMap<String, String>,
}

/// Record a **completed** verify run's baseline: for each facet head the run
/// observed, `<binding>/<facet>#verified = <token>` on the destination mem,
/// through the engine's lifecycle sync-state writer (the backlog-prescribed
/// `#verified` writer — the counterpart of the advance path's `#synced`).
///
/// Deliberately a separate step from [`verify_binding`], which keeps its
/// shared `&Engine` borrow (A5 — measurement is structurally incapable of a
/// mem mutation): the caller invokes this **only after** a verify pass
/// returned `Ok`, so an aborted or failed run never advances the token. The
/// selection loop reads the token to decide when a verify is due again; the
/// CLI `status`/report paths and the macOS panel render it.
///
/// Returns the written sync-state keys. A binding whose run observed no facet
/// head (nothing recorded, nothing moved) writes nothing.
pub fn record_verified_baseline(
    engine: &mut Engine,
    destination_mem: &str,
    outcome: &VerifyOutcome,
    note: Option<&str>,
) -> Result<Vec<String>, crate::engine::EngineError> {
    let mut written = Vec::with_capacity(outcome.facet_heads.len());
    for (facet, token) in &outcome.facet_heads {
        let key = format!("{}/{facet}#verified", outcome.binding);
        engine.set_mem_sync_state(destination_mem, &key, token, note)?;
        written.push(key);
    }
    Ok(written)
}

/// Split a canonical binding id `<mem>/<stem>` into its two path-safe halves,
/// or refuse. Uses the same guard as the advance store so a caller-supplied id
/// can never escape the `.memstead/state/findings/` tier.
fn split_binding_id(binding_id: &str) -> Result<(String, String), FindingsError> {
    binding_id
        .split_once('/')
        .filter(|(m, n)| is_single_component(m) && is_single_component(n))
        .map(|(m, n)| (m.to_string(), n.to_string()))
        .ok_or_else(|| FindingsError::MalformedId(binding_id.to_string()))
}

/// A single facet label for the thin verify: the lone primary facet when there
/// is exactly one, else a comma-join. Per-anchor facet attribution is a
/// group-B refinement.
fn source_facet_label(resolved: &ResolvedIngest) -> String {
    let facets: Vec<&str> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(p.facet_ref.as_str()),
            ResolvedSource::Reference { .. } => None,
        })
        .collect();
    facets.join(",")
}

/// Opaque recording timestamp — unix seconds as a decimal string.
fn now_seconds() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

/// Each source facet's **current head token**, keyed by facet. Starts from the
/// destination mem's recorded `#synced` tokens for the binding, then overlays
/// the cursor's current-head tokens for any facet that has moved or is newly
/// seen — so the map reflects the source's current state. These are the tokens
/// [`current_source_head`] joins into the composite key, and the per-facet
/// values [`record_verified_baseline`] writes as `#verified` after a completed
/// verify run.
fn current_facet_heads(
    engine: &Engine,
    workspace_root: &Path,
    resolved: &ResolvedIngest,
) -> BTreeMap<String, String> {
    let binding_id = &resolved.name;
    let prefix = format!("{binding_id}/");
    let mut tokens: BTreeMap<String, String> = BTreeMap::new();

    // Recorded baselines for facets that have not moved since the last sync.
    if let Some(cfg) = engine.mem_config_for(&resolved.destination_mem) {
        for (k, v) in &cfg.sync_state {
            if let Some(rest) = k.strip_prefix(&prefix)
                && let Some(facet) = rest.strip_suffix("#synced")
            {
                tokens.insert(facet.to_string(), v.clone());
            }
        }
    }

    // Current-head tokens for facets that moved / reseeded this pass win.
    let cursor = compute_source_cursor(engine, resolved, workspace_root);
    for c in cursor.write_commands.iter().chain(cursor.reseed.iter()) {
        if let Some(rest) = c.key.strip_prefix(&prefix)
            && let Some(facet) = rest.strip_suffix("#synced")
        {
            tokens.insert(facet.to_string(), c.token.clone());
        }
    }

    tokens
}

/// Join a facet-head map into the composite source-head token,
/// deterministically (`facet=token;facet=token`).
fn join_facet_heads(tokens: &BTreeMap<String, String>) -> String {
    tokens
        .iter()
        .map(|(facet, token)| format!("{facet}={token}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// The composite current source-head token: each source facet's current
/// baseline token, joined deterministically — the value changes iff any
/// facet's head changes (the A3 "source head moved" trigger).
fn current_source_head(
    engine: &Engine,
    workspace_root: &Path,
    resolved: &ResolvedIngest,
) -> String {
    join_facet_heads(&current_facet_heads(engine, workspace_root, resolved))
}

/// `hash(D)` for a binding joined to its resolved primary sources.
fn binding_hash_of(binding: &BindingV1, resolved: &ResolvedIngest) -> String {
    let primary_sources = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(p.clone()),
            ResolvedSource::Reference { .. } => None,
        })
        .collect();
    let rb = ResolvedBinding {
        binding: binding.clone(),
        primary_sources,
    };
    hash_binding(&rb)
}

/// The current recording key for a binding: `(hash(D), source_head)`.
fn current_key(
    engine: &Engine,
    workspace_root: &Path,
    binding: &BindingV1,
    resolved: &ResolvedIngest,
) -> FindingKey {
    FindingKey {
        binding_hash: binding_hash_of(binding, resolved),
        source_head: current_source_head(engine, workspace_root, resolved),
    }
}

/// The current `(hash(D), source_head)` key plus the open findings recorded
/// under it for a binding — the read the **sync brief** (group C) consumes. It
/// resolves the current key exactly as [`verify_binding`] does, reads the
/// durable store, and returns the `current(key)` slice cloned. **Read-only** on
/// the destination mem (shared `&Engine`): no findings recording, no mutation.
/// A binding whose store does not exist yet yields the key and an empty vec.
pub fn current_findings(
    engine: &Engine,
    workspace_root: &Path,
    binding: &BindingV1,
    resolved: &ResolvedIngest,
) -> Result<(FindingKey, Vec<Finding>), FindingsError> {
    let (mem, name) = split_binding_id(&resolved.name)?;
    let key = current_key(engine, workspace_root, binding, resolved);
    let findings = read_findings_store(workspace_root, &mem, &name)
        .map_err(FindingsError::Store)?
        .map(|s| s.current(&key).to_vec())
        .unwrap_or_default();
    Ok((key, findings))
}

/// Adjudicate one resolved anchor into a finding, or `None` when it resolves
/// clean.
///
/// **A2 enforcement — hash-drift exclusion.** A `drifted` / `recheck` state is
/// turned into a finding **only** for a hash-bearing class (`anchored` /
/// `derived`). An `authored` or `informed-by` anchor is excluded from hash-drift
/// adjudication by design: it never yields a `drifted` / `queued-for-adjudication`
/// finding here, whatever its content did. (Existence failures — `orphaned` —
/// are class-independent and reported for any class: a vanished artifact is not
/// a hash-drift claim.)
pub fn adjudicate_anchor(
    key: &FindingKey,
    facet: &str,
    entity: &str,
    anchor: &Anchor,
    state: AnchorState,
    created_at: &str,
) -> Option<Finding> {
    let (class, detail) = match state {
        AnchorState::Resolves => return None,
        AnchorState::Orphaned => (
            FindingClass::UnresolvableAnchor,
            format!(
                "artifact '{}' the anchor references is no longer present in the medium",
                anchor.artifact
            ),
        ),
        AnchorState::Drifted | AnchorState::Recheck => {
            // Hash-drift adjudication — excluded for non-hash-bearing classes (A2).
            if !anchor.class.is_hash_bearing() {
                return None;
            }
            match state {
                AnchorState::Drifted => (
                    FindingClass::Drifted,
                    format!(
                        "prepared-content hash of '{}' drifted from the anchored hash",
                        anchor.artifact
                    ),
                ),
                _ => (
                    FindingClass::QueuedForAdjudication,
                    format!(
                        "hash adjudication of '{}' deferred (recheck); queued",
                        anchor.artifact
                    ),
                ),
            }
        }
    };
    Some(Finding {
        key: key.clone(),
        facet: facet.to_string(),
        target: FindingTarget::Anchor {
            entity: entity.to_string(),
            artifact: anchor.artifact.clone(),
        },
        class,
        detail,
        created_at: created_at.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Tier-3 caps + scheduling (group D)
// ---------------------------------------------------------------------------

/// One source facet's enumerability — the input the full-resync scheduler
/// reasons over (D3). Built from the capability matrix per primary facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetEnumerability {
    /// The source facet.
    pub facet: String,
    /// The medium type wire string.
    pub medium_type: String,
    /// Whether the medium's scope is enumerable (`S(D)` computable).
    pub enumerable: bool,
}

/// A typed refusal from the scheduled full-enumeration walk (D3): a source facet
/// whose medium the capability matrix marks **non-enumerable**, which the walk
/// cannot cover. Emitted instead of a silent skip or a fabricated full-coverage
/// claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullResyncRefusal {
    /// The refused facet.
    pub facet: String,
    /// The non-enumerable medium type.
    pub medium_type: String,
    /// Why the scheduled walk refuses this facet.
    pub reason: String,
}

/// The full-enumeration scheduling decision for a verify run (D3). A closed,
/// serialized vocabulary so the caller (and the fidelity report) can render the
/// outcome without inferring it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum FullResyncDecision {
    /// `full_resync_every == 0` — scheduled full walks are disabled; the run
    /// uses the rotating sample only.
    Disabled,
    /// Scheduled but not due this run — the rotating sample runs; the counter
    /// advances toward the next full walk.
    NotDue {
        /// This run's 1-based verify-run count.
        run_count: u64,
        /// The configured cadence.
        every: u32,
        /// How many further runs until the next scheduled full walk.
        runs_until_due: u32,
    },
    /// Due this run: a full-enumeration walk fires for the **enumerable** facets
    /// (guaranteeing a complete coverage picture), and every **non-enumerable**
    /// facet is refused with a typed signal — never a silent skip, never a
    /// fabricated full-coverage claim.
    Due {
        /// This run's 1-based verify-run count.
        run_count: u64,
        /// The configured cadence.
        every: u32,
        /// The facets a full enumeration walk covers this run.
        walked_facets: Vec<String>,
        /// The non-enumerable facets the walk refuses (typed).
        refused: Vec<FullResyncRefusal>,
    },
}

impl FullResyncDecision {
    /// Whether this run performs a full-enumeration walk (a scheduled sweep is
    /// due). `false` for `Disabled` / `NotDue`.
    pub fn is_full_walk(&self) -> bool {
        matches!(self, FullResyncDecision::Due { .. })
    }
}

/// Decide the `full_resync_every` scheduling outcome for a verify run (D3) —
/// pure and level-triggered on the persisted run counter. `every == 0` disables
/// scheduled walks; otherwise the walk is **due** when `run_count` is a multiple
/// of `every`. When due, enumerable facets are walked and non-enumerable facets
/// are refused with a typed [`FullResyncRefusal`] (never silently skipped).
pub fn schedule_full_resync(
    every: u32,
    run_count: u64,
    facets: &[FacetEnumerability],
) -> FullResyncDecision {
    if every == 0 {
        return FullResyncDecision::Disabled;
    }
    let modulo = run_count % u64::from(every);
    if modulo != 0 {
        return FullResyncDecision::NotDue {
            run_count,
            every,
            runs_until_due: (u64::from(every) - modulo) as u32,
        };
    }
    let mut walked_facets = Vec::new();
    let mut refused = Vec::new();
    for f in facets {
        if f.enumerable {
            walked_facets.push(f.facet.clone());
        } else {
            refused.push(FullResyncRefusal {
                facet: f.facet.clone(),
                medium_type: f.medium_type.clone(),
                reason: format!(
                    "medium type '{}' is non-enumerable — a full-enumeration walk cannot cover \
                     it; the scheduled full resync refuses rather than claim full coverage",
                    f.medium_type
                ),
            });
        }
    }
    FullResyncDecision::Due {
        run_count,
        every,
        walked_facets,
        refused,
    }
}

/// The rotation item key a drift-adjudication candidate is selected under (D2) —
/// stable across runs for a given `(entity, artifact)` so the rotating window
/// covers a reproducible sequence.
fn candidate_key(entity: &str, anchor: &Anchor) -> String {
    format!("{entity}\u{1f}{}", anchor.artifact)
}

/// Adjudicate the hash-drift **candidates** under the per-run cap (D1). Each
/// candidate is an anchor observation that hash-drift adjudication applies to
/// (a hash-bearing anchor in a `drifted` / `recheck` state). `window` is the
/// rotation-selected key set this run adjudicates (D2); a candidate whose
/// [`candidate_key`] is **not** in the window is **queued** as
/// `queued-for-adjudication` (the tier-3 backlog remainder) rather than
/// adjudicated. `window = None` means uncapped — every candidate is adjudicated.
///
/// Existence failures (`orphaned`) are **not** candidates: they are cheap
/// existence checks, always reported by [`verify_binding`] regardless of the
/// cap. Non-hash-bearing classes never reach here (they produce no adjudication).
fn adjudicate_candidates(
    key: &FindingKey,
    facet: &str,
    candidates: &[(String, Anchor, AnchorState)],
    window: Option<&BTreeSet<String>>,
    created_at: &str,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for (entity, anchor, state) in candidates {
        let ck = candidate_key(entity, anchor);
        let adjudicate_now = window.is_none_or(|w| w.contains(&ck));
        if adjudicate_now {
            if let Some(f) = adjudicate_anchor(key, facet, entity, anchor, *state, created_at) {
                out.push(f);
            }
        } else {
            // Beyond the per-run cap: queue the remainder (D1) — it re-presents
            // in a later run's rotation window (D2), so the whole candidate set
            // is covered over a full rotation.
            out.push(Finding {
                key: key.clone(),
                facet: facet.to_string(),
                target: FindingTarget::Anchor {
                    entity: entity.clone(),
                    artifact: anchor.artifact.clone(),
                },
                class: FindingClass::QueuedForAdjudication,
                detail: format!(
                    "adjudication of '{}' deferred (per-run adjudication cap reached); queued",
                    anchor.artifact
                ),
                created_at: created_at.to_string(),
            });
        }
    }
    out
}

/// The thin `projection verify` write path (group A). Measures a binding's
/// fidelity and records durable findings under the current `(hash(D),
/// source_head)` key; **read-only on the destination mem** — the `&Engine`
/// (shared, not `&mut`) makes a mem mutation structurally impossible (A5).
///
/// It does two things a real verify does, enough to populate and exercise the
/// store (A1/A2): it adjudicates the destination mem's anchors against their
/// live source observation (via [`adjudicate_anchor`], honouring the A2
/// hash-drift exclusion), and it samples in-scope source artifacts through the
/// retained [`next_batch`] rotation (A4 — the rotation's sole surviving
/// consumer, used only to schedule which artifacts a pass looks at) to surface
/// uncovered ones. The full tier-1 fidelity report and the sync brief are
/// group B/C — this path deliberately renders neither.
pub fn verify_binding(
    engine: &Engine,
    workspace_root: &Path,
    binding: &BindingV1,
    resolved: &ResolvedIngest,
) -> Result<VerifyOutcome, FindingsError> {
    let binding_id = resolved.name.clone();
    let (mem, name) = split_binding_id(&binding_id)?;

    // Refuse a vanished or unmounted path-based source before observing
    // anything: a missing tree would otherwise degrade to an empty
    // enumeration whose head token (the digest of nothing) masquerades as
    // a real observation — and the caller's completed-run baseline write
    // would clobber a genuine `#verified` token with it.
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source
            && matches!(
                p.medium_type,
                crate::pipeline::MediumType::Codebase
                    | crate::pipeline::MediumType::Filesystem
                    | crate::pipeline::MediumType::Git
            )
        {
            let base = super::resolve::source_base_path(p, workspace_root);
            if !base.exists() {
                return Err(FindingsError::SourceUnreachable {
                    facet: p.facet_ref.clone(),
                    medium: p.medium.clone(),
                    path: base.display().to_string(),
                });
            }
        }
    }

    // The facet-head map is the key's per-facet decomposition: computed once,
    // joined into `key.source_head`, and returned on the outcome so a
    // completed run's baseline write records exactly what this run observed.
    let facet_heads = current_facet_heads(engine, workspace_root, resolved);
    let key = FindingKey {
        binding_hash: binding_hash_of(binding, resolved),
        source_head: join_facet_heads(&facet_heads),
    };
    let now = now_seconds();
    let facet = source_facet_label(resolved);
    let cache_root = workspace_root.join(".memstead.cache").join("ingest");

    // Tier-3 operations knobs (group D): the per-run adjudication cap (D1), the
    // scheduled full-walk cadence (D3), and the sample window size. All come off
    // the `verify` block, defaulting to the dogfood-tuned engine defaults when it
    // is absent (verify is read-only — an absent block is defaults, never a
    // refusal).
    let verify_op = binding.operations.verify.as_ref();
    let cap = verify_op.map_or(DEFAULT_ADJUDICATION_CAP, |v| v.adjudication_cap);
    let full_resync_every = verify_op.map_or(DEFAULT_FULL_RESYNC_EVERY, |v| v.full_resync_every);
    let sample_batch = verify_op
        .map_or(resolved.batch_size, |v| v.batch_size)
        .max(1) as usize;

    // Level-trigger clock + full-resync schedule (D3) — the counter ticks every
    // run (even a non-enumerable one) so the schedule can refuse on time.
    let run_count = bump_verify_runs(&cache_root, &binding_id);
    let facet_enum: Vec<FacetEnumerability> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(FacetEnumerability {
                facet: p.facet_ref.clone(),
                medium_type: medium_type_wire(p.medium_type),
                enumerable: medium_capabilities(p.medium_type).enumerable,
            }),
            ResolvedSource::Reference { .. } => None,
        })
        .collect();
    let full_resync = schedule_full_resync(full_resync_every, run_count, &facet_enum);

    let mut findings: Vec<Finding> = Vec::new();

    // 1. Adjudicate the destination mem's anchors against the live source, under
    //    the per-run cap (D1) with a rotating window (D2). Existence failures
    //    (orphaned) are cheap and always reported; hash-drift candidates are
    //    bounded — the cap-sized rotation window is adjudicated, the remainder
    //    queued, and successive runs rotate the window so the whole anchor set is
    //    covered over a full rotation.
    let mut existence: Vec<(String, Anchor, AnchorState)> = Vec::new();
    let mut candidates: Vec<(String, Anchor, AnchorState)> = Vec::new();
    for (eid, resolved_anchor) in engine.mem_anchors_resolved(&resolved.destination_mem) {
        let Some(state) = resolved_anchor.state else {
            continue;
        };
        let anchor = resolved_anchor.anchor;
        match state {
            AnchorState::Resolves => {}
            AnchorState::Orphaned => existence.push((eid.as_ref().to_string(), anchor, state)),
            AnchorState::Drifted | AnchorState::Recheck => {
                // Only hash-bearing anchors are hash-drift candidates (A2); a
                // non-hash-bearing class yields no adjudication.
                if anchor.class.is_hash_bearing() {
                    candidates.push((eid.as_ref().to_string(), anchor, state));
                }
            }
        }
    }
    for (entity, anchor, state) in &existence {
        if let Some(f) = adjudicate_anchor(&key, &facet, entity, anchor, *state, &now) {
            findings.push(f);
        }
    }
    // `cap == 0` disables the cap (adjudicate every candidate); otherwise a
    // cap-sized rotation window selects this run's adjudicated set (D1/D2).
    let window: Option<BTreeSet<String>> = if cap == 0 {
        None
    } else {
        let mut keys: Vec<String> = candidates
            .iter()
            .map(|(e, a, _)| candidate_key(e, a))
            .collect();
        keys.sort();
        keys.dedup();
        next_rotation_batch(
            &cache_root,
            &binding_id,
            ROTATION_ANCHOR_ADJUDICATION,
            keys,
            cap as usize,
        )
        .map(|b| b.files.into_iter().collect())
    };
    findings.extend(adjudicate_candidates(
        &key,
        &facet,
        &candidates,
        window.as_ref(),
        &now,
    ));

    // 2. Sample in-scope source artifacts for coverage. When a full walk is due
    //    (D3), enumerate the WHOLE source of every enumerable facet — guaranteeing
    //    complete coverage this run; otherwise sample a bounded rotating window
    //    (D2). Non-enumerable facets are refused (the typed refusal rides on
    //    `full_resync`), never silently claimed as covered.
    let sample_files: Vec<String> = if full_resync.is_full_walk() {
        let mut all: Vec<String> = Vec::new();
        for source in &resolved.sources {
            if let ResolvedSource::Primary(p) = source
                && medium_capabilities(p.medium_type).enumerable
            {
                all.extend(enumerate_facet_files(
                    p,
                    &resolved.deny_paths,
                    workspace_root,
                ));
            }
        }
        all.sort();
        all.dedup();
        all
    } else {
        next_batch(resolved, workspace_root, &cache_root, sample_batch)
            .map(|b| b.files)
            .unwrap_or_default()
    };
    for file in sample_files {
        let covered = engine
            .anchors_referencing_artifact(&file)
            .iter()
            .any(|(eid, _)| eid.mem() == resolved.destination_mem.as_str());
        if !covered {
            findings.push(Finding {
                key: key.clone(),
                facet: facet.clone(),
                target: FindingTarget::Artifact { artifact: file },
                class: FindingClass::Uncovered,
                detail: "source artifact in scope has no anchor in the destination mem".to_string(),
                created_at: now.clone(),
            });
        }
    }

    let backlog = findings
        .iter()
        .filter(|f| f.class == FindingClass::QueuedForAdjudication)
        .count();

    // Load-or-init, record under the current key (prior-key batches retained,
    // segregated — A3), persist to the durable state tier (A1).
    let mut store = read_findings_store(workspace_root, &mem, &name)
        .map_err(FindingsError::Store)?
        .unwrap_or_else(|| FindingsStore {
            binding: binding_id.clone(),
            ..Default::default()
        });
    let recorded = findings.len();
    store.record(key.clone(), now, findings);
    let superseded = store.superseded(&key).len();
    write_findings_store(workspace_root, &mem, &name, &store).map_err(FindingsError::Store)?;

    Ok(VerifyOutcome {
        binding: binding_id,
        key,
        recorded,
        superseded,
        backlog,
        full_resync,
        facet_heads,
    })
}

/// The medium type's wire string (`codebase` / `web` / …) — the serde form the
/// capability matrix and reports use.
fn medium_type_wire(t: crate::pipeline::MediumType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::{Anchor, AnchorGrain, AnchorHashStability, AnchorProvenanceClass};

    fn key(hash: &str, head: &str) -> FindingKey {
        FindingKey {
            binding_hash: hash.to_string(),
            source_head: head.to_string(),
        }
    }

    fn anchor(class: AnchorProvenanceClass) -> Anchor {
        Anchor {
            artifact: "src/lib.rs".to_string(),
            grain: AnchorGrain::File,
            class,
            at_version: None,
            hash: if class.is_hash_bearing() {
                Some("h1".to_string())
            } else {
                None
            },
            hash_stability: AnchorHashStability::Stable,
            derived_from: Vec::new(),
            binding: None,
        }
    }

    /// The store round-trips through serde and survives a write/read cycle on
    /// disk — the durability A1 rests on.
    #[test]
    fn store_round_trips_on_disk_and_delete_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(
            read_findings_store(root, "engine", "graph")
                .unwrap()
                .is_none()
        );

        let mut store = FindingsStore {
            binding: "engine/graph".to_string(),
            ..Default::default()
        };
        let k = key("hashA", "head1");
        store.record(
            k.clone(),
            "1".to_string(),
            vec![Finding {
                key: k.clone(),
                facet: "src".to_string(),
                target: FindingTarget::Artifact {
                    artifact: "src/a.rs".to_string(),
                },
                class: FindingClass::Uncovered,
                detail: "d".to_string(),
                created_at: "1".to_string(),
            }],
        );
        write_findings_store(root, "engine", "graph", &store).unwrap();
        assert!(findings_store_path(root, "engine", "graph").exists());

        // The store subtree self-ignores: per-checkout engine state must
        // not surface as untracked noise in a tracked workspace.
        let ignore = root
            .join(WORKSPACE_STORE_DIR)
            .join(STATE_DIR)
            .join(FINDINGS_DIR)
            .join(".gitignore");
        assert_eq!(std::fs::read_to_string(&ignore).unwrap(), "*\n");

        // Fresh read from disk (a later process) sees the findings (A1).
        let back = read_findings_store(root, "engine", "graph")
            .unwrap()
            .unwrap();
        assert_eq!(back, store);
        assert_eq!(back.current(&k).len(), 1);

        delete_findings_store(root, "engine", "graph").unwrap();
        assert!(
            read_findings_store(root, "engine", "graph")
                .unwrap()
                .is_none()
        );
        // Idempotent.
        delete_findings_store(root, "engine", "graph").unwrap();
    }

    /// A3 — a changed `hash(D)` segregates the prior batch: findings under the
    /// old hash are never `current` under the new key, only `superseded`.
    #[test]
    fn changed_binding_hash_supersedes_prior_findings() {
        let mut store = FindingsStore::default();
        let old = key("hashOLD", "head1");
        let new = key("hashNEW", "head1");
        let f_old = Finding {
            key: old.clone(),
            facet: "src".to_string(),
            target: FindingTarget::Artifact {
                artifact: "src/old.rs".to_string(),
            },
            class: FindingClass::Uncovered,
            detail: "old".to_string(),
            created_at: "1".to_string(),
        };
        store.record(old.clone(), "1".to_string(), vec![f_old.clone()]);

        // Recording under the new key must not touch the old batch.
        store.record(new.clone(), "2".to_string(), Vec::new());
        assert!(store.current(&new).is_empty(), "new key has its own view");
        let superseded = store.superseded(&new);
        assert_eq!(superseded.len(), 1, "old batch is segregated as superseded");
        assert_eq!(superseded[0], &f_old);
        // The old findings are never presented as current under the new key.
        assert!(!store.current(&new).contains(&f_old));
    }

    /// A3 — a moved `source_head` segregates the prior batch the same way (the
    /// key differs in its `source_head` component, not its `hash(D)`).
    #[test]
    fn moved_source_head_supersedes_prior_findings() {
        let mut store = FindingsStore::default();
        let before = key("hashA", "head1");
        let after = key("hashA", "head2");
        let f = Finding {
            key: before.clone(),
            facet: "src".to_string(),
            target: FindingTarget::Anchor {
                entity: "engine--e".to_string(),
                artifact: "src/x.rs".to_string(),
            },
            class: FindingClass::UnresolvableAnchor,
            detail: "gone".to_string(),
            created_at: "1".to_string(),
        };
        store.record(before.clone(), "1".to_string(), vec![f.clone()]);
        store.record(after.clone(), "2".to_string(), Vec::new());

        assert!(store.current(&after).is_empty());
        assert_eq!(store.superseded(&after), vec![&f]);
        // Recording the same key again replaces in place (no duplicate batch).
        store.record(after.clone(), "3".to_string(), Vec::new());
        assert_eq!(store.batches.len(), 2, "one batch per distinct key");
    }

    /// A2 — hash-drift adjudication is excluded for `informed-by` (and every
    /// non-hash-bearing class): a drifted/recheck state yields NO finding.
    #[test]
    fn informed_by_anchor_never_drifts() {
        let k = key("h", "s");
        for class in [
            AnchorProvenanceClass::InformedBy,
            AnchorProvenanceClass::Authored,
        ] {
            let a = anchor(class);
            assert!(
                adjudicate_anchor(&k, "f", "engine--e", &a, AnchorState::Drifted, "1").is_none(),
                "{class:?} must not produce a drift finding"
            );
            assert!(
                adjudicate_anchor(&k, "f", "engine--e", &a, AnchorState::Recheck, "1").is_none(),
                "{class:?} must not produce a queued finding"
            );
        }
    }

    /// A2 — hash-bearing classes DO produce drift/recheck findings, and every
    /// class produces an existence (`unresolvable-anchor`) finding when orphaned.
    #[test]
    fn hash_bearing_drifts_and_orphan_is_class_independent() {
        let k = key("h", "s");
        let anchored = anchor(AnchorProvenanceClass::Anchored);
        let drifted =
            adjudicate_anchor(&k, "f", "engine--e", &anchored, AnchorState::Drifted, "1").unwrap();
        assert_eq!(drifted.class, FindingClass::Drifted);
        assert_eq!(drifted.key, k, "the finding carries its recording key (A2)");

        let queued =
            adjudicate_anchor(&k, "f", "engine--e", &anchored, AnchorState::Recheck, "1").unwrap();
        assert_eq!(queued.class, FindingClass::QueuedForAdjudication);

        // Orphaned is existence, not hash-drift — reported for informed-by too.
        let informed = anchor(AnchorProvenanceClass::InformedBy);
        let orphan =
            adjudicate_anchor(&k, "f", "engine--e", &informed, AnchorState::Orphaned, "1").unwrap();
        assert_eq!(orphan.class, FindingClass::UnresolvableAnchor);

        // Resolves yields nothing.
        assert!(
            adjudicate_anchor(&k, "f", "engine--e", &anchored, AnchorState::Resolves, "1")
                .is_none()
        );
    }

    /// The finding class vocabulary round-trips through its wire form.
    #[test]
    fn finding_class_wire_round_trips() {
        for w in FindingClass::WIRE_VALUES {
            let c = FindingClass::from_wire(w).expect("known wire value");
            assert_eq!(c.as_wire(), *w);
        }
        assert!(FindingClass::from_wire("nonsense").is_none());
    }

    /// A malformed binding id refuses before touching the store tier.
    #[test]
    fn malformed_binding_id_refuses() {
        assert!(matches!(
            split_binding_id("../escape"),
            Err(FindingsError::MalformedId(_))
        ));
        assert!(matches!(
            split_binding_id("no-slash"),
            Err(FindingsError::MalformedId(_))
        ));
        assert_eq!(
            split_binding_id("engine/graph").unwrap(),
            ("engine".to_string(), "graph".to_string())
        );
    }

    // ---- A1/A5 end-to-end: verify writes durable findings, read-only on mem --

    use crate::anchor::AnchorSidecar;
    use crate::binding::{
        BINDING_VERSION, BuildMode, BuildOperation, CoverageSemantics, DEFAULT_ADJUDICATION_CAP,
        DEFAULT_FULL_RESYNC_EVERY, Operations, VerifyOperation,
    };
    use crate::ingest::resolve::resolve_binding_run;
    use crate::pipeline::{Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::{load_pipeline_configs, write_binding, write_facet, write_medium};
    use crate::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use crate::workspace_store::WorkspaceStoreAdapter;

    /// A full verify pass over a folder mem: it adjudicates the mem's anchors
    /// against the live source (orphaned → unresolvable-anchor; present
    /// hash-bearing → queued; informed-by → no finding, A2) and flags an
    /// uncovered source file, then persists the findings to the durable state
    /// tier. A **fresh** read from disk (a later process) sees them (A1). The
    /// pass runs on a shared `&Engine` — structurally read-only on the mem (A5).
    #[test]
    fn verify_persists_findings_readable_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mem_dir = root.join("mem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();

        // Workspace state so `from_workspace_root` sets `workspace_root` (which
        // the anchor observation and cursor need) and mounts the `engine` mem.
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: mem_dir.clone(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![mount],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // A git work tree at the workspace root so the codebase medium's `git`
        // change strategy resolves; source files: one anchored+present, one
        // uncovered.
        let out = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("present.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("src").join("uncovered.rs"), "fn b() {}\n").unwrap();

        // Seed the engine-owned anchors sidecar directly (test fixture — the
        // production write path is the mutation surface, not this verify code).
        let mk = |artifact: &str, class: AnchorProvenanceClass| Anchor {
            artifact: artifact.to_string(),
            grain: AnchorGrain::File,
            class,
            at_version: None,
            hash: class.is_hash_bearing().then(|| "recorded".to_string()),
            hash_stability: AnchorHashStability::Stable,
            derived_from: Vec::new(),
            binding: None,
        };
        let mut sidecar = AnchorSidecar::default();
        sidecar.set(
            "engine--e",
            vec![
                mk("src/present.rs", AnchorProvenanceClass::Anchored), // present, hash-bearing → recheck → queued
                mk("src/gone.rs", AnchorProvenanceClass::Anchored), // absent → unresolvable-anchor
                mk("src/present.rs", AnchorProvenanceClass::InformedBy), // present, non-hash → no finding (A2)
            ],
        );
        std::fs::write(
            mem_dir.join(crate::anchor::ANCHOR_SIDECAR_PATH),
            sidecar.to_bytes(),
        )
        .unwrap();

        // Binding engine/graph over a codebase facet (medium root = workspace).
        write_medium(
            root,
            "engine",
            "graph",
            &Medium {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
            },
        )
        .unwrap();
        write_facet(
            root,
            "engine",
            "graph",
            &Facet {
                name: "graph".to_string(),
                medium: "graph".to_string(),
                scope: vec![PatternEntry {
                    path: "src/**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            },
        )
        .unwrap();
        write_binding(
            root,
            "engine",
            "graph",
            &BindingV1 {
                version: BINDING_VERSION,
                intent: None,
                source_facets: vec!["graph".to_string()],
                reference_mems: Vec::new(),
                destination_mem: "engine".to_string(),
                deny_paths: Vec::new(),
                coverage_semantics: CoverageSemantics::Exhaustive,
                rules: None,
                prune: None,
                operations: Operations {
                    build: Some(BuildOperation {
                        mode: BuildMode::Discovery,
                        trigger: IngestTrigger::Loop,
                        batch_size: 20,
                        post_actions: None,
                    }),
                    sync: None,
                    verify: Some(VerifyOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                        adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                        full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                    }),
                },
            },
        )
        .unwrap();

        let engine = Engine::from_workspace_root(root).unwrap();

        let configs = load_pipeline_configs(root).unwrap();
        let binding = &configs.bindings[0].config;
        let resolved = resolve_binding_run(&configs, "engine/graph", binding).unwrap();

        // `&engine` — shared borrow, structurally cannot mutate the mem (A5).
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        assert!(
            outcome.recorded >= 3,
            "orphan + queued + uncovered at least"
        );
        assert_eq!(outcome.superseded, 0, "no prior key yet");
        assert_eq!(outcome.backlog, 1, "the present hash-bearing anchor queued");

        // Fresh read from disk — a later process / sync-brief render (A1).
        let store = read_findings_store(root, "engine", "graph")
            .unwrap()
            .unwrap();
        let current = store.current(&outcome.key);
        assert_eq!(current.len(), outcome.recorded);

        let has = |c: FindingClass, art: &str| {
            current.iter().any(|f| {
                f.class == c
                    && match &f.target {
                        FindingTarget::Anchor { artifact, .. } => artifact == art,
                        FindingTarget::Artifact { artifact } => artifact == art,
                    }
            })
        };
        assert!(has(FindingClass::UnresolvableAnchor, "src/gone.rs"));
        assert!(has(FindingClass::QueuedForAdjudication, "src/present.rs"));
        assert!(has(FindingClass::Uncovered, "src/uncovered.rs"));
        // A2: the informed-by anchor on the present file produced no finding.
        assert!(
            !current
                .iter()
                .any(|f| f.class == FindingClass::Drifted || f.class == FindingClass::Wrong),
            "no drift finding from a non-hash / present-clean anchor"
        );
        // The covered file is not flagged uncovered.
        assert!(!has(FindingClass::Uncovered, "src/present.rs"));
    }

    /// The completed-run `#verified` writer (backlog 2026-07-11): a verify
    /// pass surfaces its observed facet heads on the outcome (the per-facet
    /// decomposition of `key.source_head`), and [`record_verified_baseline`]
    /// records them as `<binding>/<facet>#verified` through the engine's
    /// sync-state writer — durable on disk, visible to the same config read
    /// `report`/`status` (and the macOS app) consume. A failed pass returns
    /// `Err` before any caller reaches the writer, so the token never
    /// advances on an aborted run.
    /// A vanished source directory must refuse verify with the typed
    /// `SourceUnreachable` error instead of degrading to an empty
    /// enumeration: pre-fix, the missing tree produced an empty stat map
    /// whose aggregate (the digest of nothing) completed the run and let
    /// the caller overwrite a genuine `#verified` baseline with fake
    /// state. The engine mem itself stays loadable — only the binding's
    /// source is gone.
    #[test]
    fn verify_refuses_unreachable_source_with_typed_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
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
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: mem_dir.clone(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![mount],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // The medium points at a subdirectory that does NOT exist — the
        // vanished-source case (`git` declared, so pre-fix the strategy
        // layer silently degraded instead of refusing).
        write_medium(
            root,
            "engine",
            "gone",
            &Medium {
                name: "gone".to_string(),
                medium_type: MediumType::Codebase,
                pointer: "vanished-src".to_string(),
                change_detection: Some("git".to_string()),
            },
        )
        .unwrap();
        write_facet(
            root,
            "engine",
            "gone",
            &Facet {
                name: "gone".to_string(),
                medium: "gone".to_string(),
                scope: vec![PatternEntry {
                    path: "**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            },
        )
        .unwrap();
        write_binding(
            root,
            "engine",
            "gone",
            &BindingV1 {
                version: BINDING_VERSION,
                intent: None,
                source_facets: vec!["gone".to_string()],
                reference_mems: Vec::new(),
                destination_mem: "engine".to_string(),
                deny_paths: Vec::new(),
                coverage_semantics: CoverageSemantics::Exhaustive,
                rules: None,
                prune: None,
                operations: Operations {
                    build: None,
                    sync: None,
                    verify: Some(VerifyOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                        adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                        full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                    }),
                },
            },
        )
        .unwrap();

        let engine = Engine::from_workspace_root(root).unwrap();
        let configs = load_pipeline_configs(root).unwrap();
        let binding = &configs.bindings[0].config;
        let resolved = resolve_binding_run(&configs, "engine/gone", binding).unwrap();

        match verify_binding(&engine, root, binding, &resolved) {
            Err(FindingsError::SourceUnreachable {
                facet,
                medium,
                path,
            }) => {
                assert_eq!(facet, "gone");
                assert_eq!(medium, "gone");
                assert!(
                    path.ends_with("vanished-src"),
                    "refusal must name the resolved missing path, got `{path}`",
                );
            }
            other => panic!("expected SourceUnreachable refusal, got {other:?}"),
        }

        // Nothing was observed → no `#verified` token exists (the caller
        // never reaches its baseline write on an Err).
        assert!(
            !engine
                .mem_config_for("engine")
                .unwrap()
                .sync_state
                .keys()
                .any(|k| k.ends_with("#verified")),
            "a refused verify must not leave any #verified token",
        );
    }

    #[test]
    fn completed_verify_records_the_verified_baseline() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
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
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: mem_dir.clone(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![mount],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();
        let out = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());

        write_medium(
            root,
            "engine",
            "graph",
            &Medium {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
            },
        )
        .unwrap();
        write_facet(
            root,
            "engine",
            "graph",
            &Facet {
                name: "graph".to_string(),
                medium: "graph".to_string(),
                scope: vec![PatternEntry {
                    path: "src/**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            },
        )
        .unwrap();
        write_binding(
            root,
            "engine",
            "graph",
            &BindingV1 {
                version: BINDING_VERSION,
                intent: None,
                source_facets: vec!["graph".to_string()],
                reference_mems: Vec::new(),
                destination_mem: "engine".to_string(),
                deny_paths: Vec::new(),
                coverage_semantics: CoverageSemantics::Exhaustive,
                rules: None,
                prune: None,
                operations: Operations {
                    build: Some(BuildOperation {
                        mode: BuildMode::Discovery,
                        trigger: IngestTrigger::Loop,
                        batch_size: 20,
                        post_actions: None,
                    }),
                    sync: None,
                    verify: Some(VerifyOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                        adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                        full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                    }),
                },
            },
        )
        .unwrap();

        let mut engine = Engine::from_workspace_root(root).unwrap();
        // A recorded `#synced` baseline is this facet's current head (the git
        // work tree has no commits, so the cursor contributes no newer token).
        engine
            .set_mem_sync_state("engine", "engine/graph/graph#synced", "deadbeef", None)
            .unwrap();

        let configs = load_pipeline_configs(root).unwrap();
        let binding = &configs.bindings[0].config;
        let resolved = resolve_binding_run(&configs, "engine/graph", binding).unwrap();

        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        // The outcome decomposes its own key: joined facet heads == source_head.
        assert_eq!(
            outcome.facet_heads.get("graph").map(String::as_str),
            Some("deadbeef")
        );
        assert_eq!(outcome.key.source_head, "graph=deadbeef");
        assert_eq!(
            join_facet_heads(&outcome.facet_heads),
            outcome.key.source_head
        );

        // No `#verified` token exists before the writer runs.
        assert!(
            !engine
                .mem_config_for("engine")
                .unwrap()
                .sync_state
                .contains_key("engine/graph/graph#verified")
        );

        let written = record_verified_baseline(&mut engine, "engine", &outcome, None).unwrap();
        assert_eq!(written, vec!["engine/graph/graph#verified".to_string()]);

        // Visible to the engine's config read (the app's sync_state source)…
        assert_eq!(
            engine
                .mem_config_for("engine")
                .unwrap()
                .sync_state
                .get("engine/graph/graph#verified")
                .map(String::as_str),
            Some("deadbeef")
        );
        // …and durable on disk (what a fresh CLI process reads).
        let disk: serde_json::Value = serde_json::from_slice(
            &std::fs::read(mem_dir.join(".memstead").join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            disk["syncState"]["engine/graph/graph#verified"],
            serde_json::json!("deadbeef")
        );
    }

    // ---- D1: per-run adjudication cap -----------------------------------

    /// D1 — the per-run cap queues the remainder. A rotation window covering
    /// only a subset of drift candidates adjudicates the in-window ones and
    /// QUEUES every out-of-window candidate as `queued-for-adjudication` (the
    /// tier-3 backlog). Uncapped (`window = None`) adjudicates every candidate.
    #[test]
    fn adjudication_cap_queues_the_remainder() {
        let k = key("h", "s");
        let mk = |art: &str| {
            let mut a = anchor(AnchorProvenanceClass::Anchored);
            a.artifact = art.to_string();
            a
        };
        let candidates = vec![
            (
                "engine--a".to_string(),
                mk("src/a.rs"),
                AnchorState::Drifted,
            ),
            (
                "engine--b".to_string(),
                mk("src/b.rs"),
                AnchorState::Drifted,
            ),
            (
                "engine--c".to_string(),
                mk("src/c.rs"),
                AnchorState::Drifted,
            ),
        ];
        // A cap-1 window selects only src/a.rs.
        let window: BTreeSet<String> = [candidate_key("engine--a", &mk("src/a.rs"))]
            .into_iter()
            .collect();
        let out = adjudicate_candidates(&k, "f", &candidates, Some(&window), "1");
        let drifted = out
            .iter()
            .filter(|f| f.class == FindingClass::Drifted)
            .count();
        let queued = out
            .iter()
            .filter(|f| f.class == FindingClass::QueuedForAdjudication)
            .count();
        assert_eq!(drifted, 1, "only the in-window candidate is adjudicated");
        assert_eq!(queued, 2, "the remainder is queued as the tier-3 backlog");
        // A queued remainder finding carries the queued detail, not a drift claim.
        assert!(
            out.iter()
                .any(|f| f.class == FindingClass::QueuedForAdjudication
                    && f.detail.contains("cap reached")),
            "capped remainder states it was deferred by the cap"
        );

        // Uncapped: every candidate adjudicated, none queued.
        let uncapped = adjudicate_candidates(&k, "f", &candidates, None, "1");
        assert_eq!(
            uncapped
                .iter()
                .filter(|f| f.class == FindingClass::Drifted)
                .count(),
            3,
            "uncapped adjudicates every candidate"
        );
        assert_eq!(
            uncapped
                .iter()
                .filter(|f| f.class == FindingClass::QueuedForAdjudication)
                .count(),
            0
        );
    }

    // ---- D3: full_resync scheduling + non-enumerable refusal ------------

    /// D3 — `schedule_full_resync`: disabled at cadence 0; not-due off-cadence
    /// (with a countdown); due on-cadence for an enumerable facet (walked, no
    /// refusal).
    #[test]
    fn full_resync_schedule_disabled_notdue_due() {
        let codebase = FacetEnumerability {
            facet: "src".to_string(),
            medium_type: "codebase".to_string(),
            enumerable: true,
        };
        assert_eq!(
            schedule_full_resync(0, 5, std::slice::from_ref(&codebase)),
            FullResyncDecision::Disabled
        );
        match schedule_full_resync(3, 2, std::slice::from_ref(&codebase)) {
            FullResyncDecision::NotDue { runs_until_due, .. } => assert_eq!(runs_until_due, 1),
            other => panic!("expected NotDue, got {other:?}"),
        }
        match schedule_full_resync(3, 3, std::slice::from_ref(&codebase)) {
            FullResyncDecision::Due {
                walked_facets,
                refused,
                ..
            } => {
                assert_eq!(walked_facets, vec!["src".to_string()]);
                assert!(refused.is_empty(), "enumerable facet is not refused");
            }
            other => panic!("expected Due, got {other:?}"),
        }
    }

    /// D3 REFUSAL — a scheduled full walk over a NON-enumerable medium refuses
    /// with a typed signal: it never claims coverage and is never a silent skip.
    #[test]
    fn full_resync_refuses_non_enumerable_medium() {
        let web = FacetEnumerability {
            facet: "manual".to_string(),
            medium_type: "web".to_string(),
            enumerable: false,
        };
        let d = schedule_full_resync(1, 1, &[web]);
        assert!(
            d.is_full_walk(),
            "a due sweep is a full walk even when refused"
        );
        match d {
            FullResyncDecision::Due {
                walked_facets,
                refused,
                ..
            } => {
                assert!(walked_facets.is_empty(), "nothing enumerable to walk");
                assert_eq!(refused.len(), 1, "the non-enumerable facet is refused");
                assert_eq!(refused[0].facet, "manual");
                assert_eq!(refused[0].medium_type, "web");
                assert!(
                    refused[0].reason.contains("non-enumerable"),
                    "the refusal is typed and states why"
                );
            }
            other => panic!("expected Due with a refusal, got {other:?}"),
        }
    }

    /// D3 — a scheduled full walk fires the WHOLE-source enumeration this run:
    /// with `full_resync_every = 1` (due every run) and a sample `batch_size` of
    /// 1, all three uncovered source files are flagged, not just one — the full
    /// walk overrides the bounded rotating sample for an enumerable medium.
    #[test]
    fn full_resync_full_walk_covers_whole_source() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
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
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: mem_dir.clone(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![mount],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();
        let out = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::create_dir_all(root.join("src")).unwrap();
        for f in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(root.join("src").join(f), "fn x() {}\n").unwrap();
        }

        write_medium(
            root,
            "engine",
            "graph",
            &Medium {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
            },
        )
        .unwrap();
        write_facet(
            root,
            "engine",
            "graph",
            &Facet {
                name: "graph".to_string(),
                medium: "graph".to_string(),
                scope: vec![PatternEntry {
                    path: "src/**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            },
        )
        .unwrap();
        write_binding(
            root,
            "engine",
            "graph",
            &BindingV1 {
                version: BINDING_VERSION,
                intent: None,
                source_facets: vec!["graph".to_string()],
                reference_mems: Vec::new(),
                destination_mem: "engine".to_string(),
                deny_paths: Vec::new(),
                coverage_semantics: CoverageSemantics::Exhaustive,
                rules: None,
                prune: None,
                operations: Operations {
                    build: Some(BuildOperation {
                        mode: BuildMode::Discovery,
                        trigger: IngestTrigger::Loop,
                        batch_size: 20,
                        post_actions: None,
                    }),
                    sync: None,
                    verify: Some(VerifyOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 1, // a tiny rotating sample …
                        adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                        full_resync_every: 1, // … but a full walk fires EVERY run
                    }),
                },
            },
        )
        .unwrap();

        let engine = Engine::from_workspace_root(root).unwrap();
        let configs = load_pipeline_configs(root).unwrap();
        let binding = &configs.bindings[0].config;
        let resolved = resolve_binding_run(&configs, "engine/graph", binding).unwrap();

        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        // The full walk is due on run 1 and covers the enumerable facet.
        match &outcome.full_resync {
            FullResyncDecision::Due {
                walked_facets,
                refused,
                run_count,
                ..
            } => {
                assert_eq!(*run_count, 1);
                assert_eq!(walked_facets, &vec!["graph".to_string()]);
                assert!(refused.is_empty());
            }
            other => panic!("expected a due full walk, got {other:?}"),
        }
        // All three uncovered files flagged despite the batch_size-1 sample.
        let store = read_findings_store(root, "engine", "graph")
            .unwrap()
            .unwrap();
        let uncovered = store
            .current(&outcome.key)
            .iter()
            .filter(|f| f.class == FindingClass::Uncovered)
            .count();
        assert_eq!(
            uncovered, 3,
            "the scheduled full walk covers the whole source, not a batch of one"
        );
    }
}
