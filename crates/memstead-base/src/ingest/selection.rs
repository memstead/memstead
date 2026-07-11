//! Ingest selection + backoff — pick the next *due* **(binding, operation)
//! pair** in a `--all` rotation, skipping pairs whose destination is unchanged
//! and whose sources have not moved. Engine-side generalization of the
//! plugin's `nextIngest` / `shouldSkip` / backoff state from "next due binding
//! (build)" to a per-operation rotation.
//!
//! **Eligibility** (per pair): an operation participates in the rotation only
//! when its `operations.<op>` block exists in the binding **and** declares
//! `trigger: loop` — consent to unattended rotation lives in the declaration.
//! A one-shot build that already ran stays excluded.
//!
//! **Due-checks** (cheap, per pair, before backoff): build is always due
//! (unchanged semantics — backoff alone decides); sync is due when a source
//! moved past its `#synced` baseline **or** open findings exist under the
//! binding's current `(hash(D), source_head)` key; verify is due when a source
//! moved past its `#verified` baseline (a never-verified source with a live
//! token counts as moved — the first verify is due). A pair that is not due is
//! passed over without touching its backoff state.
//!
//! The deterministic state (round-robin cursor, per-pair backoff, one-shot
//! ran-set) lives engine-side under `<workspace>/.memstead.cache/ingest/` —
//! the same engine-internal bookkeeping location the mtime memo uses. This is
//! not mem-repo / graph state; selection mutates it as its job. Cursor and
//! backoff entries are keyed by the **pair id** `<binding>#<op>`; pre-pair
//! single-key entries (plain binding ids) are **discarded** — the cache is
//! disposable, and the cost is at most one lost backoff step per binding.
//!
//! Backoff shape (mirrors the plugin exactly): a **linear-ramp** per-pair
//! skip counter. A destination-snapshot change *or* a moved source resets it
//! to zero and runs; otherwise each unproductive pass grows the cooldown by
//! one (capped at [`MAX_SKIP_LEVEL`]). A one-shot build never skips. For sync
//! / verify pairs the moved-source override is **not** applied: the due-check
//! already encodes source movement, and a productive run mutates the
//! destination mem, which resets the pair's backoff by itself — so an
//! un-acted-on brief ramps instead of being re-rendered every pass.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Engine;
use crate::binding::{BindingV1, BuildMode};
use crate::pipeline::IngestTrigger;
use crate::pipeline_store::BindingConfigs;

use super::cursor::{source_moved, source_moved_since};
use super::findings::current_findings;
use super::resolve::{ResolvedIngest, resolve_binding_run};

/// The backoff cooldown ceiling — after this many consecutive unproductive
/// passes the skip count stops growing. Mirrors the plugin's `MAX_SKIP_LEVEL`.
pub const MAX_SKIP_LEVEL: u32 = 10;

/// One operation of a binding — the second half of a `--all` rotation pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    /// The build operation (grow coverage / one-shot lens).
    Build,
    /// The sync operation (the sole maintenance writer).
    Sync,
    /// The verify operation (read-only measurement).
    Verify,
}

impl OperationKind {
    /// Every kind, in rotation-sort order (build < sync < verify).
    pub const ALL: [OperationKind; 3] = [
        OperationKind::Build,
        OperationKind::Sync,
        OperationKind::Verify,
    ];

    /// Stable wire form (`build` / `sync` / `verify`).
    pub fn as_wire(&self) -> &'static str {
        match self {
            OperationKind::Build => "build",
            OperationKind::Sync => "sync",
            OperationKind::Verify => "verify",
        }
    }
}

/// Which operations a `--all` rotation considers. `Only(op)` restricts the
/// eligible set to that operation's pairs (the CLI default is
/// `Only(Build)` — byte-stable for the ingest router); [`Self::Any`] rotates
/// across every eligible pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationFilter {
    /// Rotate over a single operation's pairs.
    Only(OperationKind),
    /// Rotate over every eligible (binding, operation) pair.
    Any,
}

impl OperationFilter {
    fn admits(self, op: OperationKind) -> bool {
        match self {
            OperationFilter::Only(only) => only == op,
            OperationFilter::Any => true,
        }
    }
}

/// The cache key of a rotation pair: `<binding>#<op>` (e.g.
/// `engine/graph#build`). Cursor and backoff state are keyed on this.
fn pair_key(binding_id: &str, op: OperationKind) -> String {
    format!("{binding_id}#{}", op.as_wire())
}

/// Per-ingest destination-snapshot backoff state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackoffEntry {
    /// Passes still to skip before the next run.
    #[serde(default)]
    pub skip_remaining: u32,
    /// Current cooldown level (grows by one per unproductive pass, capped).
    #[serde(default)]
    pub skip_level: u32,
    /// The destination snapshot token this entry was last evaluated against.
    #[serde(default)]
    pub snapshot: String,
}

/// The round-robin cursor — the (binding, operation) pair the last rotation
/// advanced to, stored as the pair id `<binding>#<op>`. A pre-pair value (a
/// plain binding id) never matches a pair id, so the first op-aware pass
/// simply restarts the rotation from the top — the cache is disposable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// The last pair id the cursor advanced to (`None` before the first pass).
    #[serde(default)]
    pub last: Option<String>,
}

/// Apply the destination-snapshot backoff to `entry`, mutating it, and return
/// whether to **skip** this pass. `current` is the destination mem's current
/// snapshot token (empty when none). Mirrors the backoff block of the plugin's
/// `shouldSkip`:
///
///   - destination moved (`snapshot` set and `current` differs) → reset to
///     zero, store `current`, **run**;
///   - `skip_remaining > 0` → decrement, **skip**;
///   - otherwise → if the snapshot is unchanged ramp the level (capped), set
///     `skip_remaining = skip_level`, store `current`, **run**.
pub fn apply_backoff(entry: &mut BackoffEntry, current: &str) -> bool {
    if !entry.snapshot.is_empty() && current != entry.snapshot {
        entry.skip_remaining = 0;
        entry.skip_level = 0;
        entry.snapshot = current.to_string();
        return false;
    }
    if entry.skip_remaining > 0 {
        entry.skip_remaining -= 1;
        return true;
    }
    if !entry.snapshot.is_empty() && current == entry.snapshot {
        entry.skip_level = (entry.skip_level + 1).min(MAX_SKIP_LEVEL);
        entry.skip_remaining = entry.skip_level;
    }
    entry.snapshot = current.to_string();
    false
}

/// Whether a binding should be skipped this rotation. A one-shot build never
/// skips (one-shots are excluded from the eligible set once run). Discovery: a
/// moved source overrides backoff; otherwise the destination-snapshot
/// [`apply_backoff`].
pub fn should_skip(
    mode: BuildMode,
    source_moved: bool,
    entry: &mut BackoffEntry,
    current: &str,
) -> bool {
    match mode {
        BuildMode::OneShot => return false,
        BuildMode::Discovery => {}
    }
    if source_moved {
        return false;
    }
    apply_backoff(entry, current)
}

// ── state files (engine-internal cache) ─────────────────────────────────────

fn read_json<T: Default + for<'de> Deserialize<'de>>(cache_root: &Path, name: &str) -> T {
    std::fs::read(cache_root.join(name))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(cache_root: &Path, name: &str, value: &T) {
    let _ = std::fs::create_dir_all(cache_root);
    if let Ok(bytes) = serde_json::to_vec(value) {
        let _ = std::fs::write(cache_root.join(name), bytes);
    }
}

/// Read the set of one-shot ingests that have already run.
fn read_one_shot_runs(cache_root: &Path) -> BTreeSet<String> {
    let map: BTreeMap<String, bool> = read_json(cache_root, "ingest-one-shot-runs.json");
    map.into_iter()
        .filter(|(_, v)| *v)
        .map(|(k, _)| k)
        .collect()
}

/// Select the next *due* ingest (build operation) for a `--all` rotation —
/// the build-only compatibility form of [`select_next_due_operation`].
/// Returns the selected binding id, or `None` when nothing is due this pass.
pub fn select_next_due(
    engine: &Engine,
    workspace_root: &Path,
    configs: &BindingConfigs,
) -> Option<String> {
    select_next_due_operation(
        engine,
        workspace_root,
        configs,
        OperationFilter::Only(OperationKind::Build),
    )
    .map(|(name, _)| name)
}

/// One eligible rotation pair: a resolved binding run plus the operation.
struct Pair<'a> {
    /// The pair id `<binding>#<op>` — the cursor/backoff cache key.
    key: String,
    /// The resolved run (its `name` is the canonical binding id).
    ingest: ResolvedIngest,
    /// The stored binding declaration (the findings due-check needs it).
    binding: &'a BindingV1,
    /// The operation half of the pair.
    op: OperationKind,
}

/// Whether an operation block exists on `binding` **and** declares
/// `trigger: loop` — the pair-eligibility gate. Consent to unattended `--all`
/// rotation lives in the declaration: a `manual` / `on-event` operation never
/// rotates, whatever the filter asks for.
fn declared_for_loop(binding: &BindingV1, op: OperationKind) -> bool {
    match op {
        OperationKind::Build => binding
            .operations
            .build
            .as_ref()
            .is_some_and(|b| b.trigger == IngestTrigger::Loop),
        OperationKind::Sync => binding
            .operations
            .sync
            .as_ref()
            .is_some_and(|s| s.trigger == IngestTrigger::Loop),
        OperationKind::Verify => binding
            .operations
            .verify
            .as_ref()
            .is_some_and(|v| v.trigger == IngestTrigger::Loop),
    }
}

/// The cheap per-operation due-check, evaluated before backoff. Build is
/// always due (unchanged semantics — backoff alone decides). Sync is due when
/// a source moved past its `#synced` baseline or open findings exist under
/// the binding's current `(hash(D), source_head)` key (the same read the sync
/// brief consumes — an unreadable findings store contributes nothing here;
/// the source-moved clause still fires, and the brief render surfaces the
/// store error). Verify is due when a source moved past its `#verified`
/// baseline, with a never-verified source counting as moved (the first
/// verify is due).
fn operation_due(engine: &Engine, workspace_root: &Path, pair: &Pair<'_>) -> bool {
    match pair.op {
        OperationKind::Build => true,
        OperationKind::Sync => {
            source_moved(engine, &pair.ingest, workspace_root)
                || current_findings(engine, workspace_root, pair.binding, &pair.ingest)
                    .map(|(_key, findings)| !findings.is_empty())
                    .unwrap_or(false)
        }
        OperationKind::Verify => {
            source_moved_since(engine, &pair.ingest, workspace_root, "verified", true)
        }
    }
}

/// Select the next *due* (binding, operation) pair for a `--all` rotation,
/// advancing the round-robin cursor and the per-pair backoff state. Returns
/// the selected binding id and operation, or `None` when nothing eligible is
/// due (or everything due is backing off) this pass.
pub fn select_next_due_operation(
    engine: &Engine,
    workspace_root: &Path,
    configs: &BindingConfigs,
    filter: OperationFilter,
) -> Option<(String, OperationKind)> {
    let cache_root = workspace_root.join(".memstead.cache").join("ingest");

    // Eligible = every (resolvable binding, loop-declared operation) pair the
    // filter admits, minus one-shot builds that already ran. Pair keys derive
    // from the canonical binding id (`<mem>/<stem>`, D3/D9) — the resolved
    // run's `name`.
    let one_shot_ran = read_one_shot_runs(&cache_root);
    let mut eligible: Vec<Pair<'_>> = Vec::new();
    for record in &configs.bindings {
        let binding_id = format!("{}/{}", record.mem, record.name);
        let Ok(ingest) = resolve_binding_run(configs, &binding_id, &record.config) else {
            continue;
        };
        for op in OperationKind::ALL {
            if !filter.admits(op) || !declared_for_loop(&record.config, op) {
                continue;
            }
            if op == OperationKind::Build
                && ingest.mode == BuildMode::OneShot
                && one_shot_ran.contains(&ingest.name)
            {
                continue;
            }
            eligible.push(Pair {
                key: pair_key(&ingest.name, op),
                ingest: ingest.clone(),
                binding: &record.config,
                op,
            });
        }
    }
    eligible.sort_by(|a, b| a.key.cmp(&b.key));
    let n = eligible.len();
    if n == 0 {
        return None;
    }

    // Advance the round-robin cursor by one from the last-picked position.
    let mut cursor: Cursor = read_json(&cache_root, "ingest-cursor.json");
    let start = cursor
        .last
        .as_ref()
        .and_then(|last| eligible.iter().position(|p| &p.key == last))
        .map_or(0, |i| (i + 1) % n);
    cursor.last = Some(eligible[start].key.clone());
    write_json(&cache_root, "ingest-cursor.json", &cursor);

    // From the start, take the first pair that is due and not backing off.
    // Pre-pair single-key backoff entries (no `#<op>` suffix) are discarded on
    // the way through — disposable cache, at most one lost backoff step.
    let mut backoff: BTreeMap<String, BackoffEntry> = read_json(&cache_root, "ingest-backoff.json");
    backoff.retain(|k, _| k.contains('#'));
    let mut selected = None;
    for offset in 0..n {
        let pair = &eligible[(start + offset) % n];
        if !operation_due(engine, workspace_root, pair) {
            continue;
        }
        let current = engine
            .mem_head_sha(&pair.ingest.destination_mem)
            .ok()
            .flatten()
            .unwrap_or_default();
        // Build keeps the moved-source backoff override (and the one-shot
        // never-skips rule). Sync / verify pairs rely on the due-check for
        // source movement and on the destination-snapshot reset for
        // productivity, so an un-acted-on brief ramps instead of re-rendering
        // every pass.
        let (mode, moved) = match pair.op {
            OperationKind::Build => (
                pair.ingest.mode,
                source_moved(engine, &pair.ingest, workspace_root),
            ),
            OperationKind::Sync | OperationKind::Verify => (BuildMode::Discovery, false),
        };
        let entry = backoff.entry(pair.key.clone()).or_default();
        if !should_skip(mode, moved, entry, &current) {
            selected = Some((pair.ingest.name.clone(), pair.op));
            break;
        }
    }
    write_json(&cache_root, "ingest-backoff.json", &backoff);
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The linear-ramp backoff: first pass at a fresh snapshot runs and stores
    /// it; a repeat (unchanged) ramps the cooldown and skips it down; a
    /// destination change resets to zero and runs immediately.
    #[test]
    fn backoff_ramps_and_resets() {
        let mut e = BackoffEntry::default();

        // First evaluation: empty snapshot → runs, stores current.
        assert!(!apply_backoff(&mut e, "sha1"));
        assert_eq!(e.snapshot, "sha1");
        assert_eq!(e.skip_level, 0);

        // Unchanged again → ramp to level 1, skip_remaining 1, runs this pass.
        assert!(!apply_backoff(&mut e, "sha1"));
        assert_eq!(e.skip_level, 1);
        assert_eq!(e.skip_remaining, 1);

        // Next pass: skip_remaining 1 → skip, decrement to 0.
        assert!(apply_backoff(&mut e, "sha1"));
        assert_eq!(e.skip_remaining, 0);

        // Next: remaining 0, unchanged → ramp to 2, runs.
        assert!(!apply_backoff(&mut e, "sha1"));
        assert_eq!(e.skip_level, 2);
        assert_eq!(e.skip_remaining, 2);

        // A destination change resets everything and runs immediately.
        assert!(!apply_backoff(&mut e, "sha2"));
        assert_eq!(e.skip_level, 0);
        assert_eq!(e.skip_remaining, 0);
        assert_eq!(e.snapshot, "sha2");
    }

    /// The ramp is capped at MAX_SKIP_LEVEL.
    #[test]
    fn backoff_caps_at_max_level() {
        let mut e = BackoffEntry {
            skip_level: MAX_SKIP_LEVEL,
            skip_remaining: 0,
            snapshot: "s".to_string(),
        };
        assert!(!apply_backoff(&mut e, "s")); // unchanged, remaining 0 → ramp
        assert_eq!(e.skip_level, MAX_SKIP_LEVEL, "capped");
        assert_eq!(e.skip_remaining, MAX_SKIP_LEVEL);
    }

    /// A one-shot build never skips; a moved source overrides backoff for
    /// discovery; an unchanged discovery destination backs off.
    #[test]
    fn should_skip_honours_mode_and_source_movement() {
        let mut e = BackoffEntry {
            skip_remaining: 3,
            skip_level: 3,
            snapshot: "s".to_string(),
        };
        // A one-shot build never skips, regardless of backoff.
        assert!(!should_skip(BuildMode::OneShot, false, &mut e.clone(), "s"));
        // Discovery with a moved source → run (backoff untouched).
        let mut e2 = e.clone();
        assert!(!should_skip(BuildMode::Discovery, true, &mut e2, "s"));
        assert_eq!(e2.skip_remaining, 3, "moved source does not touch backoff");
        // Discovery, unchanged, cooling down → skip.
        assert!(should_skip(BuildMode::Discovery, false, &mut e, "s"));
    }

    // ── op-aware selection (pairs, eligibility, due-checks) ─────────────────

    use crate::binding::{
        BINDING_VERSION, BuildOperation, CoverageSemantics, Operations, ResolvedBinding,
        SyncOperation, VerifyOperation, hash_binding,
    };
    use crate::pipeline::{Facet, Medium, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::MemPipelineRecord;

    use super::super::findings::{
        Finding, FindingClass, FindingKey, FindingTarget, FindingsStore, write_findings_store,
    };

    fn empty_engine() -> Engine {
        Engine::from_mounts(Vec::new()).unwrap()
    }

    fn binding_with(operations: Operations) -> BindingV1 {
        BindingV1 {
            version: BINDING_VERSION,
            intent: None,
            source_facets: Vec::new(),
            reference_mems: Vec::new(),
            destination_mem: "m".to_string(),
            deny_paths: Vec::new(),
            coverage_semantics: CoverageSemantics::Exhaustive,
            rules: None,
            prune: None,
            operations,
        }
    }

    fn build_op(trigger: IngestTrigger) -> BuildOperation {
        BuildOperation {
            mode: BuildMode::Discovery,
            trigger,
            batch_size: 20,
            post_actions: None,
        }
    }

    fn record(name: &str, config: BindingV1) -> MemPipelineRecord<BindingV1> {
        MemPipelineRecord {
            mem: "m".to_string(),
            name: name.to_string(),
            config,
        }
    }

    fn configs_of(bindings: Vec<MemPipelineRecord<BindingV1>>) -> BindingConfigs {
        BindingConfigs {
            mediums: Vec::new(),
            facets: Vec::new(),
            bindings,
        }
    }

    /// The eligibility gate: a pair rotates only when its operation block
    /// exists AND declares `trigger: loop`. A manual build, a build-less
    /// binding's absent block, and a manual sync/verify never rotate.
    #[test]
    fn eligibility_requires_block_and_loop_trigger() {
        let ws = tempfile::tempdir().unwrap();
        let engine = empty_engine();
        let configs = configs_of(vec![
            // build loop → the only eligible pair.
            record(
                "a",
                binding_with(Operations {
                    build: Some(build_op(IngestTrigger::Loop)),
                    sync: None,
                    verify: None,
                }),
            ),
            // build manual → excluded (consent lives in the declaration).
            record(
                "b",
                binding_with(Operations {
                    build: Some(build_op(IngestTrigger::Manual)),
                    sync: None,
                    verify: None,
                }),
            ),
            // no build; sync/verify manual → nothing eligible from it.
            record(
                "c",
                binding_with(Operations {
                    build: None,
                    sync: Some(SyncOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                    }),
                    verify: Some(VerifyOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                        adjudication_cap: 50,
                        full_resync_every: 20,
                    }),
                }),
            ),
        ]);

        // Build filter and Any agree: only `m/a`'s build pair rotates.
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Build)
            ),
            Some(("m/a".to_string(), OperationKind::Build))
        );
        assert_eq!(
            select_next_due_operation(&engine, ws.path(), &configs, OperationFilter::Any),
            Some(("m/a".to_string(), OperationKind::Build))
        );
        // Sync / verify filters: the declared blocks are manual → no pair.
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Sync)
            ),
            None
        );
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Verify)
            ),
            None
        );
    }

    /// The sync due-check: a loop-declared sync pair with unmoved sources is
    /// due only when open findings exist under the binding's current
    /// `(hash(D), source_head)` key; an empty current batch is not due.
    #[test]
    fn sync_pair_due_only_on_open_findings_when_source_unmoved() {
        let ws = tempfile::tempdir().unwrap();
        let engine = empty_engine();
        let binding = binding_with(Operations {
            build: None,
            sync: Some(SyncOperation {
                trigger: IngestTrigger::Loop,
                batch_size: 20,
            }),
            verify: None,
        });
        let configs = configs_of(vec![record("s", binding.clone())]);

        // No findings store → not due.
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Sync)
            ),
            None
        );

        // The current key for a source-less binding: hash(D) + empty head.
        let key = FindingKey {
            binding_hash: hash_binding(&ResolvedBinding {
                binding: binding.clone(),
                primary_sources: Vec::new(),
            }),
            source_head: String::new(),
        };

        // An empty batch under the current key → still not due.
        let mut store = FindingsStore {
            binding: "m/s".to_string(),
            batches: Vec::new(),
        };
        store.record(key.clone(), "0".to_string(), Vec::new());
        write_findings_store(ws.path(), "m", "s", &store).unwrap();
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Sync)
            ),
            None
        );

        // One open finding under the current key → the sync pair is due.
        store.record(
            key.clone(),
            "1".to_string(),
            vec![Finding {
                key: key.clone(),
                facet: "f".to_string(),
                target: FindingTarget::Artifact {
                    artifact: "a.rs".to_string(),
                },
                class: FindingClass::Uncovered,
                detail: "no anchor".to_string(),
                created_at: "1".to_string(),
            }],
        );
        write_findings_store(ws.path(), "m", "s", &store).unwrap();
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Sync)
            ),
            Some(("m/s".to_string(), OperationKind::Sync))
        );

        // Findings under a DIFFERENT key (superseded) do not make sync due.
        let mut stale = FindingsStore {
            binding: "m/s".to_string(),
            batches: Vec::new(),
        };
        let stale_key = FindingKey {
            binding_hash: "0000".to_string(),
            source_head: "old".to_string(),
        };
        stale.record(
            stale_key.clone(),
            "1".to_string(),
            vec![Finding {
                key: stale_key,
                facet: "f".to_string(),
                target: FindingTarget::Artifact {
                    artifact: "a.rs".to_string(),
                },
                class: FindingClass::Uncovered,
                detail: "stale".to_string(),
                created_at: "1".to_string(),
            }],
        );
        write_findings_store(ws.path(), "m", "s", &stale).unwrap();
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Sync)
            ),
            None,
            "superseded findings must not pull a sync into rotation"
        );
    }

    /// A binding with a live (mtime) source over `ws`, `source_facets: [f]`.
    fn configs_with_live_source(operations: Operations) -> BindingConfigs {
        let mut binding = binding_with(operations);
        binding.source_facets = vec!["f".to_string()];
        BindingConfigs {
            mediums: vec![MemPipelineRecord {
                mem: "m".to_string(),
                name: "src".to_string(),
                config: Medium {
                    name: "src".to_string(),
                    medium_type: MediumType::Filesystem,
                    pointer: String::new(),
                    change_detection: Some("mtime".to_string()),
                },
            }],
            facets: vec![MemPipelineRecord {
                mem: "m".to_string(),
                name: "f".to_string(),
                config: Facet {
                    name: "f".to_string(),
                    medium: "src".to_string(),
                    scope: vec![PatternEntry {
                        path: "**/*.rs".to_string(),
                        mode: PatternMode::Allow,
                    }],
                    engagement: None,
                    preparation: None,
                },
            }],
            bindings: vec![record("v", binding)],
        }
    }

    /// The verify due-check: a never-verified binding whose source has a live
    /// change-detection token is due its first verify; a source with no
    /// signal (unscoped facet → no token) is not.
    #[test]
    fn verify_pair_due_when_never_verified_with_live_token() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a.rs"), "x").unwrap();
        let engine = empty_engine();
        let verify_loop = Operations {
            build: None,
            sync: None,
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Loop,
                batch_size: 20,
                adjudication_cap: 50,
                full_resync_every: 20,
            }),
        };

        // Live token (scoped mtime source), never verified → due.
        let configs = configs_with_live_source(verify_loop.clone());
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Verify)
            ),
            Some(("m/v".to_string(), OperationKind::Verify))
        );

        // No signal (unscoped facet → no current token) → not due.
        let mut no_signal = configs_with_live_source(verify_loop);
        no_signal.facets[0].config.scope.clear();
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &no_signal,
                OperationFilter::Only(OperationKind::Verify)
            ),
            None
        );
    }

    /// `Any` rotates round-robin across (binding, operation) pairs in pair-id
    /// order, and the cursor is pair-keyed: build and verify pairs alternate.
    #[test]
    fn any_filter_rotates_across_pairs() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a.rs"), "x").unwrap();
        let engine = empty_engine();

        // Two bindings: `m/a` build-loop (no sources) and `m/v` verify-loop
        // over a live mtime source. Pair order: `m/a#build` < `m/v#verify`.
        let mut configs = configs_with_live_source(Operations {
            build: None,
            sync: None,
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Loop,
                batch_size: 20,
                adjudication_cap: 50,
                full_resync_every: 20,
            }),
        });
        configs.bindings.push(record(
            "a",
            binding_with(Operations {
                build: Some(build_op(IngestTrigger::Loop)),
                sync: None,
                verify: None,
            }),
        ));

        let next = || {
            select_next_due_operation(&engine, ws.path(), &configs, OperationFilter::Any).unwrap()
        };
        assert_eq!(next(), ("m/a".to_string(), OperationKind::Build));
        assert_eq!(next(), ("m/v".to_string(), OperationKind::Verify));
        assert_eq!(next(), ("m/a".to_string(), OperationKind::Build));
    }

    /// Pre-pair (single-key) backoff entries are discarded, not honoured: a
    /// legacy `m/a` entry with pending skips does not delay the `m/a#build`
    /// pair, and the rewritten cache carries only pair-keyed entries.
    #[test]
    fn legacy_single_key_backoff_entries_are_discarded() {
        let ws = tempfile::tempdir().unwrap();
        let engine = empty_engine();
        let cache_root = ws.path().join(".memstead.cache").join("ingest");
        std::fs::create_dir_all(&cache_root).unwrap();
        let legacy: BTreeMap<String, BackoffEntry> = [(
            "m/a".to_string(),
            BackoffEntry {
                skip_remaining: 5,
                skip_level: 5,
                snapshot: "s".to_string(),
            },
        )]
        .into();
        std::fs::write(
            cache_root.join("ingest-backoff.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();

        let configs = configs_of(vec![record(
            "a",
            binding_with(Operations {
                build: Some(build_op(IngestTrigger::Loop)),
                sync: None,
                verify: None,
            }),
        )]);
        assert_eq!(
            select_next_due_operation(
                &engine,
                ws.path(),
                &configs,
                OperationFilter::Only(OperationKind::Build)
            ),
            Some(("m/a".to_string(), OperationKind::Build)),
            "a legacy entry's pending skips are discarded, not honoured"
        );

        let rewritten: BTreeMap<String, BackoffEntry> =
            serde_json::from_slice(&std::fs::read(cache_root.join("ingest-backoff.json")).unwrap())
                .unwrap();
        assert!(!rewritten.contains_key("m/a"), "legacy key pruned");
        assert!(rewritten.contains_key("m/a#build"), "pair key written");
    }
}
