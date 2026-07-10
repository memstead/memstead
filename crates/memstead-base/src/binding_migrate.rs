//! Gen-2 four-primitive → binding **v1** migration (D10, gen-2 path).
//!
//! Converts the current four-primitive store (per-mem [`Projection`] + flat
//! [`Ingest`]) into v1 [`BindingV1`] records: each flat ingest is merged into
//! the projection its `projection` ref names, collapsing the
//! declaration/schedule split into one versioned binding. The canonical
//! binding id is `<mem>/<stem>` — the projection's owning mem dir plus its
//! file stem, i.e. the very string the ingest already used as its `projection`
//! ref (D3).
//!
//! [`migrate_gen2_bindings`] is pure and IO-free — it transforms
//! already-loaded [`PipelineConfigs`] into keyed bindings. The CLI
//! (`memstead projection migrate`) wraps it with the load / write / validate
//! IO. This module is **additive and un-wired**: it neither flips the loader
//! version-gate (D2) nor implements the gen-1 root-folder path (also D10) —
//! those are separate, later slices.
//!
//! ## Field mapping (gen-2 → v1)
//!
//! - `version` = 1 (`BINDING_VERSION`).
//! - `intent` / `source_facets` / `reference_mems` / `destination_mem` /
//!   `rules` — carried over from the [`Projection`] verbatim.
//! - `deny_paths` — moved **up** from the per-ingest record to the binding
//!   (strategy-invariant, E1); bare directory segments are rewritten to E1's
//!   workspace-relative glob dialect where trivially derivable (see
//!   [`to_glob_dialect`]), every rewrite recorded as a note.
//! - `coverage_semantics` — defaults [`CoverageSemantics::Exhaustive`] (gen-2
//!   has no such field).
//! - `operations.build` — the ingest's `mode` / `trigger` / `batch_size` /
//!   `post_actions`. `mode` maps `discovery` → [`BuildMode::Discovery`] and
//!   `one-shot` → [`BuildMode::OneShot`]; **`refinement` is a typed migrate
//!   error** ([`BindingMigrateError::RefinementModeDeleted`]) — the vocabulary
//!   is deleted, not migrated (D1).
//! - `operations.sync` / `operations.verify` — `None`. A gen-2 config declares
//!   only the build-equivalent schedule; sync/verify are enabled later via
//!   `projection enable`, never fabricated by migration.
//!
//! A **dangling ingest→projection ref** is a typed migrate error
//! ([`BindingMigrateError::DanglingProjectionRef`]), never a silent drop (D10).

use crate::binding::{
    BINDING_VERSION, BindingV1, BuildMode, BuildOperation, CoverageSemantics, Operations,
    ResolvedBinding,
};
use crate::ingest::resolve::{ResolveError, resolve_binding};
use crate::pipeline::Projection;
use crate::pipeline_store::{LegacyIngest, LegacyIngestMode, PipelineConfigs};

/// Why a gen-2 config could not be migrated to a v1 binding. Every variant
/// names the offending ingest so the failure is diagnosable without
/// re-reading the store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingMigrateError {
    /// The ingest declares build mode `refinement` — deleted from the binding
    /// vocabulary (D1). Not migrated: refinement-as-writer is gone, so there
    /// is no v1 shape to carry it into.
    #[error(
        "ingest '{ingest}' declares build mode 'refinement', which is deleted from the binding \
         vocabulary (D1) — refinement-as-writer is gone; re-declare it as a discovery build (plus \
         a sync/verify obligation) before migrating"
    )]
    RefinementModeDeleted {
        /// The offending ingest (its file stem).
        ingest: String,
    },
    /// The ingest's `projection` field is not the required `"<mem>/<name>"`.
    #[error(
        "ingest '{ingest}' has a malformed projection ref '{projection}'; expected \"<mem>/<name>\""
    )]
    MalformedProjectionRef {
        /// The ingest whose projection ref is malformed.
        ingest: String,
        /// The malformed value.
        projection: String,
    },
    /// The ingest references a projection that does not exist — a dangling ref.
    /// A typed error, never a silent drop (D10).
    #[error(
        "ingest '{ingest}' references projection '{projection_ref}' which does not exist in mem \
         '{mem}' (dangling ref — not migrated); available: {}",
        fmt_list(available)
    )]
    DanglingProjectionRef {
        /// The referencing ingest.
        ingest: String,
        /// The full `"<mem>/<name>"` ref.
        projection_ref: String,
        /// The mem the projection was looked up in.
        mem: String,
        /// The projection names that do exist in that mem.
        available: Vec<String>,
    },
}

/// Render a name list for an error message: `a, b, c` or `(none)`.
fn fmt_list(names: &[String]) -> String {
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

/// Convert a legacy bare-name `deny_paths` entry to E1's workspace-relative
/// glob dialect where trivially derivable, else carry it through unchanged
/// (D1 — the gen-2 dialect-forward-carry).
///
/// A **bare directory segment** — non-empty, no path separator, no glob
/// metacharacter (`*?[]`), and no `.` extension marker — is rewritten to a
/// recursive-subtree glob `<segment>/**` (so `dev` → `dev/**`, matching E1's
/// `"dev/**"` example). Anything already carrying a `/`, a glob metacharacter,
/// or a `.` (a file like `VISION.md`, already a valid workspace-relative
/// match) is a no-op — carried through. The return value equals the input
/// exactly when nothing changed, so callers can detect (and note) rewrites by
/// comparison.
fn to_glob_dialect(entry: &str) -> String {
    let is_bare_segment = !entry.is_empty()
        && !entry.contains('/')
        && !entry.contains('.')
        && !entry.contains(['*', '?', '[', ']']);
    if is_bare_segment {
        format!("{entry}/**")
    } else {
        entry.to_string()
    }
}

/// A single migrated binding paired with the identity and provenance the CLI
/// needs to write it to disk and report it.
#[derive(Debug, Clone, PartialEq)]
pub struct MigratedBinding {
    /// The canonical binding id `<mem>/<stem>` (D3) — the projection ref the
    /// ingest already used.
    pub id: String,
    /// The projection's owning mem dir (the `.memstead/projections/<mem>/`
    /// tier the binding file lives under).
    pub mem: String,
    /// The projection file stem (`<stem>` in the binding id).
    pub name: String,
    /// The flat ingest merged into this binding (its file stem) — used to
    /// resolve the binding for validation and to delete the consumed ingest.
    pub ingest_name: String,
    /// The produced v1 binding.
    pub binding: BindingV1,
    /// Human-readable notes about non-identity transforms applied (e.g.
    /// deny-path dialect rewrites). Empty when the migration was verbatim.
    pub notes: Vec<String>,
}

/// Convert one gen-2 (`Ingest` + its `Projection`) into a v1 [`BindingV1`],
/// returning the binding plus any per-field transform notes. Pure — no IO.
/// `ingest_name` is used only for error messages.
///
/// See the [module docs](self) for the full field mapping. `refinement` mode
/// is a typed error; every other field carries over or defaults as documented.
pub(crate) fn binding_from_gen2(
    ingest_name: &str,
    ingest: &LegacyIngest,
    projection: &Projection,
) -> Result<(BindingV1, Vec<String>), BindingMigrateError> {
    let mode = match ingest.mode {
        LegacyIngestMode::Discovery => BuildMode::Discovery,
        LegacyIngestMode::OneShot => BuildMode::OneShot,
        LegacyIngestMode::Refinement => {
            return Err(BindingMigrateError::RefinementModeDeleted {
                ingest: ingest_name.to_string(),
            });
        }
    };

    let mut notes = Vec::new();
    let deny_paths = ingest
        .deny_paths
        .iter()
        .map(|d| {
            let converted = to_glob_dialect(d);
            if converted != *d {
                notes.push(format!(
                    "deny_paths: rewrote bare entry '{d}' to glob dialect '{converted}'"
                ));
            }
            converted
        })
        .collect();

    let binding = BindingV1 {
        version: BINDING_VERSION,
        intent: projection.intent.clone(),
        source_facets: projection.source_facets.clone(),
        reference_mems: projection.reference_mems.clone(),
        destination_mem: projection.destination_mem.clone(),
        deny_paths,
        coverage_semantics: CoverageSemantics::Exhaustive,
        rules: projection.rules.clone(),
        prune: None,
        operations: Operations {
            build: Some(BuildOperation {
                mode,
                trigger: ingest.trigger,
                batch_size: ingest.batch_size,
                post_actions: ingest.post_actions.clone(),
            }),
            // A gen-2 config declares only the build-equivalent schedule.
            // Sync/verify are enabled later via `projection enable`, never
            // fabricated by migration.
            sync: None,
            verify: None,
        },
    };
    Ok((binding, notes))
}

/// Migrate every gen-2 flat ingest in `configs` into a v1 [`MigratedBinding`],
/// keyed by binding id (`<mem>/<stem>`, D3), in id order.
///
/// Ingest-driven (D10 — "merge each flat ingest into its projection"): each
/// ingest's `projection` ref is resolved to a projection in that mem and the
/// pair merged. A malformed ref, a dangling ref, or a `refinement` mode is a
/// typed [`BindingMigrateError`] — the migration refuses rather than dropping
/// or fabricating. Pure and IO-free.
///
/// A projection with no ingest pointing at it is inert (never runnable in
/// gen-2) and is not emitted — nothing schedules it, so there is no obligation
/// to promote.
pub fn migrate_gen2_bindings(
    configs: &PipelineConfigs,
) -> Result<Vec<MigratedBinding>, BindingMigrateError> {
    let mut out = Vec::new();
    for record in &configs.ingests {
        let ingest = &record.config;
        let projection_ref = ingest.projection.clone();
        let (mem, name) = projection_ref
            .split_once('/')
            .filter(|(m, n)| !m.is_empty() && !n.is_empty())
            .ok_or_else(|| BindingMigrateError::MalformedProjectionRef {
                ingest: record.name.clone(),
                projection: projection_ref.clone(),
            })?;
        let mem = mem.to_string();
        let name = name.to_string();

        let projection = configs
            .projections
            .iter()
            .find(|r| r.mem == mem && r.name == name)
            .map(|r| &r.config)
            .ok_or_else(|| BindingMigrateError::DanglingProjectionRef {
                ingest: record.name.clone(),
                projection_ref: projection_ref.clone(),
                mem: mem.clone(),
                available: configs
                    .projections
                    .iter()
                    .filter(|r| r.mem == mem)
                    .map(|r| r.name.clone())
                    .collect(),
            })?;

        let (binding, notes) = binding_from_gen2(&record.name, ingest, projection)?;
        out.push(MigratedBinding {
            id: projection_ref,
            mem,
            name,
            ingest_name: record.name.clone(),
            binding,
            notes,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Resolve a migrated binding's primary sources (facet + medium) via the
/// binding resolve layer, so the produced binding can be validated against the
/// D6 capability matrix ([`crate::binding::validate_binding`]).
///
/// Resolves the binding's own `source_facets` against the still-loaded gen-2
/// configs (facets/mediums in the binding-id's `<mem>` tier) via
/// [`resolve_binding`]; reference mems are not resolved (only primary facets
/// carry capability constraints). This gives the D6 matrix a real,
/// config-derived consumer.
pub fn resolve_migrated_binding(
    configs: &PipelineConfigs,
    binding_id: &str,
    binding: BindingV1,
) -> Result<ResolvedBinding, ResolveError> {
    resolve_binding(configs, binding_id, &binding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{CapabilityError, validate_binding};
    use crate::pipeline::{Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::{
        LegacyIngest, LegacyIngestMode, MemPipelineRecord, PipelineRecord,
    };

    fn medium(mem: &str, name: &str, ty: MediumType, pointer: &str) -> MemPipelineRecord<Medium> {
        MemPipelineRecord {
            mem: mem.to_string(),
            name: name.to_string(),
            config: Medium {
                name: name.to_string(),
                medium_type: ty,
                pointer: pointer.to_string(),
                change_detection: None,
            },
        }
    }

    fn facet(mem: &str, name: &str, medium: &str, prep: Option<&str>) -> MemPipelineRecord<Facet> {
        MemPipelineRecord {
            mem: mem.to_string(),
            name: name.to_string(),
            config: Facet {
                name: name.to_string(),
                medium: medium.to_string(),
                scope: vec![PatternEntry {
                    path: "../src/**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: prep.map(str::to_string),
            },
        }
    }

    fn projection(
        mem: &str,
        name: &str,
        facets: &[&str],
        refs: &[&str],
        dest: &str,
    ) -> MemPipelineRecord<Projection> {
        MemPipelineRecord {
            mem: mem.to_string(),
            name: name.to_string(),
            config: Projection {
                intent: Some(format!("intent of {name}")),
                source_facets: facets.iter().map(|s| s.to_string()).collect(),
                reference_mems: refs.iter().map(|s| s.to_string()).collect(),
                destination_mem: dest.to_string(),
                rules: Some(serde_json::json!({ "routing": "r" })),
            },
        }
    }

    fn ingest(
        name: &str,
        projection: &str,
        mode: LegacyIngestMode,
        deny: &[&str],
    ) -> PipelineRecord<LegacyIngest> {
        PipelineRecord {
            name: name.to_string(),
            config: LegacyIngest {
                projection: projection.to_string(),
                mode,
                trigger: IngestTrigger::Loop,
                batch_size: 20,
                deny_paths: deny.iter().map(|s| s.to_string()).collect(),
                post_actions: Some(serde_json::json!({ "archive_source": true })),
            },
        }
    }

    /// A well-formed gen-2 pair migrates to a v1 binding that carries the
    /// merged operations (mode/trigger/batch/post_actions) and the projection's
    /// declarative fields, with build-only operations and the id `<mem>/<stem>`.
    #[test]
    fn migrates_a_well_formed_pair() {
        let configs = PipelineConfigs {
            mediums: vec![medium("engine", "src", MediumType::Codebase, "../public")],
            facets: vec![facet("engine", "source-tree", "src", None)],
            projections: vec![projection(
                "engine",
                "graph",
                &["source-tree"],
                &["plugin"],
                "engine",
            )],
            ingests: vec![ingest(
                "engine-graph",
                "engine/graph",
                LegacyIngestMode::Discovery,
                &[],
            )],
        };

        let migrated = migrate_gen2_bindings(&configs).unwrap();
        assert_eq!(migrated.len(), 1);
        let m = &migrated[0];
        assert_eq!(m.id, "engine/graph");
        assert_eq!(m.mem, "engine");
        assert_eq!(m.name, "graph");
        assert_eq!(m.ingest_name, "engine-graph");

        let b = &m.binding;
        assert_eq!(b.version, BINDING_VERSION);
        assert_eq!(b.intent.as_deref(), Some("intent of graph"));
        assert_eq!(b.source_facets, vec!["source-tree".to_string()]);
        assert_eq!(b.reference_mems, vec!["plugin".to_string()]);
        assert_eq!(b.destination_mem, "engine");
        assert_eq!(b.coverage_semantics, CoverageSemantics::Exhaustive);
        assert_eq!(b.rules, Some(serde_json::json!({ "routing": "r" })));
        // Operations: build carries the merged schedule; sync/verify absent.
        assert_eq!(
            b.operations.build.as_ref().unwrap().mode,
            BuildMode::Discovery
        );
        assert_eq!(
            b.operations.build.as_ref().unwrap().trigger,
            IngestTrigger::Loop
        );
        assert_eq!(b.operations.build.as_ref().unwrap().batch_size, 20);
        assert_eq!(
            b.operations.build.as_ref().unwrap().post_actions,
            Some(serde_json::json!({ "archive_source": true }))
        );
        assert!(b.operations.sync.is_none());
        assert!(b.operations.verify.is_none());
    }

    /// The produced binding round-trips losslessly through serde (the on-disk
    /// promotion is faithful).
    #[test]
    fn produced_binding_round_trips() {
        let configs = PipelineConfigs {
            mediums: vec![medium("engine", "src", MediumType::Codebase, "../public")],
            facets: vec![facet("engine", "source-tree", "src", None)],
            projections: vec![projection(
                "engine",
                "graph",
                &["source-tree"],
                &[],
                "engine",
            )],
            ingests: vec![ingest(
                "engine-graph",
                "engine/graph",
                LegacyIngestMode::Discovery,
                &[],
            )],
        };
        let migrated = migrate_gen2_bindings(&configs).unwrap();
        let b = &migrated[0].binding;
        let json = serde_json::to_string(b).unwrap();
        let back: BindingV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, b);
    }

    /// `deny_paths` move up to the binding; a bare directory segment is
    /// rewritten to the glob dialect (with a note), while glob/`/`/`.`
    /// entries carry through unchanged.
    #[test]
    fn deny_paths_move_up_and_bare_segments_convert() {
        let configs = PipelineConfigs {
            mediums: vec![medium("engine", "src", MediumType::Codebase, "../public")],
            facets: vec![facet("engine", "source-tree", "src", None)],
            projections: vec![projection(
                "engine",
                "graph",
                &["source-tree"],
                &[],
                "engine",
            )],
            ingests: vec![ingest(
                "engine-graph",
                "engine/graph",
                LegacyIngestMode::Discovery,
                &["dev", "VISION.md", "../public/target/**"],
            )],
        };
        let migrated = migrate_gen2_bindings(&configs).unwrap();
        let m = &migrated[0];
        assert_eq!(
            m.binding.deny_paths,
            vec![
                "dev/**".to_string(),              // bare segment → glob
                "VISION.md".to_string(),           // has '.', carried through
                "../public/target/**".to_string(), // has '/' + glob, carried through
            ]
        );
        assert_eq!(m.notes.len(), 1, "only the bare 'dev' rewrite is noted");
        assert!(m.notes[0].contains("dev") && m.notes[0].contains("dev/**"));
    }

    /// `one-shot` maps to the one-shot build mode.
    #[test]
    fn one_shot_mode_maps() {
        let configs = PipelineConfigs {
            projections: vec![projection("m", "p", &[], &[], "m")],
            ingests: vec![ingest("i", "m/p", LegacyIngestMode::OneShot, &[])],
            ..Default::default()
        };
        let migrated = migrate_gen2_bindings(&configs).unwrap();
        assert_eq!(
            migrated[0].binding.operations.build.as_ref().unwrap().mode,
            BuildMode::OneShot
        );
    }

    /// `refinement` mode is a typed migrate error — the vocabulary is deleted.
    #[test]
    fn refinement_mode_is_a_typed_error() {
        let configs = PipelineConfigs {
            projections: vec![projection("m", "p", &[], &[], "m")],
            ingests: vec![ingest("i", "m/p", LegacyIngestMode::Refinement, &[])],
            ..Default::default()
        };
        let err = migrate_gen2_bindings(&configs).unwrap_err();
        assert!(
            matches!(err, BindingMigrateError::RefinementModeDeleted { ref ingest } if ingest == "i"),
            "got {err:?}"
        );
    }

    /// A dangling ingest→projection ref is a typed error, never a silent drop.
    #[test]
    fn dangling_projection_ref_is_a_typed_error() {
        let configs = PipelineConfigs {
            projections: vec![projection("m", "other", &[], &[], "m")],
            ingests: vec![ingest("i", "m/missing", LegacyIngestMode::Discovery, &[])],
            ..Default::default()
        };
        let err = migrate_gen2_bindings(&configs).unwrap_err();
        match err {
            BindingMigrateError::DanglingProjectionRef {
                ingest,
                projection_ref,
                mem,
                available,
            } => {
                assert_eq!(ingest, "i");
                assert_eq!(projection_ref, "m/missing");
                assert_eq!(mem, "m");
                assert_eq!(available, vec!["other".to_string()]);
            }
            other => panic!("expected DanglingProjectionRef, got {other:?}"),
        }
    }

    /// A malformed projection ref (no `/`) is a typed error.
    #[test]
    fn malformed_projection_ref_is_a_typed_error() {
        let configs = PipelineConfigs {
            ingests: vec![ingest("i", "noslash", LegacyIngestMode::Discovery, &[])],
            ..Default::default()
        };
        let err = migrate_gen2_bindings(&configs).unwrap_err();
        assert!(
            matches!(err, BindingMigrateError::MalformedProjectionRef { .. }),
            "got {err:?}"
        );
    }

    /// A legal codebase binding resolves and validates clean against the D6
    /// matrix.
    #[test]
    fn migrated_codebase_binding_validates_clean() {
        let configs = PipelineConfigs {
            mediums: vec![medium("engine", "src", MediumType::Codebase, "../public")],
            facets: vec![facet("engine", "source-tree", "src", None)],
            projections: vec![projection(
                "engine",
                "graph",
                &["source-tree"],
                &[],
                "engine",
            )],
            ingests: vec![ingest(
                "engine-graph",
                "engine/graph",
                LegacyIngestMode::Discovery,
                &["../public/target/**"],
            )],
        };
        let migrated = migrate_gen2_bindings(&configs).unwrap();
        let m = &migrated[0];
        let resolved = resolve_migrated_binding(&configs, &m.id, m.binding.clone()).unwrap();
        assert!(validate_binding(&resolved).is_ok());
    }

    /// A migrated binding whose facet declares a preparation surfaces the D6
    /// capability refusal at validation.
    #[test]
    fn migrated_binding_with_preparation_surfaces_capability_refusal() {
        let configs = PipelineConfigs {
            mediums: vec![medium("docs", "manuals", MediumType::Filesystem, "../docs")],
            facets: vec![facet("docs", "pages", "manuals", Some("pdf-to-markdown"))],
            projections: vec![projection("docs", "manual", &["pages"], &[], "docs")],
            ingests: vec![ingest(
                "docs-manual",
                "docs/manual",
                LegacyIngestMode::Discovery,
                &[],
            )],
        };
        let migrated = migrate_gen2_bindings(&configs).unwrap();
        let m = &migrated[0];
        let resolved = resolve_migrated_binding(&configs, &m.id, m.binding.clone()).unwrap();
        let errs = validate_binding(&resolved).unwrap_err();
        assert!(
            errs.iter().any(|e| matches!(
                e,
                CapabilityError::PreparationUnsupported { preparation, .. }
                    if preparation == "pdf-to-markdown"
            )),
            "expected PreparationUnsupported, got {errs:?}"
        );
    }
}
