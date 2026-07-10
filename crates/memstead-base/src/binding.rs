//! Binding format **v1** — the additive format foundation for the projection
//! promotion (bundle plan `03-projection-promotion`, decisions D1/D5/D6).
//!
//! This is the **live** binding shape: [`crate::pipeline_store::load_pipeline_configs`]
//! reads it (version-gated), the `projection` CLI tree writes it, and the
//! resolve / brief / status / advance paths consume it. The legacy
//! four-primitive `Projection` + flat-ingest store is parsed only by the
//! migrate/legacy path (via [`crate::pipeline_store::LegacyIngest`]); the
//! retired `Ingest` / `IngestMode` machinery is gone.
//!
//! Three things live here:
//!
//! 1. [`BindingV1`] — the versioned binding record (D1): one file per
//!    source→mem obligation, collapsing the projection + ingest split into a
//!    single record with an `operations { build, sync, verify }` block.
//! 2. [`hash_binding`] — `hash(D)` (D5): the lowercase-hex SHA-256 of the
//!    canonical JSON of a binding's *content-defining resolved projection*.
//!    Scheduling knobs (`trigger` / `batch_size` / `post_actions`) are
//!    excluded by construction; a facet selection pattern or a medium pointer
//!    changing — inputs *outside* the binding file — changes the hash.
//! 3. [`medium_capabilities`] + [`validate_binding`] — the medium-capability
//!    matrix (D6) and the validation entry point that generalizes the
//!    render-time preparation refusal to binding-validation time.
//!
//! The findings-store key + record (plan 03's schema stub, once here) now live
//! as the real, IO-backed store in [`crate::ingest::findings`] (group A of plan
//! 05): [`crate::ingest::findings::FindingKey`] keys it, `hash(D)` still
//! partitions its keyspace so a declaration edit invalidates prior findings.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::ingest::resolve::ResolvedPrimarySource;
use crate::pipeline::{IngestTrigger, MediumType, PatternEntry};

/// The current binding format version. A v1 binding carries `version: 1`.
pub const BINDING_VERSION: u32 = 1;

/// The engine's current preparation-implementation version — the single
/// source of truth for "which preparation implementation is live".
///
/// No preparation implementation exists yet, so this is `0` ("none"). It
/// nonetheless participates in [`hash_binding`]: a future preparation
/// implementation bumps this constant, which — because the preparation
/// identifier + this version are both hashed — invalidates every prior
/// finding keyed on the old `hash(D)` by construction.
pub const PREPARATION_IMPL_VERSION: u32 = 0;

// ---------------------------------------------------------------------------
// D1 — Binding format v1
// ---------------------------------------------------------------------------

/// Coverage semantics — whether the binding claims to cover *everything* in
/// its declared scope (`exhaustive`) or a deliberately partial slice
/// (`curated`). Defaults to [`CoverageSemantics::Exhaustive`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CoverageSemantics {
    /// Every artifact in scope is expected to be accounted for.
    #[default]
    Exhaustive,
    /// A deliberately partial selection — an unaccounted artifact is
    /// information, not a defect.
    Curated,
}

/// How a [`BuildOperation`] engages its binding. **`refinement` is deleted
/// from the vocabulary** (D1) — it is neither a variant here nor migrated, so
/// deserializing `"mode": "refinement"` fails as an unknown value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildMode {
    /// Build out new coverage.
    Discovery,
    /// A single bounded pass.
    OneShot,
}

/// The **build** operation — the only operation carrying a mode. Grows new
/// coverage (or runs a one-shot lens). `trigger` / `batch_size` /
/// `post_actions` are scheduling attributes, excluded from [`hash_binding`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildOperation {
    /// Discovery / one-shot. The one operation with a mode.
    pub mode: BuildMode,
    /// What sets this operation running (loop / manual / on-event).
    pub trigger: IngestTrigger,
    /// How many artifacts a single run processes.
    pub batch_size: u32,
    /// Free-form post-run actions (e.g. a one-shot `archive_source` flag).
    /// Opaque to the engine — consumed only by the one-shot brief renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_actions: Option<serde_json::Value>,
}

/// The **sync** operation — the (future) sole maintenance writer. Optional: an
/// absent `sync` block makes that *mutating* operation refuse at run time.
/// Carries no mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncOperation {
    /// What sets a sync running.
    pub trigger: IngestTrigger,
    /// How many artifacts a single run processes.
    pub batch_size: u32,
}

/// The **verify** operation — read-only measurement. Optional: an absent
/// `verify` block means engine defaults, never a refusal (verify is
/// read-only). Carries no mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyOperation {
    /// What sets a verify running.
    pub trigger: IngestTrigger,
    /// How many artifacts a single run processes.
    pub batch_size: u32,
}

/// The operations block of a [`BindingV1`]: every operation is **optional**
/// (D1/D6). An absent `build` / `sync` block makes that *mutating* operation
/// refuse at run time with a `projection enable <op>` remedy; an absent
/// `verify` block means engine defaults (verify is read-only — never a
/// refusal). `build` is optional in serde so an absent block yields the
/// remedy-bearing refusal rather than a generic "missing field" parse error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operations {
    /// The build operation (optional — absent = mutating op refuses with the
    /// `projection enable build` remedy at run time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildOperation>,
    /// The sync operation (optional — absent = mutating op refuses with the
    /// `projection enable sync` remedy at run time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncOperation>,
    /// The verify operation (optional — absent = engine defaults, never a refusal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyOperation>,
}

/// A **binding**, format version 1 (D1). One versioned record per source→mem
/// obligation: the projection declaration (`intent`, `source_facets`,
/// `reference_mems`, `destination_mem`, `deny_paths`, `coverage_semantics`,
/// `rules`) plus an `operations { build, sync, verify }` block. Collapses the
/// legacy projection + flat-ingest split into one record.
///
/// This is the live store record — [`crate::pipeline_store::load_pipeline_configs`]
/// reads it version-gated and the `projection` CLI tree writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingV1 {
    /// Format version — required. v1 is [`BINDING_VERSION`]. A projection file
    /// without it is refused by the loader (integration deferred).
    pub version: u32,
    /// What the binding is trying to accomplish — prose for the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Source facets (by name) the binding consumes.
    #[serde(default)]
    pub source_facets: Vec<String>,
    /// Read-only reference mems that supply cross-mem context.
    #[serde(default)]
    pub reference_mems: Vec<String>,
    /// The mem this binding writes into.
    pub destination_mem: String,
    /// Paths excluded from the binding's scope (workspace-relative globs).
    /// Moved **up** from the per-ingest record — strategy-invariant.
    #[serde(default)]
    pub deny_paths: Vec<String>,
    /// Whether the binding claims exhaustive or curated coverage.
    #[serde(default)]
    pub coverage_semantics: CoverageSemantics,
    /// Free-form binding rules (e.g. a one-shot lens `routing` string).
    /// Opaque to the engine — consumed only by the one-shot brief renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<serde_json::Value>,
    /// The operations this binding declares (build required; sync/verify optional).
    pub operations: Operations,
}

// ---------------------------------------------------------------------------
// D5 — hash(D)
// ---------------------------------------------------------------------------

/// A binding joined to its **resolved** primary sources — the shape
/// [`hash_binding`] and [`validate_binding`] consume. `reference_mems` are
/// carried on the [`BindingV1`] itself; only the primary facets need
/// resolving (each facet's selection patterns, preparation, and its medium's
/// type / pointer / change-detection).
///
/// This mirrors the resolution [`crate::ingest::resolve`] performs for the
/// legacy ingest, reusing [`ResolvedPrimarySource`], but is constructed
/// independently for these additive primitives — it is not produced by the
/// live resolve path yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBinding {
    /// The binding declaration.
    pub binding: BindingV1,
    /// The binding's primary sources, resolved (facet + medium), in
    /// `source_facets` order.
    pub primary_sources: Vec<ResolvedPrimarySource>,
}

/// One resolved facet's content-defining projection, in a fixed serde shape so
/// [`hash_binding`] hashes every content input (D5). Private — the hash is the
/// only consumer.
#[derive(Serialize)]
struct HashFacet<'a> {
    facet: &'a str,
    patterns: &'a [PatternEntry],
    preparation: &'a Option<String>,
    preparation_impl_version: u32,
    medium_type: MediumType,
    medium_pointer: &'a str,
    change_detection: &'a Option<String>,
}

/// The content-defining projection of a binding, in a fixed serde shape.
/// Private — serialized to canonical JSON for hashing. Excludes `trigger`,
/// `batch_size`, `post_actions`, and the `sync` / `verify` blocks: scheduling
/// never changes what the mem claims.
#[derive(Serialize)]
struct HashInput<'a> {
    version: u32,
    intent: &'a Option<String>,
    source_facets: Vec<HashFacet<'a>>,
    reference_mems: &'a [String],
    destination_mem: &'a str,
    deny_paths: &'a [String],
    coverage_semantics: CoverageSemantics,
    rules: &'a Option<serde_json::Value>,
    /// The build mode participates in `hash(D)`; an absent build block simply
    /// does not contribute it (skipped from the canonical JSON).
    #[serde(skip_serializing_if = "Option::is_none")]
    build_mode: Option<BuildMode>,
}

/// Serialize a JSON value with **recursively sorted object keys** and no
/// insignificant whitespace — the canonical form. serde_json's map is a
/// sorted `BTreeMap` today; this rebuild makes the canonicalization explicit
/// and robust even if the `preserve_order` feature is ever enabled build-wide.
fn canonical_json(value: &serde_json::Value) -> String {
    fn sorted(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&map[k]));
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sorted).collect())
            }
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).expect("canonical JSON serializes")
}

/// Compute `hash(D)` (D5) — the lowercase-hex SHA-256 of the canonical JSON of
/// a binding's content-defining resolved projection.
///
/// Hashed: `version`, `intent`, `source_facets` **resolved** (per facet: its
/// selection patterns, its preparation identifier + [`PREPARATION_IMPL_VERSION`],
/// and its medium's `type` / `pointer` / `change_detection`), `reference_mems`,
/// `destination_mem`, `deny_paths`, `coverage_semantics`, `rules`, and
/// `operations.build.mode`.
///
/// **Excluded:** `trigger`, `batch_size`, `post_actions`, and future tier
/// knobs — scheduling never changes what the mem claims. Because facet
/// selection and medium pointer participate (inputs *outside* the binding
/// file), a change to either invalidates the hash, and thus any findings
/// keyed on it.
pub fn hash_binding(resolved: &ResolvedBinding) -> String {
    let source_facets: Vec<HashFacet<'_>> = resolved
        .primary_sources
        .iter()
        .map(|p| HashFacet {
            facet: &p.facet_ref,
            patterns: &p.scope,
            preparation: &p.preparation,
            preparation_impl_version: PREPARATION_IMPL_VERSION,
            medium_type: p.medium_type,
            medium_pointer: &p.medium_pointer,
            change_detection: &p.declared_change_detection,
        })
        .collect();

    let input = HashInput {
        version: resolved.binding.version,
        intent: &resolved.binding.intent,
        source_facets,
        reference_mems: &resolved.binding.reference_mems,
        destination_mem: &resolved.binding.destination_mem,
        deny_paths: &resolved.binding.deny_paths,
        coverage_semantics: resolved.binding.coverage_semantics,
        rules: &resolved.binding.rules,
        build_mode: resolved.binding.operations.build.as_ref().map(|b| b.mode),
    };

    let value = serde_json::to_value(&input).expect("hash input serializes to a JSON value");
    let canonical = canonical_json(&value);
    let digest = Sha256::digest(canonical.as_bytes());
    format!("{digest:x}")
}

// ---------------------------------------------------------------------------
// D6 — medium-capability matrix + validation
// ---------------------------------------------------------------------------

/// What a medium can support (D6) — the row of the capability matrix for a
/// [`MediumType`]. Pure data; [`validate_binding`] reads it to refuse
/// operations a medium cannot support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediumCapabilities {
    /// Can the medium's scope be enumerated (`S(D)` computable)?
    pub enumerable: bool,
    /// Does the medium provide a change signal?
    pub change_signal: bool,
    /// Can a base version be retrieved (for three-way merge)?
    pub base_version_retrievable: bool,
    /// The medium's anchor namespace (`path`, `path+commit`, `entity`, `url`).
    pub anchor_namespace: &'static str,
    /// Is a glob `deny_paths` list legal (i.e. is the namespace path-shaped)?
    pub glob_deny_legal: bool,
}

/// The capability-matrix row for a medium type (D6). The single source of
/// truth the fidelity report (E3b) will also render.
pub fn medium_capabilities(medium_type: MediumType) -> MediumCapabilities {
    match medium_type {
        MediumType::Codebase => MediumCapabilities {
            enumerable: true,
            change_signal: true,
            base_version_retrievable: true,
            anchor_namespace: "path",
            glob_deny_legal: true,
        },
        MediumType::Filesystem => MediumCapabilities {
            enumerable: true,
            change_signal: true,
            base_version_retrievable: true,
            anchor_namespace: "path",
            glob_deny_legal: true,
        },
        MediumType::Git => MediumCapabilities {
            enumerable: true,
            change_signal: true,
            base_version_retrievable: true,
            anchor_namespace: "path+commit",
            glob_deny_legal: true,
        },
        MediumType::Graph => MediumCapabilities {
            enumerable: true,
            change_signal: true,
            base_version_retrievable: true,
            anchor_namespace: "entity",
            glob_deny_legal: false,
        },
        MediumType::Web => MediumCapabilities {
            // Web enumeration / change detection / base retrieval are all
            // deferred this cycle (operator decision 7).
            enumerable: false,
            change_signal: false,
            base_version_retrievable: false,
            anchor_namespace: "url",
            glob_deny_legal: false,
        },
    }
}

/// A binding operation subject to capability validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// The sync (maintenance-write) operation.
    Sync,
    /// The verify (measurement) operation.
    Verify,
}

impl Operation {
    /// The lowercase name used in refusal messages.
    fn name(self) -> &'static str {
        match self {
            Operation::Sync => "sync",
            Operation::Verify => "verify",
        }
    }
}

/// A validation-time capability refusal (D6). Sibling to
/// [`crate::ingest::resolve::ResolveError`] (which refuses *dangling*
/// references); this refuses declared operations a medium cannot support.
/// Every refusal names the offending facet/medium so it is diagnosable
/// without re-reading the store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    /// A `sync` / `verify` operation is declared over a medium that cannot
    /// support it this cycle (a `web` medium — operator decision 7). The
    /// out-of-scope statement is said out loud, never a silent mtime-over-URL.
    #[error(
        "operation '{operation}' is out of scope for facet '{facet}' over a '{medium_type}' \
         medium: this medium has no change signal this cycle (deferred — operator decision 7)"
    )]
    OperationOutOfScope {
        /// The offending operation.
        operation: &'static str,
        /// The source facet.
        facet: String,
        /// The medium type that cannot support the operation.
        medium_type: String,
    },
    /// Glob `deny_paths` are declared over a medium whose namespace is not
    /// path-shaped (`graph`, `web`) — a glob cannot select in that namespace.
    #[error(
        "glob deny_paths are illegal for facet '{facet}' over a '{medium_type}' medium: its \
         '{anchor_namespace}' namespace is not path-shaped"
    )]
    GlobDenyIllegal {
        /// The source facet.
        facet: String,
        /// The medium type whose namespace is not path-shaped.
        medium_type: String,
        /// That medium's anchor namespace.
        anchor_namespace: &'static str,
    },
    /// A facet declares a deterministic preparation step. No preparation
    /// implementation exists ([`PREPARATION_IMPL_VERSION`] is `0`), so any
    /// declared preparation is unsupported — refused at validation time, not
    /// only at render time.
    #[error(
        "facet '{facet}' declares preparation '{preparation}', which has no implementation \
         (preparation impl version {impl_version})"
    )]
    PreparationUnsupported {
        /// The source facet.
        facet: String,
        /// The declared preparation identifier.
        preparation: String,
        /// The current preparation-implementation version (`0` = none).
        impl_version: u32,
    },
}

/// Validate a resolved binding against the medium-capability matrix (D6),
/// returning **every** capability refusal (empty `Err` never returned — `Ok`
/// means clean). Generalizes the render-time preparation refusal to
/// binding-validation time.
///
/// Refuses, per D6:
/// - a declared `sync` / `verify` operation over a `web` medium
///   ([`CapabilityError::OperationOutOfScope`]);
/// - a glob `deny_paths` list over a non-path-namespace medium
///   ([`CapabilityError::GlobDenyIllegal`]);
/// - any declared facet preparation
///   ([`CapabilityError::PreparationUnsupported`]).
///
/// A binding whose every declared operation the matrix marks legal validates
/// clean (`Ok(())`). This is a new, callable entry point — it is not yet wired
/// into the live loader / resolve path.
pub fn validate_binding(resolved: &ResolvedBinding) -> Result<(), Vec<CapabilityError>> {
    let mut refusals = Vec::new();
    let has_deny = !resolved.binding.deny_paths.is_empty();
    let sync_declared = resolved.binding.operations.sync.is_some();
    let verify_declared = resolved.binding.operations.verify.is_some();

    for source in &resolved.primary_sources {
        let caps = medium_capabilities(source.medium_type);
        let medium_type = serde_json::to_value(source.medium_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();

        // A declared preparation is always unsupported (no implementation).
        if let Some(prep) = &source.preparation {
            refusals.push(CapabilityError::PreparationUnsupported {
                facet: source.facet_ref.clone(),
                preparation: prep.clone(),
                impl_version: PREPARATION_IMPL_VERSION,
            });
        }

        // sync / verify over a medium with no change signal (web) is out of scope.
        if !caps.change_signal {
            for (declared, op) in [
                (sync_declared, Operation::Sync),
                (verify_declared, Operation::Verify),
            ] {
                if declared {
                    refusals.push(CapabilityError::OperationOutOfScope {
                        operation: op.name(),
                        facet: source.facet_ref.clone(),
                        medium_type: medium_type.clone(),
                    });
                }
            }
        }

        // Glob deny_paths over a non-path-shaped namespace is illegal.
        if has_deny && !caps.glob_deny_legal {
            refusals.push(CapabilityError::GlobDenyIllegal {
                facet: source.facet_ref.clone(),
                medium_type: medium_type.clone(),
                anchor_namespace: caps.anchor_namespace,
            });
        }
    }

    if refusals.is_empty() {
        Ok(())
    } else {
        Err(refusals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::PatternMode;

    // ---- builders -------------------------------------------------------

    fn build_op() -> BuildOperation {
        BuildOperation {
            mode: BuildMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 20,
            post_actions: None,
        }
    }

    fn binding() -> BindingV1 {
        BindingV1 {
            version: BINDING_VERSION,
            intent: Some("prose for the agent".to_string()),
            source_facets: vec!["source-tree".to_string()],
            reference_mems: vec!["engine".to_string()],
            destination_mem: "plugin".to_string(),
            deny_paths: vec!["VISION.md".to_string(), "dev/**".to_string()],
            coverage_semantics: CoverageSemantics::Exhaustive,
            rules: Some(serde_json::json!({ "routing": "…" })),
            operations: Operations {
                build: Some(build_op()),
                sync: Some(SyncOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                }),
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                }),
            },
        }
    }

    fn allow(path: &str) -> PatternEntry {
        PatternEntry {
            path: path.to_string(),
            mode: PatternMode::Allow,
        }
    }

    fn primary(
        facet: &str,
        medium_type: MediumType,
        pointer: &str,
        scope: Vec<PatternEntry>,
        preparation: Option<&str>,
        change_detection: Option<&str>,
    ) -> ResolvedPrimarySource {
        ResolvedPrimarySource {
            facet_ref: facet.to_string(),
            medium: "m".to_string(),
            medium_type,
            medium_pointer: pointer.to_string(),
            declared_change_detection: change_detection.map(str::to_string),
            scope,
            preparation: preparation.map(str::to_string),
        }
    }

    fn resolved(binding: BindingV1, sources: Vec<ResolvedPrimarySource>) -> ResolvedBinding {
        ResolvedBinding {
            binding,
            primary_sources: sources,
        }
    }

    fn one_codebase_source() -> Vec<ResolvedPrimarySource> {
        vec![primary(
            "source-tree",
            MediumType::Codebase,
            "../public",
            vec![allow("../public/**/*.rs")],
            None,
            None,
        )]
    }

    // ---- D1: BindingV1 serde --------------------------------------------

    /// A v1 binding round-trips: serialize → deserialize → equal.
    #[test]
    fn binding_round_trips() {
        let b = binding();
        let json = serde_json::to_string(&b).unwrap();
        let back: BindingV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }

    /// A real-shaped v1 binding JSON (the D1 example) deserializes, with the
    /// operations block and coverage semantics as declared.
    #[test]
    fn real_shaped_v1_json_deserializes() {
        let src = r#"{
          "version": 1,
          "intent": "prose for the agent",
          "source_facets": ["source-tree"],
          "reference_mems": ["engine"],
          "destination_mem": "plugin",
          "deny_paths": ["VISION.md", "dev/**"],
          "coverage_semantics": "exhaustive",
          "rules": { "routing": "…" },
          "operations": {
            "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20, "post_actions": { "archive_source": true } },
            "sync":  { "trigger": "manual", "batch_size": 20 },
            "verify": { "trigger": "manual", "batch_size": 20 }
          }
        }"#;
        let b: BindingV1 = serde_json::from_str(src).unwrap();
        assert_eq!(b.version, 1);
        assert_eq!(b.destination_mem, "plugin");
        assert_eq!(b.coverage_semantics, CoverageSemantics::Exhaustive);
        assert_eq!(
            b.operations.build.as_ref().unwrap().mode,
            BuildMode::Discovery
        );
        assert_eq!(
            b.operations.build.as_ref().unwrap().trigger,
            IngestTrigger::Loop
        );
        assert_eq!(
            b.operations.build.as_ref().unwrap().post_actions,
            Some(serde_json::json!({ "archive_source": true }))
        );
        assert!(b.operations.sync.is_some());
        assert!(b.operations.verify.is_some());
    }

    /// `coverage_semantics` defaults to exhaustive when absent, and `one-shot`
    /// is the kebab wire form.
    #[test]
    fn coverage_defaults_and_one_shot_wire_form() {
        let src = r#"{
          "version": 1,
          "destination_mem": "m",
          "operations": { "build": { "mode": "one-shot", "trigger": "manual", "batch_size": 5 } }
        }"#;
        let b: BindingV1 = serde_json::from_str(src).unwrap();
        assert_eq!(b.coverage_semantics, CoverageSemantics::Exhaustive);
        assert_eq!(
            b.operations.build.as_ref().unwrap().mode,
            BuildMode::OneShot
        );
        assert!(b.operations.sync.is_none());
        assert!(b.operations.verify.is_none());
        // one-shot serializes to the kebab form.
        assert_eq!(
            serde_json::to_string(&BuildMode::OneShot).unwrap(),
            r#""one-shot""#
        );
    }

    /// `"mode": "refinement"` is a deleted value — deserialization fails.
    #[test]
    fn refinement_mode_is_rejected() {
        let src = r#"{
          "version": 1,
          "destination_mem": "m",
          "operations": { "build": { "mode": "refinement", "trigger": "loop", "batch_size": 20 } }
        }"#;
        let err = serde_json::from_str::<BindingV1>(src).unwrap_err();
        assert!(
            err.to_string().contains("refinement") || err.to_string().contains("unknown variant"),
            "unexpected error: {err}"
        );
    }

    /// `version` is required — a projection file without it refuses.
    #[test]
    fn version_is_required() {
        let src = r#"{
          "destination_mem": "m",
          "operations": { "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 } }
        }"#;
        assert!(serde_json::from_str::<BindingV1>(src).is_err());
    }

    // ---- D5: hash(D) ----------------------------------------------------

    /// `hash(D)` is stable and recomputable: the same resolved binding hashes
    /// identically, and the digest is 64 lowercase hex chars.
    #[test]
    fn hash_is_stable_and_recomputable() {
        let r = resolved(binding(), one_codebase_source());
        let h1 = hash_binding(&r);
        let h2 = hash_binding(&r);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(
            h1.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    /// Changing a facet selection pattern (an input *outside* the binding
    /// file) changes the hash.
    #[test]
    fn changing_a_facet_pattern_changes_the_hash() {
        let base = hash_binding(&resolved(binding(), one_codebase_source()));
        let changed = hash_binding(&resolved(
            binding(),
            vec![primary(
                "source-tree",
                MediumType::Codebase,
                "../public",
                vec![allow("../public/**/*.md")], // different pattern
                None,
                None,
            )],
        ));
        assert_ne!(base, changed);
    }

    /// Changing a medium pointer (an input *outside* the binding file) changes
    /// the hash.
    #[test]
    fn changing_a_medium_pointer_changes_the_hash() {
        let base = hash_binding(&resolved(binding(), one_codebase_source()));
        let changed = hash_binding(&resolved(
            binding(),
            vec![primary(
                "source-tree",
                MediumType::Codebase,
                "../elsewhere", // different pointer
                vec![allow("../public/**/*.rs")],
                None,
                None,
            )],
        ));
        assert_ne!(base, changed);
    }

    /// Changing `trigger`, `batch_size`, or `post_actions` does **not** change
    /// the hash — scheduling never changes what the mem claims.
    #[test]
    fn scheduling_knobs_do_not_change_the_hash() {
        let base = hash_binding(&resolved(binding(), one_codebase_source()));

        let mut b_trigger = binding();
        b_trigger.operations.build.as_mut().unwrap().trigger = IngestTrigger::Manual;
        assert_eq!(
            base,
            hash_binding(&resolved(b_trigger, one_codebase_source())),
            "trigger is excluded"
        );

        let mut b_batch = binding();
        b_batch.operations.build.as_mut().unwrap().batch_size = 999;
        assert_eq!(
            base,
            hash_binding(&resolved(b_batch, one_codebase_source())),
            "batch_size is excluded"
        );

        let mut b_post = binding();
        b_post.operations.build.as_mut().unwrap().post_actions =
            Some(serde_json::json!({ "archive_source": false }));
        assert_eq!(
            base,
            hash_binding(&resolved(b_post, one_codebase_source())),
            "post_actions is excluded"
        );

        // The sync/verify blocks are excluded too.
        let mut b_sync = binding();
        b_sync.operations.sync = None;
        assert_eq!(
            base,
            hash_binding(&resolved(b_sync, one_codebase_source())),
            "sync block is excluded"
        );
    }

    /// Changing `operations.build.mode` — a content-defining input — **does**
    /// change the hash.
    #[test]
    fn changing_build_mode_changes_the_hash() {
        let base = hash_binding(&resolved(binding(), one_codebase_source()));
        let mut b = binding();
        b.operations.build.as_mut().unwrap().mode = BuildMode::OneShot;
        assert_ne!(base, hash_binding(&resolved(b, one_codebase_source())));
    }

    /// An absent `build` block deserializes (serde default) and still hashes —
    /// the build mode simply does not participate in `hash(D)` (D1/AC4).
    #[test]
    fn absent_build_deserializes_and_hashes() {
        let src = r#"{
          "version": 1,
          "destination_mem": "m",
          "operations": { "verify": { "trigger": "manual", "batch_size": 5 } }
        }"#;
        let b: BindingV1 = serde_json::from_str(src).unwrap();
        assert!(b.operations.build.is_none(), "absent build parses to None");
        // Hashes without panicking; build_mode is omitted from the canonical JSON.
        let h = hash_binding(&resolved(b, one_codebase_source()));
        assert_eq!(h.len(), 64);
    }

    // ---- D6: capability matrix + validate -------------------------------

    /// The matrix rows match D6's table.
    #[test]
    fn capability_matrix_matches_d6_table() {
        let web = medium_capabilities(MediumType::Web);
        assert!(!web.enumerable && !web.change_signal && !web.base_version_retrievable);
        assert!(!web.glob_deny_legal);
        assert_eq!(web.anchor_namespace, "url");

        let graph = medium_capabilities(MediumType::Graph);
        assert!(graph.enumerable && graph.change_signal && graph.base_version_retrievable);
        assert!(!graph.glob_deny_legal, "graph namespace is not path-shaped");
        assert_eq!(graph.anchor_namespace, "entity");

        for ty in [
            MediumType::Codebase,
            MediumType::Filesystem,
            MediumType::Git,
        ] {
            let c = medium_capabilities(ty);
            assert!(c.enumerable && c.change_signal && c.base_version_retrievable);
            assert!(c.glob_deny_legal, "{ty:?} allows glob deny_paths");
        }
        assert_eq!(
            medium_capabilities(MediumType::Git).anchor_namespace,
            "path+commit"
        );
    }

    /// `sync` and `verify` over a `web` medium each refuse as out-of-scope.
    #[test]
    fn sync_and_verify_over_web_refuse() {
        // Web binding, no deny_paths (globs illegal), no prep — isolate the op refusal.
        let mut b = binding();
        b.deny_paths.clear();
        let sources = vec![primary(
            "web-facet",
            MediumType::Web,
            "https://example.com",
            vec![],
            None,
            None,
        )];
        let errs = validate_binding(&resolved(b, sources)).unwrap_err();
        let ops: Vec<&str> = errs
            .iter()
            .filter_map(|e| match e {
                CapabilityError::OperationOutOfScope { operation, .. } => Some(*operation),
                _ => None,
            })
            .collect();
        assert!(ops.contains(&"sync"), "sync refused: {errs:?}");
        assert!(ops.contains(&"verify"), "verify refused: {errs:?}");
    }

    /// Glob `deny_paths` over a `graph` medium refuses.
    #[test]
    fn glob_deny_over_graph_refuses() {
        // Graph binding with build-only (avoid the change-signal check; graph
        // *does* have a change signal anyway) and a glob deny list.
        let mut b = binding();
        b.operations.sync = None;
        b.operations.verify = None;
        b.deny_paths = vec!["some/**".to_string()];
        let sources = vec![primary(
            "graph-facet",
            MediumType::Graph,
            "home",
            vec![],
            None,
            None,
        )];
        let errs = validate_binding(&resolved(b, sources)).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, CapabilityError::GlobDenyIllegal { .. })),
            "expected GlobDenyIllegal, got {errs:?}"
        );
    }

    /// A declared facet preparation refuses at validation time.
    #[test]
    fn declared_preparation_refuses() {
        let mut b = binding();
        b.operations.sync = None;
        b.operations.verify = None;
        b.deny_paths.clear();
        let sources = vec![primary(
            "manual-pages",
            MediumType::Filesystem,
            "../docs",
            vec![],
            Some("pdf-to-markdown"),
            None,
        )];
        let errs = validate_binding(&resolved(b, sources)).unwrap_err();
        assert!(
            errs.iter().any(|e| matches!(
                e,
                CapabilityError::PreparationUnsupported { preparation, .. } if preparation == "pdf-to-markdown"
            )),
            "expected PreparationUnsupported, got {errs:?}"
        );
    }

    /// Every combination the matrix marks legal validates clean:
    /// codebase / filesystem / git / graph bindings with build+sync+verify all
    /// pass (graph carries no glob deny_paths, none carry preparation).
    #[test]
    fn legal_combinations_validate_clean() {
        // codebase / filesystem / git — path-shaped, deny_paths legal.
        for ty in [
            MediumType::Codebase,
            MediumType::Filesystem,
            MediumType::Git,
        ] {
            let sources = vec![primary(
                "f",
                ty,
                "../src",
                vec![allow("../src/**")],
                None,
                None,
            )];
            assert!(
                validate_binding(&resolved(binding(), sources)).is_ok(),
                "{ty:?} build+sync+verify should validate clean"
            );
        }
        // graph — build+sync+verify legal, but only without glob deny_paths.
        let mut graph_binding = binding();
        graph_binding.deny_paths.clear();
        let graph_sources = vec![primary("g", MediumType::Graph, "home", vec![], None, None)];
        assert!(
            validate_binding(&resolved(graph_binding, graph_sources)).is_ok(),
            "graph build+sync+verify with no glob deny should validate clean"
        );
    }

    /// A `web` binding scaffolded build-only (no sync/verify, no deny, no prep)
    /// validates clean — the matrix-filtered default.
    #[test]
    fn web_build_only_validates_clean() {
        let mut b = binding();
        b.operations.sync = None;
        b.operations.verify = None;
        b.deny_paths.clear();
        let sources = vec![primary(
            "web-facet",
            MediumType::Web,
            "https://example.com",
            vec![],
            None,
            None,
        )];
        assert!(validate_binding(&resolved(b, sources)).is_ok());
    }
}
