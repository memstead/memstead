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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Engine;
use crate::anchor::{Anchor, AnchorState};
use crate::binding::{BindingV1, ResolvedBinding, hash_binding};
use crate::workspace_store::{StoreError, WORKSPACE_STORE_DIR};

use super::advance::is_single_component;
use super::cursor::compute_source_cursor;
use super::refinement::next_batch;
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

/// Persist the durable findings store for a binding (pretty JSON), creating
/// parent directories.
pub fn write_findings_store(
    workspace_root: &Path,
    mem: &str,
    name: &str,
    store: &FindingsStore,
) -> Result<(), StoreError> {
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

/// The composite current source-head token: each source facet's current
/// baseline token, joined deterministically. Starts from the destination mem's
/// recorded `#synced` tokens for the binding, then overlays the cursor's
/// current-head tokens for any facet that has moved or is newly seen — so the
/// value reflects the source's current state and changes iff any facet's head
/// changes (the A3 "source head moved" trigger).
fn current_source_head(
    engine: &Engine,
    workspace_root: &Path,
    resolved: &ResolvedIngest,
) -> String {
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
        .iter()
        .map(|(facet, token)| format!("{facet}={token}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// The current recording key for a binding: `(hash(D), source_head)`.
fn current_key(
    engine: &Engine,
    workspace_root: &Path,
    binding: &BindingV1,
    resolved: &ResolvedIngest,
) -> FindingKey {
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
    FindingKey {
        binding_hash: hash_binding(&rb),
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

    let key = current_key(engine, workspace_root, binding, resolved);
    let now = now_seconds();
    let facet = source_facet_label(resolved);

    let mut findings: Vec<Finding> = Vec::new();

    // 1. Adjudicate the destination mem's anchors against the live source.
    for (eid, resolved_anchor) in engine.mem_anchors_resolved(&resolved.destination_mem) {
        if let Some(state) = resolved_anchor.state
            && let Some(finding) = adjudicate_anchor(
                &key,
                &facet,
                eid.as_ref(),
                &resolved_anchor.anchor,
                state,
                &now,
            )
        {
            findings.push(finding);
        }
    }

    // 2. Sample in-scope source artifacts via the retained rotation machinery
    //    (scheduling only) and flag those with no anchor in the destination mem.
    let cache_root = workspace_root.join(".memstead.cache").join("ingest");
    if let Some(batch) = next_batch(resolved, workspace_root, &cache_root) {
        for file in batch.files {
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
                    detail: "source artifact in scope has no anchor in the destination mem"
                        .to_string(),
                    created_at: now.clone(),
                });
            }
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
    })
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
        BINDING_VERSION, BuildMode, BuildOperation, CoverageSemantics, Operations, VerifyOperation,
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
}
