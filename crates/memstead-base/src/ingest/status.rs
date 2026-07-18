//! `memstead status` projection view (bundle plan `03-projection-promotion`,
//! decision D11).
//!
//! The `projections` array the status payload carries alongside the graph
//! counts: one entry per v1 binding, reporting its declared operations, each
//! source's baseline tokens + resolved change-detection signal, and the
//! pending/disposed advance counts. Purely read-only — it loads the v1 binding
//! store, reads the destination mem's `sync_state`, resolves each source's
//! [`ChangeStrategy`], and reads the durable advance store. No mutation, no
//! scheduling.
//!
//! The `signal` is the *resolved* change-detection strategy or the literal
//! `"none"` (E1's visible-NoSignal) — never a fabricated token: a
//! detection-less source renders `"none"`, not a fake green.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::Engine;
use crate::binding::CoverageSemantics;
use crate::ingest::advance::read_advance_store;
use crate::ingest::cursor::source_moved;
use crate::ingest::findings::{FindingClass, current_findings};
use crate::ingest::render::mem_predates_binding;
use crate::ingest::resolve::{
    ChangeStrategy, ResolvedIngest, ResolvedSource, resolve_binding_run, resolve_change_strategy,
};
use crate::pipeline_store::load_pipeline_configs;

/// One source facet's (or reference mem's) baseline + signal state (D11). Keyed
/// in [`ProjectionStatus::state`] by the facet-or-refmem name — the same key
/// space the `sync_state` map uses (`<binding>/<facet>#synced`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FacetState {
    /// The `#synced` baseline token, or `None` (rendered `null`) when the
    /// source has never been synced.
    pub synced: Option<String>,
    /// The `#verified` baseline token, or `None` when never verified.
    pub verified: Option<String>,
    /// The resolved change-detection strategy — `git` / `mtime` / `graph` — or
    /// `none` (E1's visible-NoSignal). Never a fabricated token.
    pub signal: String,
}

/// The advance counts (D11): how many artifacts a frozen advance slice still
/// has pending versus how many have been disposed. Both zero when no advance
/// is in flight for the binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdvanceCounts {
    /// Undisposed artifacts remaining in the frozen slice.
    pub pending: usize,
    /// Artifacts disposed so far.
    pub disposed: usize,
}

/// Open-finding counts by class for one binding — the drill-down's share
/// of the scan the rollup aggregates. All zero when the binding is clean
/// or onboarding (onboarding skips the findings scan by design: its
/// uncovered artifacts are the backfill worklist, not defects).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FindingCounts {
    /// Entities describing source that no longer exists.
    pub unresolvable: usize,
    /// Anchors drifted from source (adjudicated mismatches included).
    pub drifted: usize,
    /// In-scope source artifacts carrying no entity.
    pub uncovered: usize,
    /// Findings queued for adjudication.
    pub queued: usize,
}

/// One binding's status entry (D11) — the per-binding drill-down, carrying
/// the SAME resolution the workspace rollup aggregates (verdict, moved
/// source, finding counts) so consumers never re-derive it client-side.
/// The workspace-level lead stays [`Rollup`] / [`projection_rollup`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectionStatus {
    /// The canonical binding id `<mem>/<stem>` (D3).
    pub binding: String,
    /// The mem this binding writes into.
    pub destination_mem: String,
    /// The operations the binding declares — `build` always, plus `sync` /
    /// `verify` when their blocks are present.
    pub operations: Vec<String>,
    /// Per source facet-or-refmem state, keyed by the facet/mem name.
    pub state: BTreeMap<String, FacetState>,
    /// Pending / disposed advance counts for the binding.
    pub advance: AdvanceCounts,
    /// This binding's own verdict, by the rollup's exact rules:
    /// `onboarding` when the mem predates its binding (never red);
    /// `action-needed` on open findings that count as actions or a moved
    /// source; `clean` otherwise.
    pub verdict: RollupVerdict,
    /// True when a change-detectable source moved past its `#synced`
    /// baseline. Always false for onboarding bindings (scan skipped).
    pub source_moved: bool,
    /// Open findings under the current key, by class.
    pub findings: FindingCounts,
}

/// The shared per-binding scan both [`projection_status`] and
/// [`projection_rollup`] resolve from — one truth, two projections.
struct BindingResolution {
    onboarding: bool,
    source_moved: bool,
    findings: FindingCounts,
    /// Whether the findings/moved state counts as an action under the
    /// rollup's rules (uncovered only under exhaustive coverage).
    has_action: bool,
}

impl BindingResolution {
    fn verdict(&self) -> RollupVerdict {
        if self.onboarding {
            RollupVerdict::Onboarding
        } else if self.has_action {
            RollupVerdict::ActionNeeded
        } else {
            RollupVerdict::Clean
        }
    }
}

fn resolve_binding_status(
    engine: &Engine,
    workspace_root: &Path,
    binding: &crate::binding::Binding,
    resolved: &ResolvedIngest,
) -> BindingResolution {
    if mem_predates_binding(engine, resolved) {
        return BindingResolution {
            onboarding: true,
            source_moved: false,
            findings: FindingCounts::default(),
            has_action: false,
        };
    }
    let source_moved = source_moved(engine, resolved, workspace_root);
    let mut findings = FindingCounts::default();
    if let Ok((_key, list)) = current_findings(engine, workspace_root, binding, resolved) {
        for f in &list {
            match f.class {
                FindingClass::UnresolvableAnchor => findings.unresolvable += 1,
                FindingClass::Drifted | FindingClass::Wrong => findings.drifted += 1,
                FindingClass::Uncovered => findings.uncovered += 1,
                FindingClass::QueuedForAdjudication => findings.queued += 1,
            }
        }
    }
    let uncovered_counts = findings.uncovered > 0
        && matches!(binding.coverage_semantics, CoverageSemantics::Exhaustive);
    let has_action = source_moved
        || findings.unresolvable > 0
        || findings.drifted > 0
        || uncovered_counts
        || findings.queued > 0;
    BindingResolution {
        onboarding: false,
        source_moved,
        findings,
        has_action,
    }
}

/// Map a resolved [`ChangeStrategy`] to its `signal` string (D11). `None`
/// renders the literal `"none"` — E1's visible-NoSignal, never a fake token.
fn signal_of(strategy: ChangeStrategy) -> &'static str {
    match strategy {
        ChangeStrategy::None => "none",
        ChangeStrategy::Git => "git",
        ChangeStrategy::Mtime => "mtime",
        ChangeStrategy::Graph => "graph",
    }
}

/// Build the `projections` array for `memstead status` (D11) from the v1
/// binding store rooted at `workspace_root`, reading baselines off `engine`'s
/// destination-mem `sync_state` and the durable advance store.
///
/// Read-only and best-effort: a workspace with no v1 binding store (or one
/// whose store fails to load — e.g. a not-yet-migrated legacy layout) yields an
/// empty array rather than failing the whole status call. A binding whose
/// sources cannot be resolved (dangling facet/medium) contributes its
/// operations + advance counts with an empty `state` map.
pub fn projection_status(engine: &Engine, workspace_root: &Path) -> Vec<ProjectionStatus> {
    let Ok(configs) = load_pipeline_configs(workspace_root) else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(configs.bindings.len());
    for record in &configs.bindings {
        let binding_id = format!("{}/{}", record.mem, record.name);
        let binding = &record.config;

        let mut operations = Vec::new();
        if binding.operations.build.is_some() {
            operations.push("build".to_string());
        }
        if binding.operations.sync.is_some() {
            operations.push("sync".to_string());
        }
        if binding.operations.verify.is_some() {
            operations.push("verify".to_string());
        }

        // Baselines live on the destination mem's config `sync_state` (D4).
        let sync_state = engine
            .mem_config_for(&binding.destination_mem)
            .map(|c| c.sync_state.clone())
            .unwrap_or_default();

        // Resolve the binding's sources so each facet's change-detection
        // strategy (its `signal`) is the same one the cursor/brief path uses.
        let mut state = BTreeMap::new();
        let mut resolution: Option<BindingResolution> = None;
        if let Ok(resolved) = resolve_binding_run(&binding_id, binding) {
            resolution = Some(resolve_binding_status(
                engine,
                workspace_root,
                binding,
                &resolved,
            ));
            for source in &resolved.sources {
                let (facet, signal) = match source {
                    ResolvedSource::Primary(p) => (
                        p.name.clone(),
                        signal_of(resolve_change_strategy(p, workspace_root)).to_string(),
                    ),
                    // Reference mems are graph-detected by definition (the
                    // source mem's snapshot token).
                    ResolvedSource::Reference { mem } => (mem.clone(), "graph".to_string()),
                };
                let synced = sync_state
                    .get(&format!("{binding_id}/{facet}#synced"))
                    .cloned();
                let verified = sync_state
                    .get(&format!("{binding_id}/{facet}#verified"))
                    .cloned();
                state.insert(
                    facet,
                    FacetState {
                        synced,
                        verified,
                        signal,
                    },
                );
            }
        }

        // Durable advance store (D7) — absent = nothing in flight (0/0).
        let advance = match read_advance_store(workspace_root, &record.mem, &record.name) {
            Ok(Some(s)) => AdvanceCounts {
                pending: s.pending(),
                disposed: s.disposed(),
            },
            _ => AdvanceCounts {
                pending: 0,
                disposed: 0,
            },
        };

        let (verdict, source_moved, findings) = match &resolution {
            Some(r) => (r.verdict(), r.source_moved, r.findings),
            None => (RollupVerdict::Clean, false, FindingCounts::default()),
        };
        out.push(ProjectionStatus {
            binding: binding_id,
            destination_mem: binding.destination_mem.clone(),
            operations,
            state,
            advance,
            verdict,
            source_moved,
            findings,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Rollup — the dashboard lead (G1)
// ---------------------------------------------------------------------------

/// The single dashboard verdict `memstead status` leads with (G1). One verdict
/// summarising every projection binding; the per-binding numbers
/// ([`projection_status`]) are the drill-down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RollupVerdict {
    /// No binding declares open findings and no source has moved past its
    /// baseline — or there are no bindings at all.
    Clean,
    /// Onboarding only: one or more bindings predate their binding (adopt) and
    /// nothing else needs a maintenance pass. **A pre-binding mem is never a red
    /// verdict** — 0% anchored is expected onboarding, not a defect (E1).
    Onboarding,
    /// One or more bindings carry open findings (drift, unresolvable anchors,
    /// uncovered artifacts under exhaustive coverage, adjudication backlog) or
    /// have a source that moved past its `#synced` baseline.
    ActionNeeded,
}

impl RollupVerdict {
    /// Stable wire string.
    pub fn as_wire(&self) -> &'static str {
        match self {
            RollupVerdict::Clean => "clean",
            RollupVerdict::Onboarding => "onboarding",
            RollupVerdict::ActionNeeded => "action-needed",
        }
    }
}

/// The dashboard rollup (G1): one verdict, a one-line headline, and up to three
/// concrete, highest-severity actions derived from the durable findings store
/// plus freshness. The full per-binding numbers ride [`projection_status`] as
/// the drill-down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rollup {
    /// The single lead verdict.
    pub verdict: RollupVerdict,
    /// A one-line human/agent summary of the workspace's projection health.
    pub headline: String,
    /// Up to three concrete next actions, highest-severity first (e.g. "3
    /// entities describe source that no longer exists — run sync").
    pub actions: Vec<String>,
}

impl Default for Rollup {
    fn default() -> Self {
        Rollup {
            verdict: RollupVerdict::Clean,
            headline: "No projection bindings declared.".to_string(),
            actions: Vec::new(),
        }
    }
}

/// One candidate action with its severity — the higher, the more urgent. Used
/// only to rank the top-three actions the rollup surfaces.
struct Candidate {
    severity: u8,
    text: String,
}

/// Compute the dashboard rollup (G1) for every projection binding in the
/// workspace: one verdict plus the top-three concrete actions, derived from the
/// durable findings store and freshness (source movement vs. the `#synced`
/// baseline). **Read-only** on every mem — it borrows `&Engine` (shared) and
/// only reads the binding store, the findings store, and the live cursor.
///
/// Best-effort like [`projection_status`]: a workspace with no binding store, or
/// one whose bindings fail to resolve, yields the default clean rollup rather
/// than failing the whole status call.
///
/// A binding that predates its binding (no anchors, never synced) contributes an
/// **onboarding** action, never a red one — its uncovered artifacts are the
/// expected first-sync backfill worklist, so pre-binding history alone never
/// drives an `action-needed` verdict (E1).
pub fn projection_rollup(engine: &Engine, workspace_root: &Path) -> Rollup {
    let Ok(configs) = load_pipeline_configs(workspace_root) else {
        return Rollup::default();
    };
    if configs.bindings.is_empty() {
        return Rollup::default();
    }
    let total = configs.bindings.len();

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut action_bindings = 0usize;
    let mut onboarding_bindings = 0usize;

    for record in &configs.bindings {
        let binding_id = format!("{}/{}", record.mem, record.name);
        let binding = &record.config;
        let Ok(resolved) = resolve_binding_run(&binding_id, binding) else {
            continue;
        };

        // The SAME per-binding scan projection_status serves (one truth).
        let resolution = resolve_binding_status(engine, workspace_root, binding, &resolved);

        // Adopt (E1): a mem that predates its binding is onboarding, never a red
        // verdict. Its uncovered artifacts are the backfill worklist, so we skip
        // the findings/freshness scan that would otherwise read them as defects.
        if resolution.onboarding {
            onboarding_bindings += 1;
            candidates.push(Candidate {
                severity: 1,
                text: format!(
                    "`{binding_id}` predates its binding — 0% anchored is expected; run \
                     `memstead projection sync {binding_id}` for a first-sync backfill"
                ),
            });
            continue;
        }

        // Freshness: a change-detectable source moved past its `#synced` baseline.
        if resolution.source_moved {
            candidates.push(Candidate {
                severity: 4,
                text: format!(
                    "`{binding_id}` source moved since the last sync — run `memstead projection \
                     sync {binding_id}`"
                ),
            });
        }

        let FindingCounts {
            unresolvable,
            drifted,
            uncovered,
            queued,
        } = resolution.findings;
        if unresolvable > 0 {
            candidates.push(Candidate {
                severity: 6,
                text: format!(
                    "{unresolvable} entit{} in `{binding_id}` describe source that no longer \
                     exists — run `memstead projection sync {binding_id}`",
                    if unresolvable == 1 { "y" } else { "ies" }
                ),
            });
        }
        if drifted > 0 {
            candidates.push(Candidate {
                severity: 5,
                text: format!(
                    "{drifted} anchor(s) in `{binding_id}` drifted from their source — run \
                     `memstead projection sync {binding_id}`"
                ),
            });
        }
        // Uncovered drives an action only under exhaustive coverage — a
        // curated binding covers a deliberate slice, so uncovered is
        // information, not a defect (B4). (`has_action` already encodes
        // this rule; the candidate mirrors it.)
        if uncovered > 0 && matches!(binding.coverage_semantics, CoverageSemantics::Exhaustive) {
            candidates.push(Candidate {
                severity: 3,
                text: format!(
                    "{uncovered} in-scope source artifact(s) in `{binding_id}` carry no entity \
                     — run `memstead projection verify {binding_id}`, then sync"
                ),
            });
        }
        if queued > 0 {
            candidates.push(Candidate {
                severity: 2,
                text: format!(
                    "{queued} finding(s) in `{binding_id}` queued for adjudication — run \
                     `memstead projection verify {binding_id}`"
                ),
            });
        }

        if resolution.has_action {
            action_bindings += 1;
        }
    }

    // Highest severity first; the stable sort preserves insertion order within a
    // severity so runs are reproducible.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.severity));
    let actions: Vec<String> = candidates.into_iter().take(3).map(|c| c.text).collect();

    let verdict = if action_bindings > 0 {
        RollupVerdict::ActionNeeded
    } else if onboarding_bindings > 0 {
        RollupVerdict::Onboarding
    } else {
        RollupVerdict::Clean
    };

    let headline = match verdict {
        RollupVerdict::ActionNeeded => format!(
            "Action needed — {action_bindings} of {total} projection(s) have open findings or a \
             moved source."
        ),
        RollupVerdict::Onboarding => format!(
            "Onboarding — {onboarding_bindings} of {total} projection(s) predate their binding; a \
             first-sync backfill is expected, not a defect."
        ),
        RollupVerdict::Clean => {
            format!("All {total} projection(s) are in sync — no open findings, no moved sources.")
        }
    };

    Rollup {
        verdict,
        headline,
        actions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{
        BINDING_VERSION, Binding, BuildMode, BuildOperation, CoverageSemantics, Operations,
        SyncOperation,
    };
    use crate::pipeline::{Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::{write_binding, write_facet, write_medium};
    use crate::storage::FilesystemMemWriter;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
    use tempfile::TempDir;

    /// A workspace with one folder mem `engine` (also a git source tree), a v1
    /// binding `engine/graph` over a `source-tree` facet (codebase / git), and
    /// a seeded `#synced` baseline — the projection status reports the binding's
    /// operations, the `git` signal, the synced token, and 0/0 advance.
    #[test]
    fn projection_status_reports_operations_signal_and_baseline() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Mem config so `sync_state` can be read/written.
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("config.json"),
            br#"{"format":1,"schema":"default@1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "[workspace]\n",
        )
        .unwrap();
        // A git work tree so the codebase medium resolves the `git` strategy.
        let out = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());

        // The v1 binding + its facet/medium.
        write_binding(
            root,
            "engine",
            "graph",
            &Binding {
                version: BINDING_VERSION,
                intent: None,
                sources: vec![crate::pipeline::Source {
                    name: "graph".to_string(),
                    medium_type: MediumType::Codebase,
                    pointer: String::new(),
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
                    sync: Some(SyncOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                    }),
                    verify: None,
                },
            },
        )
        .unwrap();

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
        let mut engine = Engine::from_mounts(vec![(
            mount,
            Box::new(FilesystemMemWriter::new(root.to_path_buf()))
                as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        engine
            .set_mem_sync_state("engine", "engine/graph/graph#synced", "deadbeef", None)
            .unwrap();

        let ps = projection_status(&engine, root);
        assert_eq!(ps.len(), 1);
        let p = &ps[0];
        assert_eq!(p.binding, "engine/graph");
        assert_eq!(p.destination_mem, "engine");
        assert_eq!(p.operations, vec!["build".to_string(), "sync".to_string()]);
        let facet = p.state.get("graph").expect("the source facet's state");
        assert_eq!(facet.signal, "git");
        assert_eq!(facet.synced.as_deref(), Some("deadbeef"));
        assert_eq!(facet.verified, None);
        assert_eq!(
            p.advance,
            AdvanceCounts {
                pending: 0,
                disposed: 0
            }
        );
    }

    /// A workspace with no v1 binding store yields an empty array — status
    /// never fails because a workspace declares no projections.
    #[test]
    fn projection_status_empty_without_bindings() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("config.json"),
            br#"{"format":1,"schema":"default@1.0.0"}"#,
        )
        .unwrap();
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
        let engine = Engine::from_mounts(vec![(
            mount,
            Box::new(FilesystemMemWriter::new(root.to_path_buf()))
                as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        assert!(projection_status(&engine, root).is_empty());
    }

    // ---- G1: rollup verdict + top-3 actions -----------------------------

    /// Build the same one-binding `engine/graph` workspace the status test uses,
    /// returning the engine and root. Seeds **no** `#synced` baseline and **no**
    /// anchors, so the mem predates its binding (the adopt case) unless the
    /// caller seeds otherwise.
    fn one_binding_workspace(tmp: &TempDir) -> Engine {
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("config.json"),
            br#"{"format":1,"schema":"default@1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "[workspace]\n",
        )
        .unwrap();
        let out = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());

        write_binding(
            root,
            "engine",
            "graph",
            &Binding {
                version: BINDING_VERSION,
                intent: None,
                sources: vec![crate::pipeline::Source {
                    name: "graph".to_string(),
                    medium_type: MediumType::Codebase,
                    pointer: String::new(),
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
                    sync: Some(SyncOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                    }),
                    verify: None,
                },
            },
        )
        .unwrap();

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

    /// G1 + E1 — a binding whose mem predates it (no anchors, never synced)
    /// rolls up to an **onboarding** verdict, never `action-needed`: the
    /// onboarding action is surfaced and pre-binding history alone drives no red
    /// verdict (E1's refusal at the dashboard level).

    /// The per-binding drill-down carries the SAME resolution the rollup
    /// aggregates: an adopt (pre-binding) mem reads `onboarding` on its own
    /// entry — never red, no moved flag, zero finding counts — and the
    /// workspace rollup agrees (one truth, two projections).
    #[test]
    fn projection_status_carries_the_per_binding_verdict() {
        let tmp = TempDir::new().unwrap();
        let engine = one_binding_workspace(&tmp);
        let statuses = projection_status(&engine, tmp.path());
        assert_eq!(statuses.len(), 1);
        let s = &statuses[0];
        assert_eq!(s.verdict, RollupVerdict::Onboarding);
        assert!(!s.source_moved, "onboarding skips the freshness scan");
        assert_eq!(s.findings, FindingCounts::default());
        // Wire shape: the verdict serializes kebab-case like the rollup's.
        let json = serde_json::to_value(s).unwrap();
        assert_eq!(json["verdict"], "onboarding");
        assert_eq!(json["source_moved"], false);
        assert_eq!(json["findings"]["unresolvable"], 0);
        // And the workspace rollup resolves from the same scan.
        let rollup = projection_rollup(&engine, tmp.path());
        assert_eq!(rollup.verdict, RollupVerdict::Onboarding);
    }

    #[test]
    fn rollup_adopt_binding_is_onboarding_not_action_needed() {
        let tmp = TempDir::new().unwrap();
        let engine = one_binding_workspace(&tmp);
        let rollup = projection_rollup(&engine, tmp.path());
        assert_eq!(rollup.verdict, RollupVerdict::Onboarding);
        assert_ne!(
            rollup.verdict,
            RollupVerdict::ActionNeeded,
            "pre-binding history alone must never be a red verdict"
        );
        assert!(
            rollup
                .actions
                .iter()
                .any(|a| a.contains("predates its binding")),
            "the onboarding action is surfaced: {:?}",
            rollup.actions
        );
        assert!(rollup.headline.contains("Onboarding"));
    }

    /// G1 — a workspace with no bindings rolls up to the default **clean**
    /// verdict with no actions.
    #[test]
    fn rollup_empty_without_bindings_is_clean() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("config.json"),
            br#"{"format":1,"schema":"default@1.0.0"}"#,
        )
        .unwrap();
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
        let engine = Engine::from_mounts(vec![(
            mount,
            Box::new(FilesystemMemWriter::new(root.to_path_buf()))
                as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        let rollup = projection_rollup(&engine, root);
        assert_eq!(rollup.verdict, RollupVerdict::Clean);
        assert!(rollup.actions.is_empty());
        assert!(rollup.headline.contains("No projection bindings"));
    }
}
