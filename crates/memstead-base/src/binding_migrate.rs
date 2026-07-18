//! Migration to binding **v2** — the single-record pipeline format.
//!
//! Two conversion legs live here, both pure and IO-free (the CLI's
//! `memstead projection migrate` wraps them with the load / write /
//! tree-removal IO):
//!
//! 1. **Gen-2 → v2** ([`migrate_gen2_bindings`]): each flat legacy ingest is
//!    merged into the projection its `projection` ref names, and the
//!    projection's facet references are **folded inline** — each referenced
//!    facet joins its medium and becomes one [`Source`] under the facet's
//!    name, byte-verbatim (source names key sync watermarks).
//! 2. **v1 → v2** ([`fold_v1_binding`]): a three-file-store binding
//!    ([`LegacyBindingV1`], `source_facets` by name) folds its referenced
//!    facets + mediums inline the same way; every other field carries over
//!    verbatim and `version` becomes 2.
//!
//! After both legs every medium/facet record must be **consumed**
//! ([`check_all_consumed`]) — an orphan (a record no binding references) is a
//! typed error, never a silent drop; the CLI removes the emptied `mediums/`
//! and `facets/` trees only after that check passes.
//!
//! A **dangling reference** (ingest→projection, projection→facet,
//! facet→medium) is a typed migrate error, never a silent drop. `refinement`
//! build mode is a typed error — the vocabulary is deleted, not migrated.

use serde::Deserialize;

use crate::binding::{
    BINDING_VERSION, Binding, BuildMode, BuildOperation, CoverageSemantics, Operations,
    PruneConfig,
};
use crate::pipeline::{Projection, Source};
use crate::pipeline_store::{LegacyIngest, LegacyIngestMode, PipelineConfigs};

/// Why a legacy config could not be migrated to a v2 binding. Every variant
/// names the offending record so the failure is diagnosable without
/// re-reading the store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingMigrateError {
    /// The ingest declares build mode `refinement` — deleted from the binding
    /// vocabulary. Not migrated: refinement-as-writer is gone, so there
    /// is no v2 shape to carry it into.
    #[error(
        "ingest '{ingest}' declares build mode 'refinement', which is deleted from the binding \
         vocabulary — refinement-as-writer is gone; re-declare it as a discovery build (plus \
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
    /// A typed error, never a silent drop.
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
    /// A binding/projection references a facet that does not exist in its
    /// mem — the fold cannot inline what is not there.
    #[error(
        "'{owner}' references facet '{facet}' not found in mem '{mem}' (dangling ref — not \
         migrated); available: {}",
        fmt_list(available)
    )]
    DanglingFacetRef {
        /// The binding/projection id holding the reference.
        owner: String,
        /// The missing facet name.
        facet: String,
        /// The mem the facet was looked up in.
        mem: String,
        /// The facet names that do exist in that mem.
        available: Vec<String>,
    },
    /// A facet references a medium that does not exist in its mem.
    #[error(
        "facet '{facet}' (folded for '{owner}') references medium '{medium}' not found in mem \
         '{mem}' (dangling ref — not migrated); available: {}",
        fmt_list(available)
    )]
    DanglingMediumRef {
        /// The binding/projection id whose fold hit the dangling medium.
        owner: String,
        /// The referencing facet.
        facet: String,
        /// The missing medium name.
        medium: String,
        /// The mem the medium was looked up in.
        mem: String,
        /// The medium names that do exist in that mem.
        available: Vec<String>,
    },
    /// After folding every binding, medium/facet records remain that no
    /// binding referenced. Removing the trees would silently drop their
    /// content — refused instead; the operator deletes or binds them first.
    #[error(
        "orphan pipeline records not referenced by any binding: {} — delete them (or bind \
         them) before migrating; the migration removes the mediums/ and facets/ trees only \
         when every record folded into a binding",
        fmt_list(orphans)
    )]
    OrphanRecords {
        /// `mediums/<mem>/<name>` / `facets/<mem>/<name>` style identifiers.
        orphans: Vec<String>,
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

/// The retired **v1 binding** shape (migrate-local): the three-file-store
/// record that referenced facets by name. Parsed only by the v1→v2 fold leg;
/// the live loader refuses `version: 1` files with the migrate-naming error.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LegacyBindingV1 {
    /// Always `1` on disk (the loader routed the file here by that value).
    pub version: u32,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub source_facets: Vec<String>,
    #[serde(default)]
    pub reference_mems: Vec<String>,
    pub destination_mem: String,
    #[serde(default)]
    pub deny_paths: Vec<String>,
    #[serde(default)]
    pub coverage_semantics: CoverageSemantics,
    #[serde(default)]
    pub rules: Option<serde_json::Value>,
    #[serde(default)]
    pub prune: Option<PruneConfig>,
    pub operations: Operations,
}

/// Convert a legacy bare-name `deny_paths` entry to the workspace-relative
/// glob dialect where trivially derivable, else carry it through unchanged
/// (the gen-2 dialect-forward-carry).
///
/// A **bare directory segment** — non-empty, no path separator, no glob
/// metacharacter (`*?[]`), and no `.` extension marker — is rewritten to a
/// recursive-subtree glob `<segment>/**` (so `dev` → `dev/**`). Anything
/// already carrying a `/`, a glob metacharacter, or a `.` (a file like
/// `VISION.md`, already a valid workspace-relative match) is a no-op —
/// carried through. The return value equals the input exactly when nothing
/// changed, so callers can detect (and note) rewrites by comparison.
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

/// Fold a list of facet references (`facet_names`, in the `<mem>` tier) into
/// inline [`Source`]s: each facet joins its medium, the facet's name becomes
/// the source's name **byte-verbatim** (it keys sync watermarks), the
/// medium's `type` / `pointer` / `change_detection` become the medium half,
/// and the facet's `scope` / `engagement` / `preparation` the facet half.
/// `owner` names the folding binding/projection in dangling-ref errors.
fn fold_sources(
    owner: &str,
    facet_names: &[String],
    mem: &str,
    configs: &PipelineConfigs,
) -> Result<Vec<Source>, BindingMigrateError> {
    let mut sources = Vec::with_capacity(facet_names.len());
    for facet_name in facet_names {
        let facet = configs
            .facets
            .iter()
            .find(|r| r.mem == mem && r.name == *facet_name)
            .map(|r| &r.config)
            .ok_or_else(|| BindingMigrateError::DanglingFacetRef {
                owner: owner.to_string(),
                facet: facet_name.clone(),
                mem: mem.to_string(),
                available: configs
                    .facets
                    .iter()
                    .filter(|r| r.mem == mem)
                    .map(|r| r.name.clone())
                    .collect(),
            })?;
        let medium = configs
            .mediums
            .iter()
            .find(|r| r.mem == mem && r.name == facet.medium)
            .map(|r| &r.config)
            .ok_or_else(|| BindingMigrateError::DanglingMediumRef {
                owner: owner.to_string(),
                facet: facet_name.clone(),
                medium: facet.medium.clone(),
                mem: mem.to_string(),
                available: configs
                    .mediums
                    .iter()
                    .filter(|r| r.mem == mem)
                    .map(|r| r.name.clone())
                    .collect(),
            })?;
        sources.push(Source {
            name: facet_name.clone(),
            medium_type: medium.medium_type,
            pointer: medium.pointer.clone(),
            change_detection: medium.change_detection.clone(),
            scope: facet.scope.clone(),
            engagement: facet.engagement.clone(),
            preparation: facet.preparation.clone(),
        });
    }
    Ok(sources)
}

/// A single migrated binding paired with the identity and provenance the CLI
/// needs to write it to disk and report it.
#[derive(Debug, Clone, PartialEq)]
pub struct MigratedBinding {
    /// The canonical binding id `<mem>/<stem>`.
    pub id: String,
    /// The binding's owning mem dir (the `.memstead/projections/<mem>/`
    /// tier the binding file lives under).
    pub mem: String,
    /// The projection file stem (`<stem>` in the binding id).
    pub name: String,
    /// The flat ingest merged into this binding (its file stem), when the
    /// gen-2 leg produced it — used to delete the consumed ingest. Empty for
    /// the v1→v2 fold leg (no ingest involved).
    pub ingest_name: String,
    /// The facet names this binding's fold consumed (the orphan check reads
    /// them; they equal the produced source names).
    pub consumed_facets: Vec<String>,
    /// The produced v2 binding.
    pub binding: Binding,
    /// Human-readable notes about non-identity transforms applied (e.g.
    /// deny-path dialect rewrites). Empty when the migration was verbatim.
    pub notes: Vec<String>,
}

/// Convert one gen-2 (`Ingest` + its `Projection`) into a v2 [`Binding`],
/// folding the projection's facet references inline. Returns the binding
/// plus any per-field transform notes. Pure — no IO. `ingest_name` is used
/// only for error messages.
pub(crate) fn binding_from_gen2(
    ingest_name: &str,
    ingest: &LegacyIngest,
    projection: &Projection,
    projection_ref: &str,
    mem: &str,
    configs: &PipelineConfigs,
) -> Result<(Binding, Vec<String>), BindingMigrateError> {
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

    let sources = fold_sources(projection_ref, &projection.source_facets, mem, configs)?;

    let binding = Binding {
        version: BINDING_VERSION,
        intent: projection.intent.clone(),
        sources,
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

/// Migrate every gen-2 flat ingest in `configs` into a v2 [`MigratedBinding`],
/// keyed by binding id (`<mem>/<stem>`), in id order.
///
/// Ingest-driven ("merge each flat ingest into its projection"): each
/// ingest's `projection` ref is resolved to a projection in that mem, the
/// pair merged, and the projection's facet references folded inline. A
/// malformed ref, a dangling ref, or a `refinement` mode is a typed
/// [`BindingMigrateError`] — the migration refuses rather than dropping
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

        let (binding, notes) = binding_from_gen2(
            &record.name,
            ingest,
            projection,
            &projection_ref,
            &mem,
            configs,
        )?;
        out.push(MigratedBinding {
            id: projection_ref,
            mem,
            name,
            ingest_name: record.name.clone(),
            consumed_facets: projection.source_facets.clone(),
            binding,
            notes,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Fold one v1 three-file-store binding into a v2 [`Binding`]: every field
/// carries over verbatim, `version` becomes 2, and each `source_facets`
/// entry folds its facet + medium inline under the facet's name
/// byte-verbatim (it keys sync watermarks). Pure — no IO.
pub fn fold_v1_binding(
    binding_id: &str,
    mem: &str,
    v1: &LegacyBindingV1,
    configs: &PipelineConfigs,
) -> Result<Binding, BindingMigrateError> {
    let sources = fold_sources(binding_id, &v1.source_facets, mem, configs)?;
    Ok(Binding {
        version: BINDING_VERSION,
        intent: v1.intent.clone(),
        sources,
        reference_mems: v1.reference_mems.clone(),
        destination_mem: v1.destination_mem.clone(),
        deny_paths: v1.deny_paths.clone(),
        coverage_semantics: v1.coverage_semantics,
        rules: v1.rules.clone(),
        prune: v1.prune.clone(),
        operations: v1.operations.clone(),
    })
}

/// Verify every medium/facet record in `configs` was consumed by a fold —
/// `consumed_facets` is the union of every migrated binding's
/// `(mem, facet-name)` pairs. A facet no binding referenced, or a medium no
/// consumed facet engages, is an **orphan**: removing the trees would drop
/// its content silently, so the migration refuses instead
/// ([`BindingMigrateError::OrphanRecords`]) and names each leftover.
pub fn check_all_consumed(
    configs: &PipelineConfigs,
    consumed_facets: &[(String, String)],
) -> Result<(), BindingMigrateError> {
    let mut orphans = Vec::new();
    for facet in &configs.facets {
        if !consumed_facets
            .iter()
            .any(|(mem, name)| *mem == facet.mem && *name == facet.name)
        {
            orphans.push(format!("facets/{}/{}", facet.mem, facet.name));
        }
    }
    for medium in &configs.mediums {
        let engaged = configs.facets.iter().any(|f| {
            f.mem == medium.mem
                && f.config.medium == medium.name
                && consumed_facets
                    .iter()
                    .any(|(mem, name)| *mem == f.mem && *name == f.name)
        });
        if !engaged {
            orphans.push(format!("mediums/{}/{}", medium.mem, medium.name));
        }
    }
    if orphans.is_empty() {
        Ok(())
    } else {
        orphans.sort();
        Err(BindingMigrateError::OrphanRecords { orphans })
    }
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

    fn gen2_configs() -> PipelineConfigs {
        PipelineConfigs {
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
        }
    }

    /// A well-formed gen-2 pair migrates to a v2 binding: the merged
    /// operations (mode/trigger/batch/post_actions), the projection's
    /// declarative fields, and the facet+medium folded inline under the
    /// facet's name byte-verbatim.
    #[test]
    fn migrates_a_well_formed_pair_folding_sources_inline() {
        let migrated = migrate_gen2_bindings(&gen2_configs()).unwrap();
        assert_eq!(migrated.len(), 1);
        let m = &migrated[0];
        assert_eq!(m.id, "engine/graph");
        assert_eq!(m.mem, "engine");
        assert_eq!(m.name, "graph");
        assert_eq!(m.ingest_name, "engine-graph");
        assert_eq!(m.consumed_facets, vec!["source-tree".to_string()]);

        let b = &m.binding;
        assert_eq!(b.version, BINDING_VERSION);
        assert_eq!(b.intent.as_deref(), Some("intent of graph"));
        assert_eq!(b.reference_mems, vec!["plugin".to_string()]);
        assert_eq!(b.destination_mem, "engine");
        assert_eq!(b.coverage_semantics, CoverageSemantics::Exhaustive);
        assert_eq!(b.rules, Some(serde_json::json!({ "routing": "r" })));
        // The fold: one inline source under the facet's name, carrying the
        // medium half (type/pointer) and the facet half (scope).
        assert_eq!(b.sources.len(), 1);
        let s = &b.sources[0];
        assert_eq!(s.name, "source-tree");
        assert_eq!(s.medium_type, MediumType::Codebase);
        assert_eq!(s.pointer, "../public");
        assert_eq!(s.scope.len(), 1);
        assert_eq!(s.engagement, None);
        assert_eq!(s.preparation, None);
        // Operations: build carries the merged schedule; sync/verify absent.
        assert_eq!(
            b.operations.build.as_ref().unwrap().mode,
            BuildMode::Discovery
        );
        assert_eq!(
            b.operations.build.as_ref().unwrap().post_actions,
            Some(serde_json::json!({ "archive_source": true }))
        );
        assert!(b.operations.sync.is_none());
        assert!(b.operations.verify.is_none());
    }

    /// The produced binding round-trips losslessly through serde (the on-disk
    /// promotion is faithful) and validates clean.
    #[test]
    fn produced_binding_round_trips_and_validates() {
        let migrated = migrate_gen2_bindings(&gen2_configs()).unwrap();
        let b = &migrated[0].binding;
        let json = serde_json::to_string(b).unwrap();
        let back: Binding = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, b);
        assert!(validate_binding(b).is_ok());
    }

    /// `deny_paths` move up to the binding; a bare directory segment is
    /// rewritten to the glob dialect (with a note), while glob/`/`/`.`
    /// entries carry through unchanged.
    #[test]
    fn deny_paths_move_up_and_bare_segments_convert() {
        let mut configs = gen2_configs();
        configs.ingests = vec![ingest(
            "engine-graph",
            "engine/graph",
            LegacyIngestMode::Discovery,
            &["dev", "VISION.md", "../public/target/**"],
        )];
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

    /// A projection→facet dangling ref is a typed error at fold time.
    #[test]
    fn dangling_facet_ref_is_a_typed_error() {
        let mut configs = gen2_configs();
        configs.facets.clear();
        let err = migrate_gen2_bindings(&configs).unwrap_err();
        assert!(
            matches!(
                err,
                BindingMigrateError::DanglingFacetRef { ref facet, .. } if facet == "source-tree"
            ),
            "got {err:?}"
        );
    }

    /// A facet→medium dangling ref is a typed error at fold time.
    #[test]
    fn dangling_medium_ref_is_a_typed_error() {
        let mut configs = gen2_configs();
        configs.mediums.clear();
        let err = migrate_gen2_bindings(&configs).unwrap_err();
        assert!(
            matches!(
                err,
                BindingMigrateError::DanglingMediumRef { ref medium, .. } if medium == "src"
            ),
            "got {err:?}"
        );
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

    // ---- v1 → v2 fold ---------------------------------------------------

    fn v1_binding(facets: &[&str]) -> LegacyBindingV1 {
        LegacyBindingV1 {
            version: 1,
            intent: Some("v1 intent".to_string()),
            source_facets: facets.iter().map(|s| s.to_string()).collect(),
            reference_mems: vec!["engineering".to_string()],
            destination_mem: "engine".to_string(),
            deny_paths: vec!["../dev/**".to_string()],
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
                sync: Some(crate::binding::SyncOperation {
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                }),
                verify: None,
            },
        }
    }

    /// The v1→v2 fold carries every field verbatim, bumps `version` to 2,
    /// and inlines each referenced facet + medium as a source under the
    /// facet's name **byte-verbatim** — the watermark-key preservation
    /// contract.
    #[test]
    fn fold_v1_binding_preserves_fields_and_source_names() {
        let configs = gen2_configs();
        let v1 = v1_binding(&["source-tree"]);
        let b = fold_v1_binding("engine/graph", "engine", &v1, &configs).unwrap();
        assert_eq!(b.version, 2);
        assert_eq!(b.intent.as_deref(), Some("v1 intent"));
        assert_eq!(b.reference_mems, vec!["engineering".to_string()]);
        assert_eq!(b.destination_mem, "engine");
        assert_eq!(b.deny_paths, vec!["../dev/**".to_string()]);
        // The operations block carries over whole — sync survives.
        assert!(b.operations.sync.is_some());
        assert!(b.operations.verify.is_none());
        // The fold: facet name preserved byte-verbatim as the source name.
        assert_eq!(b.sources.len(), 1);
        assert_eq!(b.sources[0].name, "source-tree");
        assert_eq!(b.sources[0].pointer, "../public");
    }

    /// A v1 binding referencing a missing facet is a typed fold error.
    #[test]
    fn fold_v1_binding_dangling_facet_errors() {
        let mut configs = gen2_configs();
        configs.facets.clear();
        let v1 = v1_binding(&["source-tree"]);
        let err = fold_v1_binding("engine/graph", "engine", &v1, &configs).unwrap_err();
        assert!(matches!(
            err,
            BindingMigrateError::DanglingFacetRef { ref facet, .. } if facet == "source-tree"
        ));
    }

    /// The raw v1 JSON on disk (the dogfood shape) parses as
    /// [`LegacyBindingV1`] — the fold leg's reader contract.
    #[test]
    fn legacy_v1_json_parses() {
        let src = r#"{
          "version": 1,
          "intent": "Rust engine source.",
          "source_facets": ["source-tree"],
          "reference_mems": ["engineering"],
          "destination_mem": "engine",
          "deny_paths": ["../dev/**"],
          "coverage_semantics": "exhaustive",
          "operations": {
            "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 },
            "sync": { "trigger": "loop", "batch_size": 20 },
            "verify": { "trigger": "loop", "batch_size": 20,
                        "adjudication_cap": 50, "full_resync_every": 20 }
          }
        }"#;
        let v1: LegacyBindingV1 = serde_json::from_str(src).unwrap();
        assert_eq!(v1.version, 1);
        assert_eq!(v1.source_facets, vec!["source-tree".to_string()]);
        assert!(v1.operations.verify.is_some());
    }

    // ---- orphan check ---------------------------------------------------

    /// All records consumed → clean; an unreferenced facet (and its now
    /// unengaged medium) are orphans, named in the typed error.
    #[test]
    fn check_all_consumed_flags_orphans() {
        let mut configs = gen2_configs();
        configs
            .facets
            .push(facet("engine", "stray-facet", "stray-medium", None));
        configs.mediums.push(medium(
            "engine",
            "stray-medium",
            MediumType::Filesystem,
            "../docs",
        ));

        let consumed = vec![("engine".to_string(), "source-tree".to_string())];
        let err = check_all_consumed(&configs, &consumed).unwrap_err();
        match err {
            BindingMigrateError::OrphanRecords { orphans } => {
                assert_eq!(
                    orphans,
                    vec![
                        "facets/engine/stray-facet".to_string(),
                        "mediums/engine/stray-medium".to_string(),
                    ]
                );
            }
            other => panic!("expected OrphanRecords, got {other:?}"),
        }

        // With the stray facet consumed too, everything is clean.
        let consumed_all = vec![
            ("engine".to_string(), "source-tree".to_string()),
            ("engine".to_string(), "stray-facet".to_string()),
        ];
        assert!(check_all_consumed(&configs, &consumed_all).is_ok());
    }

    /// A migrated binding whose facet declares a preparation surfaces the
    /// capability refusal at validation of the folded record.
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
        let errs = validate_binding(&migrated[0].binding).unwrap_err();
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
