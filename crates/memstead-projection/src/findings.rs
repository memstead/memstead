//! The engine-owned durable **findings store** and the thin `projection verify`
//! write path that populates it.
//!
//! Verify **measures** fidelity and records durable findings; it mutates no
//! entity in the destination mem (though a completed run does write this
//! store, backfill observed anchor hashes, and record a `#verified`
//! baseline). The store is the real home behind plan 03's findings
//! schema stub ([`memstead_base::binding`]'s removed `FindingKey` / `FindingRecord`).
//!
//! ## Keying: `hash(D)` alone — findings survive head movement
//!
//! The store keys on the binding's **`hash(D)` alone**: a binding-declaration
//! edit still mechanically partitions findings into a fresh keyspace (prior
//! findings are never presented as current, only segregated as superseded —
//! A3), but a **source-head move does not**. Each finding records the
//! `source_head` it was observed at as metadata (its [`Finding::key`]), and
//! sync briefs present **all** open findings under the current `hash(D)`
//! regardless of recorded head — an open finding survives source movement and
//! keeps appearing until an agent's repair lets a verify observe it clean, or
//! a verify supersedes it. (Originally the key was `(hash(D), source_head)`,
//! which leaked exactly the findings sync exists to consume: once the source
//! advanced, open findings recorded at the previous head went invisible to
//! every subsequent brief.) The store does not grow unboundedly: verify
//! re-observes every anchor each pass and closes what resolves clean, and a
//! carried coverage finding whose artifact left `S(D)` or gained an anchor is
//! closed, not carried (see [`merge_with_prior`]). On-disk format is unchanged
//! — pre-re-key stores (batches keyed `(hash(D), source_head)`) load as-is;
//! same-hash batches from different heads collapse under the hash-alone view
//! (the latest-recorded batch is current, the rest superseded until the next
//! verify rewrites the hash's batch).
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
//! through the sync brief, never through findings recording/reading.
//! Two sanctioned post-run writes exist, both explicit separate steps the
//! caller performs only after a pass returns `Ok` (so an aborted or failed
//! run never records either), and both measurement bookkeeping — never entity
//! content: the **verified baseline** ([`record_verified_baseline`] records
//! `<binding>/<facet>#verified` per observed facet head through the lifecycle
//! sync-state writer) and the **prepared-hash backfill**
//! ([`record_anchor_hash_backfill`] records this pass's observed
//! prepared-content hashes onto hash-less hash-bearing anchors in the
//! engine-owned anchors sidecar).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use memstead_base::Engine;
use memstead_base::WarningHint;
use memstead_base::anchor::{Anchor, AnchorState, ObservedArtifactHash};
use memstead_base::binding::{
    Binding, DEFAULT_ADJUDICATION_CAP, DEFAULT_FULL_RESYNC_EVERY, hash_binding, medium_capabilities,
};
use memstead_base::entity::EntityId;
use memstead_base::workspace_store::{StoreError, WORKSPACE_STORE_DIR};

use super::advance::is_single_component;
use super::cursor::{compute_source_cursor, enumerate_source_artifacts};
use super::refinement::{
    ROTATION_ANCHOR_ADJUDICATION, bump_verify_runs, next_batch, next_rotation_batch,
};
use memstead_base::binding_run::{ResolvedIngest, ResolvedSource};

/// The engine-owned state directory root, under the workspace store:
/// `<root>/.memstead/state/`. Mirrors [`super::advance`]'s `STATE_DIR`.
const STATE_DIR: &str = "state";
/// The findings store's subtree: `<root>/.memstead/state/findings/`.
const FINDINGS_DIR: &str = "findings";

// ---------------------------------------------------------------------------
// Key
// ---------------------------------------------------------------------------

/// A binding's `hash(D)` plus the `source_head` a finding was observed at.
///
/// Only the **`binding_hash` half keys the store**: a changed `hash(D)` (a
/// binding-declaration edit) invalidates prior findings by construction —
/// segregated as superseded, never silently mixed into the current view (A3).
/// The `source_head` half is **observation metadata**, carried on every
/// finding so it stays self-describing about when it was observed — a moved
/// head does NOT invalidate a finding (findings survive head movement; see
/// the module docs).
///
/// The real key behind plan 03's schema stub (which lived, IO-less, in
/// [`memstead_base::binding`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FindingKey {
    /// The binding's `hash(D)` (lowercase hex SHA-256; see
    /// [`memstead_base::binding::hash_binding`]) — the store key.
    pub binding_hash: String,
    /// The composite source-head token the finding was observed at — the
    /// per-facet baseline tokens current at observation time. Metadata, not
    /// part of the store key.
    pub source_head: String,
}

// ---------------------------------------------------------------------------
// Finding
// ---------------------------------------------------------------------------

/// The class of a verify finding (A2). A closed vocabulary: `drifted` and
/// `queued-for-adjudication` come only from **hash-drift adjudication** (over
/// hash-bearing anchors — never `authored` / `informed-by`, see
/// [`adjudicate_anchor`]); `unresolvable-anchor` is an existence failure;
/// `uncovered` marks a source artifact with no anchor; `unanchored-mention`
/// marks a destination entity naming an in-scope artifact it carries no
/// anchor on; `wrong` is reserved for an adjudicated content mismatch the
/// group-B report renders.
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
    /// A destination entity names an in-scope source artifact in its body
    /// (by path, outside fenced code) and carries no anchor on it: a claim
    /// about a file no verify watches on the entity's behalf. A finding, never
    /// a refusal; the remedy is an anchor (`memstead_update` with `anchors`)
    /// or an authored exclusion of the artifact with a rationale.
    UnanchoredMention,
}

impl FindingClass {
    /// Every wire string, in declaration order.
    pub const WIRE_VALUES: &'static [&'static str] = &[
        "drifted",
        "wrong",
        "uncovered",
        "unresolvable-anchor",
        "queued-for-adjudication",
        "unanchored-mention",
    ];

    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            FindingClass::Drifted => "drifted",
            FindingClass::Wrong => "wrong",
            FindingClass::Uncovered => "uncovered",
            FindingClass::UnresolvableAnchor => "unresolvable-anchor",
            FindingClass::QueuedForAdjudication => "queued-for-adjudication",
            FindingClass::UnanchoredMention => "unanchored-mention",
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
            "unanchored-mention" => Some(FindingClass::UnanchoredMention),
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
    /// A body mention: the entity whose prose names the artifact, the
    /// artifact it names, and the section the mention sits in (the first
    /// naming section when several do; the detail lists them all).
    Mention {
        /// The entity id (`mem--slug`) whose body names the artifact.
        entity: String,
        /// The source-side artifact id, in the spelling `S(D)` enumerates.
        artifact: String,
        /// The section key the mention was found in.
        section: String,
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

/// One batch of findings recorded for a single `hash(D)` in one verify pass.
/// A new pass under the same `hash(D)` replaces the batch (after
/// [`verify_binding`]'s merge carried forward what stays open); a pass under a
/// different `hash(D)` lands as a separate batch — the prior one is retained,
/// segregated, never overwritten (A3). The batch's `key.source_head` is the
/// head the batch was last **recorded** at; each finding's own key records the
/// head *it* was observed at (a carried finding keeps its original).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingsBatch {
    /// The key this batch was recorded under (`binding_hash` is the store
    /// key; `source_head` is the recording head, metadata).
    pub key: FindingKey,
    /// When the batch was last recorded (opaque timestamp string).
    pub recorded_at: String,
    /// The findings in this batch.
    pub findings: Vec<Finding>,
}

/// One binding's durable findings store (A1). Persisted at
/// `.memstead/state/findings/<mem>/<name>.json`, read fresh per call. Holds
/// findings grouped by the `hash(D)` they were recorded under so declaration
/// invalidation is mechanical: [`Self::current`] presents the current hash's
/// batch — regardless of source head; [`Self::superseded`] surfaces everything
/// else, segregated (A3).
///
/// The on-disk shape predates the hash-alone re-key and is unchanged: a store
/// written when batches were keyed `(hash(D), source_head)` loads without loss.
/// Such a legacy store may hold several batches sharing one `binding_hash`
/// (recorded at different heads); the hash-alone view treats the
/// latest-recorded of them as current and the next [`Self::record`] collapses
/// them into one.
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
    /// Index of the store's current batch for `binding_hash`: the
    /// latest-recorded batch carrying that hash (ties break toward the later
    /// entry — [`Self::record`] appends). Usually unique; a legacy per-head
    /// store may hold several.
    fn current_batch_index(&self, binding_hash: &str) -> Option<usize> {
        self.batches
            .iter()
            .enumerate()
            .filter(|(_, b)| b.key.binding_hash == binding_hash)
            .max_by_key(|(i, b)| (b.recorded_at.parse::<u64>().unwrap_or(0), *i))
            .map(|(i, _)| i)
    }

    /// Record `findings` under `key.binding_hash`, replacing **every** prior
    /// batch recorded under that hash (including legacy per-head siblings) and
    /// leaving every other hash's batch untouched (A3 segregation — a changed
    /// `hash(D)` never overwrites the old batch).
    pub fn record(&mut self, key: FindingKey, recorded_at: String, findings: Vec<Finding>) {
        self.batches
            .retain(|b| b.key.binding_hash != key.binding_hash);
        self.batches.push(FindingsBatch {
            key,
            recorded_at,
            findings,
        });
    }

    /// The findings recorded under `key.binding_hash` — the **only** findings
    /// ever presented as current (A3), **regardless of `key.source_head`**: an
    /// open finding recorded at a previous head stays presented after the
    /// source advances. Empty when nothing was recorded under this hash.
    pub fn current(&self, key: &FindingKey) -> &[Finding] {
        self.current_batch_index(&key.binding_hash)
            .map(|i| self.batches[i].findings.as_slice())
            .unwrap_or(&[])
    }

    /// Every finding **outside** the current view of `key.binding_hash` —
    /// superseded by a `hash(D)` change (or stranded in an older legacy
    /// per-head batch of the same hash), segregated so a consumer can show
    /// them as stale without mixing them into the current view (A3).
    pub fn superseded(&self, key: &FindingKey) -> Vec<&Finding> {
        let current = self.current_batch_index(&key.binding_hash);
        self.batches
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != current)
            .flat_map(|(_, b)| b.findings.iter())
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

/// The mem-scoped findings key for binding-less (standalone) anchor
/// verification. A distinguished constant that
/// can never collide with a real `hash(D)` (which is always 64 hex
/// chars): a hand-authored mem with no binding persists its verify
/// findings under this key, in its own store file
/// (`state/findings/<mem>/standalone.json`), closing the
/// observe-and-forget gap. Binding-backed stores keep their `hash(D)`
/// key and semantics untouched — the two keyspaces coexist and never
/// share a file.
pub const STANDALONE_KEY: &str = "standalone";

/// One standalone finding with its already-seen annotation: `true`
/// when the previous standalone pass recorded the same target and
/// class — the re-serving that makes a second pass say "known" rather
/// than rediscovering.
#[derive(Debug, Clone, Serialize)]
pub struct AnnotatedStandaloneFinding {
    #[serde(flatten)]
    pub finding: Finding,
    pub already_seen: bool,
}

/// Persist a standalone (binding-less) anchor-verification pass's
/// flagged findings under the mem-scoped [`STANDALONE_KEY`], and
/// annotate each against the previous pass. `drifted` and
/// `unresolvable` anchors become durable findings (`recheck` is
/// transient by definition and `resolved` is not a finding); a pass
/// whose flagged set is empty still records — the empty batch IS the
/// "everything resolved clean" statement that closes prior findings.
pub fn record_standalone_findings(
    workspace_root: &Path,
    report: &memstead_base::engine::query::MemAnchorVerification,
) -> Result<Vec<AnnotatedStandaloneFinding>, StoreError> {
    let mem = &report.mem;
    let key = FindingKey {
        binding_hash: STANDALONE_KEY.to_string(),
        source_head: String::new(),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string();

    let findings: Vec<Finding> = report
        .anchors
        .iter()
        .filter_map(|a| {
            // `unobserved` is deliberately absent (consistency-sweep 03/05).
            // A finding asserts a MEASURED condition, and an unobserved row is
            // the absence of a measurement: recording it as
            // `UnresolvableAnchor` claimed the artifact was gone when nobody
            // had looked, which is the collapse this removes. It is not
            // dropped silently either — the population statement and
            // `fully_adjudicated` on this same surface report it, and the
            // binding report raises it as a blind spot that blocks a clean
            // verdict.
            let class = match a.state.as_str() {
                "drifted" => FindingClass::Drifted,
                // The row spells the enum's wire name; the finding class
                // keeps the name the findings store is keyed by.
                "orphaned" => FindingClass::UnresolvableAnchor,
                _ => return None,
            };
            Some(Finding {
                key: key.clone(),
                facet: STANDALONE_KEY.to_string(),
                target: FindingTarget::Anchor {
                    entity: a.entity_id.clone(),
                    artifact: a.artifact.clone(),
                },
                class,
                detail: format!("{} ({} {})", a.state, a.class, a.grain),
                created_at: now.clone(),
            })
        })
        .collect();

    let mut store =
        read_findings_store(workspace_root, mem, STANDALONE_KEY)?.unwrap_or_else(|| {
            FindingsStore {
                binding: format!("{mem}/{STANDALONE_KEY}"),
                ..Default::default()
            }
        });
    let prior: BTreeSet<(String, String)> = store
        .current(&key)
        .iter()
        .map(|f| {
            (
                serde_json::to_string(&f.target).unwrap_or_default(),
                f.class.as_wire().to_string(),
            )
        })
        .collect();
    let annotated: Vec<AnnotatedStandaloneFinding> = findings
        .iter()
        .map(|f| AnnotatedStandaloneFinding {
            finding: f.clone(),
            already_seen: prior.contains(&(
                serde_json::to_string(&f.target).unwrap_or_default(),
                f.class.as_wire().to_string(),
            )),
        })
        .collect();
    store.record(key, now, findings);
    write_findings_store(workspace_root, mem, STANDALONE_KEY, &store)?;
    Ok(annotated)
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
    #[error("source '{source_name}' unreachable: `{path}` does not exist")]
    SourceUnreachable {
        /// The source whose pointer resolved to the missing path.
        source_name: String,
        /// The resolved base path that does not exist.
        path: String,
    },
    /// A full measurement ([`verify_binding_full`]) was requested over a
    /// facet whose medium the capability matrix marks **non-enumerable**: the
    /// full `S(D)` walk cannot cover it, so the whole run refuses — typed,
    /// carrying the same [`FullResyncRefusal`] shape the scheduled walk emits
    /// — rather than render a report with fabricated completeness. (The
    /// *scheduled* walk refuses per facet and walks the rest; an explicit
    /// full measurement promises complete figures, so a partial walk is not
    /// an answer.)
    #[error(
        "full verify refused: facet '{}' over medium type '{}' cannot be fully walked — {}",
        .0.facet, .0.medium_type, .0.reason
    )]
    FullWalkNonEnumerable(FullResyncRefusal),
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
    /// The full-enumeration scheduling decision for this run — whether a
    /// scheduled full walk fired, is not yet due, is disabled, and any typed
    /// non-enumerable refusals. Surfaced (never a silent skip) to the caller.
    pub full_resync: FullResyncDecision,
    /// Each source facet's current head token as observed by this run — the
    /// per-facet decomposition of `key.source_head`. The completed-run
    /// baseline [`record_verified_baseline`] writes as `#verified`.
    pub facet_heads: BTreeMap<String, String>,
    /// Prepared-content hashes this pass observed for **hash-less**
    /// hash-bearing (`anchored` / `derived`) anchors whose artifact resolved —
    /// the backfill worklist. The caller records them onto the anchors via
    /// [`record_anchor_hash_backfill`] after the pass returns `Ok` (the same
    /// sanctioned post-run-write pattern as [`record_verified_baseline`]);
    /// once recorded, subsequent verifies adjudicate those anchors
    /// deterministically and this list comes back empty. `authored` /
    /// `informed-by` anchors never appear here — the observation computes no
    /// hash for them.
    pub hash_backfill: Vec<ObservedArtifactHash>,
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
/// CLI `status`/report paths render it.
///
/// Returns the written sync-state keys. A binding whose run observed no facet
/// head (nothing recorded, nothing moved) writes nothing.
pub fn record_verified_baseline(
    engine: &mut Engine,
    destination_mem: &str,
    outcome: &VerifyOutcome,
    note: Option<&str>,
) -> Result<Vec<String>, memstead_base::engine::EngineError> {
    let mut written = Vec::with_capacity(outcome.facet_heads.len());
    for (facet, token) in &outcome.facet_heads {
        let key = format!("{}/{facet}#verified", outcome.binding);
        engine.set_mem_sync_state(destination_mem, &key, token, note)?;
        written.push(key);
    }
    Ok(written)
}

/// Record a **completed** verify run's prepared-hash backfill: every hash the
/// pass observed for a hash-less hash-bearing anchor
/// ([`VerifyOutcome::hash_backfill`]) is written onto that anchor in the
/// destination mem's engine-owned anchors sidecar, through
/// [`Engine::record_anchor_observed_hashes`].
///
/// Measurement bookkeeping only: the write touches the sidecar and nothing
/// else — no entity content, no section, no `_hash`. Deliberately a separate
/// step from [`verify_binding`] (which keeps its shared `&Engine` borrow —
/// A5), mirroring [`record_verified_baseline`]: the caller invokes this only
/// after a verify pass returned `Ok`, so an aborted or failed run never
/// records a hash. Idempotent — the engine writer skips anchors that already
/// carry a hash, and a pass over fully-backfilled anchors observes an empty
/// worklist, so re-verifying stages nothing and produces no commit.
///
/// Returns how many anchors gained a recorded hash.
pub fn record_anchor_hash_backfill(
    engine: &mut Engine,
    destination_mem: &str,
    outcome: &VerifyOutcome,
    note: Option<&str>,
) -> Result<usize, memstead_base::engine::EngineError> {
    engine.record_anchor_observed_hashes(destination_mem, &outcome.hash_backfill, note)
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
            ResolvedSource::Primary(p) => Some(p.name.as_str()),
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

/// `hash(D)` for a v2 binding — the record alone carries every content
/// input, so the resolved shape is not needed.
fn binding_hash_of(binding: &Binding, _resolved: &ResolvedIngest) -> String {
    hash_binding(binding)
}

/// The current [`FindingKey`] for a binding — `hash(D)` (the half that
/// keys the store) plus the current `source_head` (observation
/// metadata carried on each finding, not part of the store key).
fn current_key(
    engine: &Engine,
    workspace_root: &Path,
    binding: &Binding,
    resolved: &ResolvedIngest,
) -> FindingKey {
    FindingKey {
        binding_hash: binding_hash_of(binding, resolved),
        source_head: current_source_head(engine, workspace_root, resolved),
    }
}

/// The current `(hash(D), source_head)` key plus the open findings under the
/// key's `hash(D)` for a binding — the read the **sync brief**
/// consumes. It resolves the current key exactly as [`verify_binding`] does,
/// reads the durable store, and returns the `current(key)` slice cloned —
/// which presents **all open findings regardless of the head they were
/// recorded at** (findings survive source movement; each carries its observed
/// head on its own key). **Read-only** on the destination mem (shared
/// `&Engine`): no findings recording, no mutation. A binding whose store does
/// not exist yet yields the key and an empty vec.
///
/// The durable authored-exclusion ledger is consulted HERE, not only at
/// recording time: an `uncovered` finding whose artifact the ledger names is
/// dropped from the slice, so an exclusion `projection advance` /
/// `projection exclude` just accepted stops presenting on the very next
/// brief — without waiting for a verify pass to rewrite the stored batch.
/// (Recording has consulted the ledger since 2026-08-28; a batch recorded
/// before an exclusion landed still carried the finding, and three
/// independent runs read that as a repair that did not take.)
pub fn current_findings(
    engine: &Engine,
    workspace_root: &Path,
    binding: &Binding,
    resolved: &ResolvedIngest,
) -> Result<(FindingKey, Vec<Finding>), FindingsError> {
    let (mem, name) = split_binding_id(&resolved.name)?;
    let key = current_key(engine, workspace_root, binding, resolved);
    let mut findings = read_findings_store(workspace_root, &mem, &name)
        .map_err(FindingsError::Store)?
        .map(|s| s.current(&key).to_vec())
        .unwrap_or_default();
    let excluded: BTreeSet<String> =
        crate::advance::read_advance_store(workspace_root, &mem, &name)
            .ok()
            .flatten()
            .map(|state| state.exclusions.keys().cloned().collect())
            .unwrap_or_default();
    if !excluded.is_empty() {
        findings.retain(|f| {
            !(f.class == FindingClass::Uncovered
                && matches!(&f.target, FindingTarget::Artifact { artifact } if excluded.contains(artifact)))
        });
    }
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
/// reasons over. Built from the capability matrix per primary facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetEnumerability {
    /// The source facet.
    pub facet: String,
    /// The medium type wire string.
    pub medium_type: String,
    /// Whether the medium's scope is enumerable (`S(D)` computable).
    pub enumerable: bool,
}

/// A typed refusal from the scheduled full-enumeration walk: a source facet
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

/// The full-enumeration scheduling decision for a verify run. A closed,
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
    /// A full walk was **explicitly requested** ([`verify_binding_full`] —
    /// the CLI's `--full`), not schedule-triggered: the whole enumerable
    /// `S(D)` is walked, the sampling scheduler is bypassed, and the
    /// adjudication cap is treated as unlimited. Only ever constructed after
    /// the every-facet-enumerable gate, so it carries no per-facet refusal
    /// list — a non-enumerable facet refuses the entire run instead
    /// ([`FindingsError::FullWalkNonEnumerable`]).
    Forced {
        /// The facets the full enumeration walk covers.
        walked_facets: Vec<String>,
    },
}

impl FullResyncDecision {
    /// Whether this run performs a full-enumeration walk (a scheduled sweep
    /// is due, or an explicit full measurement was requested). `false` for
    /// `Disabled` / `NotDue`.
    pub fn is_full_walk(&self) -> bool {
        matches!(
            self,
            FullResyncDecision::Due { .. } | FullResyncDecision::Forced { .. }
        )
    }
}

/// Decide the `full_resync_every` scheduling outcome for a verify run —
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

/// The rotation item key a drift-adjudication candidate is selected under —
/// stable across runs for a given `(entity, artifact)` so the rotating window
/// covers a reproducible sequence.
fn candidate_key(entity: &str, anchor: &Anchor) -> String {
    format!("{entity}\u{1f}{}", anchor.artifact)
}

/// Adjudicate the hash-drift **candidates** under the per-run cap. Each
/// candidate is an anchor observation that hash-drift adjudication applies to
/// (a hash-bearing anchor in a `drifted` / `recheck` state). `window` is the
/// rotation-selected key set this run adjudicates; a candidate whose
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
            // Beyond the per-run cap: queue the remainder — it re-presents
            // in a later run's rotation window, so the whole candidate set
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

/// Stable identity of a finding's subject, class-independent — the unit the
/// head-durable merge ([`merge_with_prior`]) matches prior and fresh findings
/// on.
fn target_key(target: &FindingTarget) -> String {
    match target {
        FindingTarget::Anchor { entity, artifact } => format!("a\u{1f}{entity}\u{1f}{artifact}"),
        FindingTarget::Artifact { artifact } => format!("f\u{1f}{artifact}"),
        // Keyed on entity and artifact: a mention that moves between sections
        // is the same claim, and the section rides as description.
        FindingTarget::Mention {
            entity, artifact, ..
        } => format!("m\u{1f}{entity}\u{1f}{artifact}"),
    }
}

/// What one verify pass **observed** and what still exists — the inputs the
/// head-durable merge judges prior findings against.
struct PassObservation {
    /// Anchor targets ([`target_key`] form) whose live state this pass
    /// resolved (`Some(state)`).
    anchors_observed: BTreeSet<String>,
    /// Anchor targets still present in the mem's sidecar — any state,
    /// observed or not.
    anchors_existing: BTreeSet<String>,
    /// Artifact ids the coverage leg looked at this pass (the sample window,
    /// or the whole of `S(D)` on a full walk).
    files_observed: BTreeSet<String>,
    /// The binding's enumerable source set `S(D)`.
    s_d: BTreeSet<String>,
}

/// Merge this pass's fresh findings with the prior open batch — the write half
/// of head-durable findings (the store keys on `hash(D)` alone; see the module
/// docs).
///
/// A **re-observed** target's outcome is this pass's: a prior finding for it
/// is closed (observed clean — no fresh finding) or replaced (observed still
/// wrong — fresh finding wins). One exception keeps supersession honest: a
/// fresh `queued-for-adjudication` entry is a scheduling deferral, not an
/// observation, so it never downgrades a prior substantive adjudication —
/// a prior `drifted`/`wrong` verdict stands in its place.
///
/// An **unobserved** prior finding carries forward iff its subject is still
/// open:
/// - an anchor finding carries while its anchor still exists but was
///   unobservable this pass; a vanished anchor closes it;
/// - a coverage (artifact) finding carries while the artifact is still in
///   `S(D)` and is still unaccounted (`accounted_now`: no covering anchor
///   and no ledger exclusion); departure from `S(D)`, gained coverage, or a
///   recorded exclusion closes it — so an exclusion supersedes a standing
///   `uncovered` finding in the store itself, not only in the presentation
///   filter, and the verdict count cannot contradict the coverage section.
///
/// Carried findings keep their original [`Finding::key`] (the head they were
/// observed at). The carry rules are the growth bound: nothing is carried
/// whose subject left the source or re-adjudicated clean, so the open set
/// cannot grow without bound — and a closed/superseded finding is never
/// resurrected (it is simply absent from the recorded batch).
fn merge_with_prior(
    mut fresh: Vec<Finding>,
    prior: &[Finding],
    obs: &PassObservation,
    accounted_now: impl Fn(&str) -> bool,
) -> Vec<Finding> {
    let fresh_idx: BTreeMap<String, usize> = fresh
        .iter()
        .enumerate()
        .map(|(i, f)| (target_key(&f.target), i))
        .collect();
    let mut carried: Vec<Finding> = Vec::new();
    for f in prior {
        let tkey = target_key(&f.target);
        let observed = match &f.target {
            FindingTarget::Anchor { .. } => obs.anchors_observed.contains(&tkey),
            FindingTarget::Artifact { artifact } => obs.files_observed.contains(artifact),
            // The mention walk reads every destination body against the whole
            // of `S(D)` on every pass, so a mention is always re-observed: it
            // is either recorded afresh or gone.
            FindingTarget::Mention { .. } => true,
        };
        if observed {
            // Deferral must not supersede a substantive prior verdict.
            if matches!(f.class, FindingClass::Drifted | FindingClass::Wrong)
                && let Some(&i) = fresh_idx.get(&tkey)
                && fresh[i].class == FindingClass::QueuedForAdjudication
            {
                fresh[i] = f.clone();
            }
            continue;
        }
        if fresh_idx.contains_key(&tkey) {
            continue; // a fresh outcome exists for this target anyway
        }
        let still_open = match &f.target {
            FindingTarget::Anchor { .. } => obs.anchors_existing.contains(&tkey),
            FindingTarget::Artifact { artifact } => {
                obs.s_d.contains(artifact) && !accounted_now(artifact)
            }
            FindingTarget::Mention { .. } => false,
        };
        if still_open {
            carried.push(f.clone());
        }
    }
    fresh.extend(carried);
    fresh
}

/// The thin `projection verify` write path. Measures a binding's
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
    binding: &Binding,
    resolved: &ResolvedIngest,
) -> Result<VerifyOutcome, FindingsError> {
    run_verify(engine, workspace_root, binding, resolved, false)
}

/// [`verify_binding`]'s **full-measurement** mode (the CLI's `--full`):
/// enumerate the whole `S(D)` (the sampling scheduler is bypassed — the
/// rotation state is neither consulted nor advanced), treat the per-run
/// adjudication cap as unlimited, and observe every anchor — so the recorded
/// findings, and the tier-1 report computed over them, carry no
/// sampling/truncation caveat: coverage and accuracy are computed, not
/// sampled. The prepared-hash backfill worklist rides the outcome exactly as
/// on a sampled pass.
///
/// REFUSAL: a facet whose medium the capability matrix marks non-enumerable
/// refuses the **whole** run with the typed
/// [`FindingsError::FullWalkNonEnumerable`] — an explicit full measurement
/// promises complete figures, so a partial walk is never silently substituted
/// and a fabricated-complete report is never rendered. The sampled path
/// ([`verify_binding`]) is untouched by this mode's existence.
pub fn verify_binding_full(
    engine: &Engine,
    workspace_root: &Path,
    binding: &Binding,
    resolved: &ResolvedIngest,
) -> Result<VerifyOutcome, FindingsError> {
    run_verify(engine, workspace_root, binding, resolved, true)
}

/// The shared verify pass behind [`verify_binding`] (`full = false`, the
/// capped/sampled loop economics) and [`verify_binding_full`] (`full = true`,
/// the uncapped whole-`S(D)` measurement).
fn run_verify(
    engine: &Engine,
    workspace_root: &Path,
    binding: &Binding,
    resolved: &ResolvedIngest,
    full: bool,
) -> Result<VerifyOutcome, FindingsError> {
    let binding_id = resolved.name.clone();
    let (mem, name) = split_binding_id(&binding_id)?;

    // Full measurement requires every primary facet to be enumerable — refuse
    // the whole run typed before observing anything (never a fake-complete
    // report over a partially-walkable source).
    if full {
        for source in &resolved.sources {
            if let ResolvedSource::Primary(p) = source {
                let medium_type = medium_type_wire(p.medium_type);
                if !medium_capabilities(p.medium_type).enumerable {
                    return Err(FindingsError::FullWalkNonEnumerable(FullResyncRefusal {
                        facet: p.name.clone(),
                        medium_type: medium_type.clone(),
                        reason: format!(
                            "medium type '{medium_type}' is non-enumerable — a full-enumeration \
                             walk cannot cover it; the full measurement refuses rather than \
                             render a report with fabricated completeness"
                        ),
                    }));
                }
            }
        }

        // The matrix claiming enumerability is not evidence that a walk
        // happened. When a medium is declared enumerable but its walk yields
        // nothing, `--full` used to sail through the gate above and return
        // clean over a zero-artifact measurement — coverage 0/0, every anchor
        // unobserved, verdict green. That is the exact shape a full
        // measurement exists to make impossible, so refuse it.
        //
        // This guard survives the enumerator being fixed: it is the standing
        // check that a future medium cannot be added to the matrix as
        // enumerable without an enumeration arm and still report green.
        // Checked PER FACET. A binding-level union hides the mixed case: one
        // facet that walks makes the union non-empty, so `--full` returned
        // clean while a sibling enumerable facet was never walked at all —
        // complete coverage claimed over a scope nobody looked at. Each
        // enumerable facet must produce something of its own.
        for source in &resolved.sources {
            if let ResolvedSource::Primary(p) = source
                && medium_capabilities(p.medium_type).enumerable
            {
                let walked = super::cursor::enumerate_source_artifacts_reported(
                    engine,
                    p,
                    &resolved.deny_paths,
                    workspace_root,
                );
                let medium_type = medium_type_wire(p.medium_type);
                // A PARTIAL walk is the case the empty-check above cannot
                // see: some patterns resolved, so the facet is non-empty and
                // the gate waved it through, and `--full` then reported
                // complete coverage over a denominator missing whatever the
                // skipped patterns would have contributed. A full measurement
                // promises complete figures; a known-incomplete enumeration
                // cannot deliver one.
                if let Some(why) = walked.partiality_reason() {
                    return Err(FindingsError::FullWalkNonEnumerable(FullResyncRefusal {
                        facet: p.name.clone(),
                        medium_type: medium_type.clone(),
                        reason: format!(
                            "this facet's enumeration is incomplete — {why} — so a full \
                             measurement would claim complete coverage over a denominator \
                             that is not the population. Fix those patterns first"
                        ),
                    }));
                }
                if walked.files.is_empty() {
                    // The remedy text has to name the real cause. "Check that
                    // its scope patterns actually select something" is wrong
                    // advice when the patterns DO select artifacts and merely
                    // speak the retired workspace-relative dialect.
                    let remedy = if walked.legacy_dialect.is_empty() {
                        "Check that its scope patterns actually select something".to_string()
                    } else {
                        format!(
                            "its scope pattern(s) are still written against the workspace root \
                             rather than the source pointer ({}), so they select nothing under \
                             the pointer join — rewrite them relative to the pointer",
                            walked
                                .legacy_dialect
                                .iter()
                                .map(|n| n.pattern.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    return Err(FindingsError::FullWalkNonEnumerable(FullResyncRefusal {
                        facet: p.name.clone(),
                        medium_type: medium_type.clone(),
                        reason: format!(
                            "medium type '{medium_type}' claims to be enumerable, but this \
                             facet's enumeration yielded no artifacts — a full measurement over \
                             an empty walk would report complete coverage of nothing. {remedy}"
                        ),
                    }));
                }
            }
        }
    }

    // Refuse a vanished or unmounted path-based source before observing
    // anything: a missing tree would otherwise degrade to an empty
    // enumeration whose head token (the digest of nothing) masquerades as
    // a real observation — and the caller's completed-run baseline write
    // would clobber a genuine `#verified` token with it.
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source
            && matches!(
                p.medium_type,
                memstead_base::pipeline::MediumType::Codebase
                    | memstead_base::pipeline::MediumType::Filesystem
                    | memstead_base::pipeline::MediumType::Git
            )
        {
            let base = memstead_base::binding_run::source_base_path(p, workspace_root);
            // Unreachable is not only "absent". A directory that exists but
            // cannot be entered (permissions, a broken mount) enumerates
            // nothing, and the pass then reports every anchor unresolvable —
            // drift, in the verdict, blamed on a mem that did not move. The
            // read attempt is the test: existence alone let that through.
            // These mediums (codebase / filesystem / git) are all
            // directory-shaped — their scope globs enumerate under a tree —
            // so reachable means it IS a readable directory. A regular file
            // where the pointer promises a tree enumerates nothing and used
            // to slip through to be reported as drift, though the refusal
            // text already promised "present but not enumerable".
            let reachable = base.is_dir() && std::fs::read_dir(&base).is_ok();
            if !reachable {
                return Err(FindingsError::SourceUnreachable {
                    source_name: p.name.clone(),
                    path: base.display().to_string(),
                });
            }
        }
    }

    // The same refusal for a graph source, which needs it just as badly and
    // for a worse reason. A graph source's "tree" is a mounted mem; if that
    // mem is absent from the workspace, every entity anchor into it misses
    // the store and observes as ABSENT — a definite `orphaned`, not an
    // honest "unobserved". The pass would then report drift, tell the reader
    // to repoint or unset anchors that are perfectly fine, and — because
    // `orphaned` is the one state that satisfies prune's all-orphaned gate —
    // let prune propose deleting the destination entities. An unmounted mem
    // must never be indistinguishable from a deleted one.
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source
            && p.medium_type == memstead_base::pipeline::MediumType::Graph
            && !engine.mem_names().iter().any(|m| *m == p.pointer)
        {
            return Err(FindingsError::SourceUnreachable {
                source_name: p.name.clone(),
                path: format!("mem `{}` (not mounted in this workspace)", p.pointer),
            });
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

    // Tier-3 operations knobs (group D): the per-run adjudication cap, the
    // scheduled full-walk cadence, and the sample window size. All come off
    // the `verify` block, defaulting to the engine defaults (tuned on this project's own bindings) when it
    // is absent (verify has no mutating operation to gate — an absent block is
    // defaults, never a refusal).
    let verify_op = binding.operations.verify.as_ref();
    let cap = verify_op.map_or(DEFAULT_ADJUDICATION_CAP, |v| v.adjudication_cap);
    let full_resync_every = verify_op.map_or(DEFAULT_FULL_RESYNC_EVERY, |v| v.full_resync_every);
    let sample_batch = verify_op
        .map_or(resolved.batch_size, |v| v.batch_size)
        .max(1) as usize;

    // Level-trigger clock + full-resync schedule — the counter ticks every
    // run (even a non-enumerable one) so the schedule can refuse on time. An
    // explicit full measurement ticks the same clock (it is a verify run) but
    // its walk decision is `Forced`, not schedule-derived: the every-facet-
    // enumerable gate above already held, so no per-facet refusal list exists.
    let run_count = bump_verify_runs(&cache_root, &binding_id);
    let facet_enum: Vec<FacetEnumerability> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(FacetEnumerability {
                facet: p.name.clone(),
                medium_type: medium_type_wire(p.medium_type),
                enumerable: medium_capabilities(p.medium_type).enumerable,
            }),
            ResolvedSource::Reference { .. } => None,
        })
        .collect();
    let full_resync = if full {
        FullResyncDecision::Forced {
            walked_facets: facet_enum.iter().map(|f| f.facet.clone()).collect(),
        }
    } else {
        schedule_full_resync(full_resync_every, run_count, &facet_enum)
    };
    // A SCHEDULED due walk consults partiality the way `--full` does: the
    // scheduler branches on enumerability alone (it is pure and has no
    // filesystem), so a facet whose enumeration is known-incomplete — a
    // malformed or retired-dialect scope pattern — would be walked and
    // announced as full over a denominator that is not the population. Demote
    // such a facet into the typed refusal list instead, exactly where the
    // non-enumerable ones already land. The enumeration performed here is the
    // walk itself — its files feed the coverage pass below, so nothing is
    // enumerated twice. (`Forced` needs no demotion: the explicit-full gate
    // already refused the whole run on any partial facet.)
    let mut full_walk_files: Vec<String> = Vec::new();
    let full_resync = match full_resync {
        FullResyncDecision::Due {
            run_count,
            every,
            walked_facets,
            mut refused,
        } => {
            let mut kept: Vec<String> = Vec::new();
            for source in &resolved.sources {
                if let ResolvedSource::Primary(p) = source
                    && walked_facets.iter().any(|f| f == &p.name)
                {
                    let walked = super::cursor::enumerate_source_artifacts_reported(
                        engine,
                        p,
                        &resolved.deny_paths,
                        workspace_root,
                    );
                    if let Some(why) = walked.partiality_reason() {
                        refused.push(FullResyncRefusal {
                            facet: p.name.clone(),
                            medium_type: medium_type_wire(p.medium_type),
                            reason: format!(
                                "this facet's enumeration is incomplete — {why} — so the \
                                 scheduled full walk refuses it rather than announce complete \
                                 coverage over a denominator that is not the population"
                            ),
                        });
                    } else {
                        kept.push(p.name.clone());
                        full_walk_files.extend(walked.files);
                    }
                }
            }
            FullResyncDecision::Due {
                run_count,
                every,
                walked_facets: kept,
                refused,
            }
        }
        FullResyncDecision::Forced { walked_facets } => {
            for source in &resolved.sources {
                if let ResolvedSource::Primary(p) = source
                    && medium_capabilities(p.medium_type).enumerable
                {
                    full_walk_files.extend(enumerate_source_artifacts(
                        engine,
                        p,
                        &resolved.deny_paths,
                        workspace_root,
                    ));
                }
            }
            FullResyncDecision::Forced { walked_facets }
        }
        other => other,
    };

    let mut findings: Vec<Finding> = Vec::new();

    // 1. Adjudicate the destination mem's anchors against the live source, under
    //    the per-run cap with a rotating window. Existence failures
    //    (orphaned) are cheap and always reported; hash-drift candidates are
    //    bounded — the cap-sized rotation window is adjudicated, the remainder
    //    queued, and successive runs rotate the window so the whole anchor set is
    //    covered over a full rotation.
    let mut existence: Vec<(String, Anchor, AnchorState)> = Vec::new();
    let mut candidates: Vec<(String, Anchor, AnchorState)> = Vec::new();
    // First-observation backfill worklist: a hash-less hash-bearing anchor
    // whose artifact resolved and yielded a prepared-content hash is not a
    // drift candidate (there is no recorded hash to compare — recorded ==
    // observed by construction once the backfill lands); it resolves clean
    // this pass and the observed hash rides the outcome for the caller's
    // [`record_anchor_hash_backfill`] write. From the next pass on the
    // anchor adjudicates deterministically — the recheck queue drains
    // instead of re-queueing forever.
    let mut hash_backfill: Vec<ObservedArtifactHash> = Vec::new();
    let mut backfill_seen: BTreeSet<(String, String)> = BTreeSet::new();
    // Observation bookkeeping for the head-durable merge: which anchor
    // targets exist, and which of them this pass actually resolved.
    let mut anchors_existing: BTreeSet<String> = BTreeSet::new();
    let mut anchors_observed: BTreeSet<String> = BTreeSet::new();
    // Scoped to this binding's population (consistency-sweep 03/01). An
    // excluded anchor must never raise a finding against a binding that did
    // not write it or has disclaimed the file; the report names the exclusions.
    let population = crate::anchor_population::population_for(
        engine,
        resolved,
        Some(binding_hash_of(binding, resolved).as_str()),
    );
    for (eid, resolved_anchor) in population.included {
        let tkey = target_key(&FindingTarget::Anchor {
            entity: eid.as_ref().to_string(),
            artifact: resolved_anchor.anchor.artifact.clone(),
        });
        anchors_existing.insert(tkey.clone());
        let Some(state) = resolved_anchor.state else {
            continue;
        };
        anchors_observed.insert(tkey);
        let observed_hash = resolved_anchor.observed_hash;
        let anchor = resolved_anchor.anchor;
        match state {
            AnchorState::Resolves => {}
            AnchorState::Orphaned => existence.push((eid.as_ref().to_string(), anchor, state)),
            AnchorState::Drifted | AnchorState::Recheck => {
                // Only hash-bearing anchors are hash-drift candidates (A2); a
                // non-hash-bearing class yields no adjudication.
                if !anchor.class.is_hash_bearing() {
                    continue;
                }
                if anchor.hash.is_none()
                    && let Some(hash) = observed_hash
                {
                    // First observation of a hash-less anchor on a resolvable
                    // artifact: backfill, not adjudication.
                    if backfill_seen.insert((eid.as_ref().to_string(), anchor.artifact.clone())) {
                        hash_backfill.push(ObservedArtifactHash {
                            entity: eid.as_ref().to_string(),
                            artifact: anchor.artifact.clone(),
                            hash,
                        });
                    }
                    continue;
                }
                candidates.push((eid.as_ref().to_string(), anchor, state));
            }
        }
    }
    for (entity, anchor, state) in &existence {
        if let Some(f) = adjudicate_anchor(&key, &facet, entity, anchor, *state, &now) {
            findings.push(f);
        }
    }
    // `cap == 0` disables the cap (adjudicate every candidate), and a full
    // measurement treats any configured cap as unlimited — its rotation state
    // is neither consulted nor advanced (the scheduler is bypassed, so the
    // sampled loop's window sequence is untouched by a full run). Otherwise a
    // cap-sized rotation window selects this run's adjudicated set (D1/D2).
    let window: Option<BTreeSet<String>> = if full || cap == 0 {
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
    // or explicitly requested (`Forced`), enumerate the WHOLE source of
    //    every enumerable facet — guaranteeing complete coverage this run;
    //    otherwise sample a bounded rotating window. Non-enumerable facets
    //    are refused (scheduled: the typed refusal rides on `full_resync`;
    //    explicit: the whole run refused before observing), never silently
    //    claimed as covered.
    // `S(D)` — the in-scope population under the binding as it is declared
    // NOW (deny_paths applied). Computed before the sample is drawn so the
    // window can be held to it: the rotation scheduler reconciles its
    // in-flight order against this set, and the retain below is the second
    // wall for the same invariant, a recorded `uncovered` finding names an
    // artifact of `S(D)` and nothing else.
    let mut s_d: BTreeSet<String> = BTreeSet::new();
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source
            && medium_capabilities(p.medium_type).enumerable
        {
            s_d.extend(enumerate_source_artifacts(
                engine,
                p,
                &resolved.deny_paths,
                workspace_root,
            ));
        }
    }
    let mut sample_files: Vec<String> = if full_resync.is_full_walk() {
        // Collected above where the walk decision was settled — only facets
        // the decision actually announces as walked contribute.
        let mut all = full_walk_files;
        all.sort();
        all.dedup();
        all
    } else {
        next_batch(engine, resolved, workspace_root, &cache_root, sample_batch)
            .map(|b| b.files)
            .unwrap_or_default()
    };
    sample_files.retain(|f| s_d.contains(f));
    // Filtered by BINDING, not merely by mem.
    // The report's coverage lookup was scoped first and this one
    // was missed, which is the worse of the two: this decides whether an
    // `Uncovered` finding is RECORDED and whether a prior one stays open, so a
    // mem filter here let another binding's anchor mark a file covered in the
    // durable store. An anchor with no recorded binding still counts, by the
    // same pre-provenance fallback the population uses.
    let this_binding = binding_hash_of(binding, resolved);
    // An anchor whose ENTITY is gone covers nothing,
    // guarded on the reconciliation having been possible at all so an
    // unreconcilable mem keeps its coverage rather than reading as wholly
    // uncovered.
    let entity_end_reconciled = engine
        .entity_set_is_reconcilable(&resolved.destination_mem)
        .is_ok();
    let covered_now = |artifact: &str| {
        engine
            .anchors_referencing_artifact(artifact)
            .iter()
            .any(|(eid, a)| {
                eid.mem() == resolved.destination_mem.as_str()
                    && a.binding
                        .as_deref()
                        .map(|b| b == this_binding.as_str())
                        .unwrap_or(true)
                    && (!entity_end_reconciled || !engine.entity_is_absent(eid))
            })
    };
    // The durable authored-exclusion ledger (B4) gates the RECORDING, not
    // only the report's decoration: an artifact mined and deliberately
    // excluded with a rationale is not an uncovered finding. Until
    // 2026-08-28 only the report body consulted the ledger, so the verdict
    // line and the findings store kept counting exclusions as uncovered
    // (three of them on plugin/graph) while the rationales rendered right
    // beside the count.
    // Reconciled first (A3 AC3): an exclusion whose source left the
    // declaration is dropped here as well, not only when a brief renders.
    let excluded: BTreeSet<String> =
        crate::advance::reconcile_exclusions(engine, workspace_root, resolved)
            .map(|l| l.active.into_iter().map(|e| e.artifact).collect())
            .unwrap_or_default();
    for file in &sample_files {
        if !covered_now(file) && !excluded.contains(file) {
            findings.push(Finding {
                key: key.clone(),
                facet: facet.clone(),
                target: FindingTarget::Artifact {
                    artifact: file.clone(),
                },
                class: FindingClass::Uncovered,
                detail: "source artifact in scope has no anchor in the destination mem".to_string(),
                created_at: now.clone(),
            });
        }
    }

    // 2b. Claims about artifacts an entity does not anchor. The same walk the
    //     coverage leg makes over anchors, made over entity bodies: every
    //     destination entity whose prose names an artifact of `S(D)` and
    //     carries no anchor on it. Always the whole of `S(D)` — the walk is
    //     linear in body size (path tokens are looked up, artifacts are never
    //     searched for) — so a sampled pass observes mentions exactly as a
    //     full one does. An artifact the exclusion ledger names raises none:
    //     the author has said it warrants no entity, and a mention of it is a
    //     neighbour named for contrast, not a claim owed an anchor.
    findings.extend(unanchored_mention_findings(
        engine, resolved, &s_d, &excluded, &key, &facet, &now,
    ));

    // 3. Head-durable merge (the store keys on hash(D) alone): fold the prior
    //    open batch into this pass's findings — re-observed targets take this
    //    pass's outcome; unobserved-but-still-open ones carry forward with
    //    their original observed head; departed/covered/vanished subjects
    //    close. Sync briefs thus keep presenting an open finding across
    //    source-head movement until a pass observes it clean.
    let mut store = read_findings_store(workspace_root, &mem, &name)
        .map_err(FindingsError::Store)?
        .unwrap_or_else(|| FindingsStore {
            binding: binding_id.clone(),
            ..Default::default()
        });
    let obs = PassObservation {
        anchors_observed,
        anchors_existing,
        files_observed: sample_files.into_iter().collect(),
        s_d,
    };
    let prior = store.current(&key).to_vec();
    // The merge's accounting closure folds the exclusion ledger in: an
    // artifact that gained an authored exclusion since its `uncovered`
    // finding was recorded is accounted for, so the stale finding closes in
    // the store instead of being carried forward and merely hidden by the
    // presentation filter.
    let findings = merge_with_prior(findings, &prior, &obs, |artifact: &str| {
        covered_now(artifact) || excluded.contains(artifact)
    });

    let backlog = findings
        .iter()
        .filter(|f| f.class == FindingClass::QueuedForAdjudication)
        .count();

    // Record under the current key (prior-hash batches retained, segregated —
    // A3), persist to the durable state tier (A1).
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
        hash_backfill,
    })
}

/// The unanchored-mention walk: one finding per `(entity, artifact)` for every
/// destination entity whose body names an `S(D)` artifact it carries no anchor
/// on. Prose only — fenced code blocks are masked before scanning (an inline
/// code span is the ordinary spelling of a path and stays visible). A path
/// token matches an artifact under either of the binding's spellings: the
/// workspace-relative id `S(D)` enumerates, or the source-relative form the
/// facet's pointer joins onto it (the same rule anchors resolve by). Artifacts
/// in `excluded` raise nothing. "Carries no anchor" is decided by the engine's
/// own reference rule ([`Engine::anchors_referencing_artifact`]), so a tree
/// anchor over the file's directory anchors it.
fn unanchored_mention_findings(
    engine: &Engine,
    resolved: &ResolvedIngest,
    s_d: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
    key: &FindingKey,
    facet: &str,
    now: &str,
) -> Vec<Finding> {
    if s_d.is_empty() {
        return Vec::new();
    }
    let pointers: Vec<String> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(p.pointer.clone()),
            ResolvedSource::Reference { .. } => None,
        })
        .collect();
    // (entity, artifact) → sections naming it, in body order.
    let mut mentions: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for entity in engine.store().all_entities() {
        if entity.mem != resolved.destination_mem || entity.stub {
            continue;
        }
        for (section, body) in &entity.sections {
            let masked = memstead_base::markdown::mask_code_blocks(body);
            for token in path_tokens(&masked) {
                let Some(artifact) = resolve_mention(&token, &pointers, s_d) else {
                    continue;
                };
                if excluded.contains(&artifact) {
                    continue;
                }
                let sections = mentions
                    .entry((entity.id.to_string(), artifact))
                    .or_default();
                if !sections.iter().any(|s| s == section) {
                    sections.push(section.clone());
                }
            }
        }
    }
    if mentions.is_empty() {
        return Vec::new();
    }
    // One sidecar read per distinct artifact, not per mention.
    let mut anchored_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, artifact) in mentions.keys() {
        anchored_by.entry(artifact.clone()).or_insert_with(|| {
            engine
                .anchors_referencing_artifact(artifact)
                .into_iter()
                .map(|(eid, _)| eid.as_ref().to_string())
                .collect()
        });
    }
    mentions
        .into_iter()
        .filter(|((entity, artifact), _)| {
            !anchored_by
                .get(artifact.as_str())
                .is_some_and(|holders| holders.contains(entity))
        })
        .map(|((entity, artifact), sections)| Finding {
            key: key.clone(),
            facet: facet.to_string(),
            detail: format!(
                "entity names this in-scope artifact in section{} {} and carries no anchor on \
                 it; add the anchor (`memstead_update` with `anchors`) so verify watches the \
                 claim, or exclude the artifact with a rationale (`projection exclude`)",
                if sections.len() == 1 { "" } else { "s" },
                sections
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            target: FindingTarget::Mention {
                entity,
                artifact,
                section: sections[0].clone(),
            },
            class: FindingClass::UnanchoredMention,
            created_at: now.to_string(),
        })
        .collect()
}

/// Path-shaped tokens of a prose body: maximal runs of path characters
/// (alphanumerics, `_`, `.`, `/`, `-`) that carry a `/` or a `.`, with a
/// leading `./` and trailing sentence punctuation stripped. Lexical and
/// deterministic; whether a token names an artifact is decided by lookup
/// against `S(D)`, never by guessing.
pub(crate) fn path_tokens(text: &str) -> Vec<String> {
    let is_path_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-');
    let mut out = Vec::new();
    for run in text.split(|c: char| !is_path_char(c)) {
        let mut t = run.trim_end_matches(['.', '-']);
        while let Some(rest) = t.strip_prefix("./") {
            t = rest;
        }
        if t.len() < 3 || !(t.contains('/') || t.contains('.')) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// Which `S(D)` artifact a path token names, if any: the token as written
/// (the workspace-relative id), or the token joined under a facet pointer
/// (the source-relative spelling) — the same two readings the anchor
/// resolver accepts, so one file has one identity however it is spelt.
pub(crate) fn resolve_mention(
    token: &str,
    pointers: &[String],
    s_d: &BTreeSet<String>,
) -> Option<String> {
    if s_d.contains(token) {
        return Some(token.to_string());
    }
    for pointer in pointers {
        for candidate in memstead_base::engine::query::artifact_candidates(pointer, token) {
            if s_d.contains(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// The symbols a prose body names in inline code spans: the content of every
/// single-backtick span of a body whose fenced blocks are masked, split into
/// its path segments (`Type::variant`, `module.function`) with a trailing
/// call or macro marker (`()`, `!`) dropped, so `` `advance_baseline()` `` and
/// `` `AdvanceError::UnknownArtifact` `` each name what they spell. Lexical
/// and deterministic; whether a symbol matters is decided by lookup against
/// the change's defined-or-removed set, never by guessing.
pub(crate) fn code_span_symbols(text: &str) -> Vec<String> {
    let masked = memstead_base::markdown::mask_code_blocks(text);
    let mut out = Vec::new();
    let mut rest = masked.as_str();
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        let span = &after[..close];
        if !span.is_empty() && !span.contains('\n') {
            for seg in span.split("::").flat_map(|s| s.split('.')) {
                let seg = seg
                    .trim()
                    .trim_end_matches("()")
                    .trim_end_matches('!')
                    .trim_end_matches("()");
                let is_ident = !seg.is_empty()
                    && seg.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !seg.chars().next().is_some_and(|c| c.is_ascii_digit());
                if is_ident && !out.iter().any(|o| o == seg) {
                    out.push(seg.to_string());
                }
            }
        }
        rest = &after[close + 1..];
    }
    out
}

/// The medium type's wire string (`codebase` / `web` / …) — the serde form the
/// capability matrix and reports use.
fn medium_type_wire(t: memstead_base::pipeline::MediumType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The `unanchored-mention` findings of every binding's current batch,
/// as health warnings — the loop's contribution to the health report,
/// handed to the kernel composer as data by [`super::health::compose_health`]. Empty without a workspace root (no binding store
/// to read) and for a binding whose store or resolution is unreadable —
/// health never fabricates a projection reading it did not find.
pub fn unanchored_mention_warnings(engine: &Engine) -> Vec<WarningHint> {
    let Some(root) = engine.workspace_root() else {
        return Vec::new();
    };
    let Ok(configs) = memstead_base::pipeline_store::load_pipeline_configs(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for record in &configs.bindings {
        let binding_id = format!("{}/{}", record.mem, record.name);
        let Ok(resolved) =
            memstead_base::binding_run::resolve_binding_run(&binding_id, &record.config)
        else {
            continue;
        };
        let Ok((_, findings)) = current_findings(engine, root, &record.config, &resolved) else {
            continue;
        };
        for f in findings {
            if let FindingTarget::Mention {
                entity,
                artifact,
                section,
            } = f.target
            {
                out.push(WarningHint::UnanchoredMention {
                    mem: record.config.destination_mem.clone(),
                    binding: binding_id.clone(),
                    entity: EntityId(entity),
                    artifact,
                    section,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
