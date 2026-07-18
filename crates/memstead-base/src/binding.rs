//! Binding format **v2** — one record per pipeline.
//!
//! This is the **live** binding shape: [`crate::pipeline_store::load_pipeline_configs`]
//! reads it (version-gated), the `projection` CLI tree writes it, and the
//! resolve / brief / status / advance paths consume it. A v2 [`Binding`]
//! alone fully defines a pipeline: intent, **inline sources** (each carrying
//! what the retired standalone medium + facet records carried), reference
//! mems, destination, deny paths, coverage semantics, and operations. The
//! 2026-07 consolidation (operator directive, 2026-07-18) removed the
//! three-file store: the engine reads only this format; `memstead projection
//! migrate` converts prior generations, and there is no compatibility layer.
//!
//! Three things live here:
//!
//! 1. [`Binding`] — the versioned record: one file per pipeline, collapsing
//!    the medium / facet / binding split into a single record with inline
//!    [`Source`] entries and an `operations { build, sync, verify }` block.
//! 2. [`hash_binding`] — `hash(D)`: the lowercase-hex SHA-256 of the
//!    canonical JSON of the binding's *content-defining* projection.
//!    Scheduling knobs (`trigger` / `batch_size` / `post_actions`, the
//!    sync/verify blocks, prune) are excluded by construction; a source's
//!    selection pattern or pointer changing — now inputs *inside* the one
//!    record — changes the hash.
//! 3. [`medium_capabilities`] + [`validate_binding`] — the medium-capability
//!    matrix (the medium *half* of a source description keeps the medium
//!    vocabulary) and the validation entry point: capability refusals plus
//!    in-record source validation (empty / duplicate source names).
//!
//! The findings store ([`crate::ingest::findings`]) keys on `hash(D)`, so the
//! consolidation's shape change invalidates prior findings by construction —
//! accepted and disclosed (findings are re-derivable measurements).

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::pipeline::{IngestTrigger, MediumType, PatternEntry, Source};

/// The current binding format version. A v2 binding carries `version: 2`.
pub const BINDING_VERSION: u32 = 2;

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
// The v2 record
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
/// from the vocabulary** — it is neither a variant here nor migrated, so
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

/// Default per-run tier-3 adjudication cap (bundle plan `05-verify-sync-engine`,
/// D1/D4). Dogfood-tuned against the live `engine/graph` binding (524 source
/// artifacts): a fully-drifted mem of that scale clears its adjudication backlog
/// in ~11 verify runs while each run's asserted-drift work stays bounded and its
/// token cost predictable. `0` disables the cap (adjudicate every candidate).
pub const DEFAULT_ADJUDICATION_CAP: u32 = 50;

/// Default `full_resync_every` (bundle plan `05-verify-sync-engine`, D3/D4):
/// fire a guaranteed full-enumeration coverage sweep every N verify runs.
/// Dogfood-tuned against `engine/graph` (524 artifacts, sample batch 20 → a
/// rotation completes in ~27 runs): a sweep every 20 runs guarantees a complete
/// coverage picture without waiting on the rotation to happen to finish. `0`
/// disables scheduled full walks (rotating sample only).
pub const DEFAULT_FULL_RESYNC_EVERY: u32 = 20;

fn default_adjudication_cap() -> u32 {
    DEFAULT_ADJUDICATION_CAP
}

fn default_full_resync_every() -> u32 {
    DEFAULT_FULL_RESYNC_EVERY
}

/// The **verify** operation — read-only measurement. Optional: an absent
/// `verify` block means engine defaults, never a refusal (verify is
/// read-only). Carries no mode.
///
/// `adjudication_cap` and `full_resync_every` are the tier-3 operations knobs
/// (bundle plan `05-verify-sync-engine`, group D): scheduling attributes on the
/// measurement side only — like `trigger` / `batch_size`, they never change what
/// the mem claims, so they are excluded from [`hash_binding`] (the whole
/// `verify` block is). Both are additive: an older `verify` block without them
/// deserializes to the dogfood-tuned defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyOperation {
    /// What sets a verify running.
    pub trigger: IngestTrigger,
    /// How many artifacts a single run processes.
    pub batch_size: u32,
    /// Per-run tier-3 adjudication cap: the maximum number of hash-drift
    /// adjudications a single verify run asserts. Once the cap is reached the
    /// run **stops adjudicating** and queues the remaining drift candidates as
    /// `queued-for-adjudication` findings (the tier-3 backlog the fidelity
    /// report renders). Combined with the rotating sample, successive runs
    /// adjudicate different windows, so the whole anchor set is covered over a
    /// full rotation. `0` disables the cap. Defaults to
    /// [`DEFAULT_ADJUDICATION_CAP`].
    #[serde(default = "default_adjudication_cap")]
    pub adjudication_cap: u32,
    /// Scheduled full-enumeration walk cadence: every N verify runs, a full
    /// coverage sweep enumerates the whole source set (`S(D)`) for **enumerable**
    /// mediums, guaranteeing eventual complete coverage rather than relying on
    /// the rotating sample to finish. For a medium the capability matrix marks
    /// **non-enumerable**, the scheduled walk refuses with a typed signal — never
    /// a silent skip, never a fabricated full-coverage claim. `0` disables
    /// scheduled full walks. Defaults to [`DEFAULT_FULL_RESYNC_EVERY`].
    #[serde(default = "default_full_resync_every")]
    pub full_resync_every: u32,
}

/// The prune guarantee a binding **requests** (bundle plan
/// `05-verify-sync-engine`, F1). Prune produces deletion **proposals** surfaced
/// in the sync brief (it never mutates the mem); the guarantee governs how a
/// prune proposal treats a model-side edit that races a source removal.
///
/// The guarantee a medium can *support* is derived from its base-leg
/// retrievability ([`prune_guarantee_for_medium`]): a git-backed source can
/// retrieve the base leg for a real three-way merge ([`Self::NeverClobber`]);
/// everything else degrades to conflict-flagging ([`Self::ConflictFlag`]).
/// Requesting a guarantee the medium cannot support is refused at
/// **binding-validation** time (never at run time) via
/// [`CapabilityError::PruneGuaranteeUnsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PruneGuarantee {
    /// Full never-clobber three-way merge — only where the source **base leg is
    /// retrievable** (git-backed sources). The retrieved base lets the merge
    /// tell a model-side edit apart from a clean removal, so a divergence is
    /// never silently proposed as a clean delete.
    NeverClobber,
    /// Conflict-flag degradation (the default — always supportable): where the
    /// base leg is **not** retrievable, prune presents **both** sides and never
    /// auto-writes over a model-side edit. The decided posture for non-git
    /// sources (span-snapshot base legs are out of scope — no current payer).
    #[default]
    ConflictFlag,
}

impl PruneGuarantee {
    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            PruneGuarantee::NeverClobber => "never-clobber",
            PruneGuarantee::ConflictFlag => "conflict-flag",
        }
    }
}

/// The **prune** configuration of a [`Binding`] (F1) — additive, optional. An
/// absent `prune` block means prune is not enabled for the binding (no deletion
/// proposals are produced). Prune has no independent schedule: it rides the sync
/// brief (the sole maintenance-writer channel), so it carries no `trigger` /
/// `batch_size` — only the requested [`PruneGuarantee`]. Like the `sync` /
/// `verify` blocks it is **excluded from [`hash_binding`]**: a maintenance
/// policy never changes what the mem claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneConfig {
    /// The guarantee level the binding requests. Validated against the medium's
    /// base-leg retrievability at binding-validation time (F1 refusal).
    /// Defaults to [`PruneGuarantee::ConflictFlag`] when absent.
    #[serde(default)]
    pub guarantee: PruneGuarantee,
}

/// The operations block of a [`Binding`]: every operation is **optional**.
/// An absent `build` / `sync` block makes that *mutating* operation
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

/// A **binding**, format version 2 — one record per pipeline. The single
/// versioned file at `projections/<mem>/<name>.json` that alone fully defines
/// the obligation: `intent`, inline [`Source`] entries (each carrying the
/// medium and facet halves the retired standalone records held),
/// `reference_mems`, `destination_mem`, `deny_paths`, `coverage_semantics`,
/// `rules`, `prune`, and the `operations { build, sync, verify }` block.
///
/// This is the live store record — [`crate::pipeline_store::load_pipeline_configs`]
/// reads it version-gated and the `projection` CLI tree writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// Format version — required. v2 is [`BINDING_VERSION`]. A projection file
    /// without it (or with a prior version) is refused by the loader with a
    /// typed error naming `memstead projection migrate`.
    pub version: u32,
    /// What the binding is trying to accomplish — prose for the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// The inline sources the binding consumes, in declaration order.
    /// Each `name` is unique within the record and keys per-source state.
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Read-only reference mems that supply cross-mem context.
    #[serde(default)]
    pub reference_mems: Vec<String>,
    /// The mem this binding writes into.
    pub destination_mem: String,
    /// Paths excluded from the binding's scope (workspace-relative globs).
    #[serde(default)]
    pub deny_paths: Vec<String>,
    /// Whether the binding claims exhaustive or curated coverage.
    #[serde(default)]
    pub coverage_semantics: CoverageSemantics,
    /// Free-form binding rules (e.g. a one-shot lens `routing` string).
    /// Opaque to the engine — consumed only by the one-shot brief renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<serde_json::Value>,
    /// The **prune** policy (bundle plan `05-verify-sync-engine`, F1) — additive,
    /// optional. Absent = prune disabled (no deletion proposals). Present = prune
    /// produces deletion proposals in the sync brief under the requested
    /// [`PruneGuarantee`], validated against the medium's base-leg
    /// retrievability at binding-validation time. Excluded from [`hash_binding`]
    /// (a maintenance policy, not content-defining).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prune: Option<PruneConfig>,
    /// The operations this binding declares (build required; sync/verify optional).
    pub operations: Operations,
}

// ---------------------------------------------------------------------------
// hash(D)
// ---------------------------------------------------------------------------

/// One source's content-defining projection, in a fixed serde shape so
/// [`hash_binding`] hashes every content input. Private — the hash is the
/// only consumer.
#[derive(Serialize)]
struct HashSource<'a> {
    source: &'a str,
    patterns: &'a [PatternEntry],
    preparation: &'a Option<String>,
    preparation_impl_version: u32,
    medium_type: MediumType,
    pointer: &'a str,
    change_detection: &'a Option<String>,
}

/// The content-defining projection of a binding, in a fixed serde shape.
/// Private — serialized to canonical JSON for hashing. Excludes `trigger`,
/// `batch_size`, `post_actions`, and the `sync` / `verify` / `prune` blocks:
/// scheduling and maintenance policy never change what the mem claims. The
/// `engagement` slot is likewise excluded (an engagement contract shapes how
/// an agent works, not what the mem claims — the pre-consolidation exclusion
/// carried forward).
#[derive(Serialize)]
struct HashInput<'a> {
    version: u32,
    intent: &'a Option<String>,
    sources: Vec<HashSource<'a>>,
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

/// Compute `hash(D)` — the lowercase-hex SHA-256 of the canonical JSON of a
/// binding's content-defining projection.
///
/// Hashed: `version`, `intent`, `sources` (per source: its name, selection
/// patterns, preparation identifier + [`PREPARATION_IMPL_VERSION`], and its
/// medium half's `type` / `pointer` / `change_detection`), `reference_mems`,
/// `destination_mem`, `deny_paths`, `coverage_semantics`, `rules`, and
/// `operations.build.mode`.
///
/// **Excluded:** `trigger`, `batch_size`, `post_actions`, the `sync` /
/// `verify` / `prune` blocks, and each source's `engagement` contract —
/// scheduling, maintenance policy, and engagement style never change what
/// the mem claims. The v2 record needs no external resolution: every content
/// input lives inside the one record, so a selection or pointer edit
/// invalidates the hash — and thus any findings keyed on it — directly.
pub fn hash_binding(binding: &Binding) -> String {
    let sources: Vec<HashSource<'_>> = binding
        .sources
        .iter()
        .map(|s| HashSource {
            source: &s.name,
            patterns: &s.scope,
            preparation: &s.preparation,
            preparation_impl_version: PREPARATION_IMPL_VERSION,
            medium_type: s.medium_type,
            pointer: &s.pointer,
            change_detection: &s.change_detection,
        })
        .collect();

    let input = HashInput {
        version: binding.version,
        intent: &binding.intent,
        sources,
        reference_mems: &binding.reference_mems,
        destination_mem: &binding.destination_mem,
        deny_paths: &binding.deny_paths,
        coverage_semantics: binding.coverage_semantics,
        rules: &binding.rules,
        build_mode: binding.operations.build.as_ref().map(|b| b.mode),
    };

    let value = serde_json::to_value(&input).expect("hash input serializes to a JSON value");
    let canonical = canonical_json(&value);
    let digest = Sha256::digest(canonical.as_bytes());
    crate::hex_lower(&digest)
}

// ---------------------------------------------------------------------------
// Medium-capability matrix + validation
// ---------------------------------------------------------------------------

/// What a medium can support — the row of the capability matrix for a
/// [`MediumType`] (the medium *half* of a source description). Pure data;
/// [`validate_binding`] reads it to refuse operations a medium cannot support.
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

/// The capability-matrix row for a medium type. The single source of
/// truth the fidelity report also renders.
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

/// The strongest prune guarantee a medium can **support** (F1), derived from
/// the capability matrix: a base-leg-retrievable medium (git-backed —
/// codebase / filesystem / git / graph) supports the full never-clobber
/// three-way merge; a non-retrievable medium (`web`) supports only conflict-flag
/// degradation. Validation refuses a request that exceeds this.
pub fn prune_guarantee_for_medium(medium_type: MediumType) -> PruneGuarantee {
    if medium_capabilities(medium_type).base_version_retrievable {
        PruneGuarantee::NeverClobber
    } else {
        PruneGuarantee::ConflictFlag
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

/// A validation-time refusal: a capability the source's medium half cannot
/// support, or a malformed in-record source declaration. Every refusal names
/// the offending source so it is diagnosable without re-reading the store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    /// A source has an empty `name` — the name keys per-source sync/verify
    /// state, so it must be present.
    #[error("a source has an empty name: every source names itself (the name keys its state)")]
    EmptySourceName,
    /// Two sources in the record share a name — per-source state keys would
    /// collide.
    #[error(
        "duplicate source name '{name}': source names are unique within a binding \
         (they key per-source sync/verify state)"
    )]
    DuplicateSourceName {
        /// The colliding name.
        name: String,
    },
    /// A `sync` / `verify` operation is declared over a medium that cannot
    /// support it this cycle (a `web` source — operator decision 7). The
    /// out-of-scope statement is said out loud, never a silent mtime-over-URL.
    #[error(
        "operation '{operation}' is out of scope for source '{source_name}' over a '{medium_type}' \
         medium: this medium has no change signal this cycle (deferred — operator decision 7)"
    )]
    OperationOutOfScope {
        /// The offending operation.
        operation: &'static str,
        /// The source declaring it.
        source_name: String,
        /// The medium type that cannot support the operation.
        medium_type: String,
    },
    /// Glob `deny_paths` are declared over a medium whose namespace is not
    /// path-shaped (`graph`, `web`) — a glob cannot select in that namespace.
    #[error(
        "glob deny_paths are illegal for source '{source_name}' over a '{medium_type}' medium: its \
         '{anchor_namespace}' namespace is not path-shaped"
    )]
    GlobDenyIllegal {
        /// The offending source.
        source_name: String,
        /// The medium type whose namespace is not path-shaped.
        medium_type: String,
        /// That medium's anchor namespace.
        anchor_namespace: &'static str,
    },
    /// A source declares a deterministic preparation step. No preparation
    /// implementation exists ([`PREPARATION_IMPL_VERSION`] is `0`), so any
    /// declared preparation is unsupported — refused at validation time, not
    /// only at render time.
    #[error(
        "source '{source_name}' declares preparation '{preparation}', which has no implementation \
         (preparation impl version {impl_version})"
    )]
    PreparationUnsupported {
        /// The offending source.
        source_name: String,
        /// The declared preparation identifier.
        preparation: String,
        /// The current preparation-implementation version (`0` = none).
        impl_version: u32,
    },
    /// The binding requests a `prune` guarantee the source's medium cannot
    /// support (F1) — `never-clobber` over a medium whose base leg is not
    /// retrievable (`web`). Refused at binding-validation time with the
    /// downgrade remedy, never discovered at run time.
    #[error(
        "prune guarantee '{requested}' is unsupported for source '{source_name}' over a \
         '{medium_type}' medium: its base leg is not retrievable, so only '{supported}' \
         degradation is possible — set the binding's prune guarantee to '{supported}', or \
         point the source at a git-backed medium"
    )]
    PruneGuaranteeUnsupported {
        /// The offending source.
        source_name: String,
        /// The medium type that cannot support the requested guarantee.
        medium_type: String,
        /// The requested guarantee wire string.
        requested: &'static str,
        /// The strongest guarantee this medium supports (the downgrade remedy).
        supported: &'static str,
    },
}

/// Validate a binding against the medium-capability matrix and the in-record
/// source rules, returning **every** refusal (empty `Err` never returned —
/// `Ok` means clean). The v2 record needs no external resolution: everything
/// validated lives inside the one record.
///
/// Refuses:
/// - an empty or duplicate source `name`
///   ([`CapabilityError::EmptySourceName`] /
///   [`CapabilityError::DuplicateSourceName`]) — names key per-source state;
/// - a declared `sync` / `verify` operation over a `web` source
///   ([`CapabilityError::OperationOutOfScope`]);
/// - a glob `deny_paths` list over a non-path-namespace medium
///   ([`CapabilityError::GlobDenyIllegal`]);
/// - any declared source preparation
///   ([`CapabilityError::PreparationUnsupported`]);
/// - a `prune` block requesting `never-clobber` over a non-base-retrievable
///   medium ([`CapabilityError::PruneGuaranteeUnsupported`], F1).
pub fn validate_binding(binding: &Binding) -> Result<(), Vec<CapabilityError>> {
    let mut refusals = Vec::new();
    let has_deny = !binding.deny_paths.is_empty();
    let sync_declared = binding.operations.sync.is_some();
    let verify_declared = binding.operations.verify.is_some();
    // F1: a `prune` block requesting `never-clobber` needs a base-retrievable
    // medium on every source; refuse per-source where it cannot be honoured.
    let requested_prune = binding
        .prune
        .as_ref()
        .map(|p| p.guarantee)
        .filter(|g| *g == PruneGuarantee::NeverClobber);

    let mut seen_names: Vec<&str> = Vec::new();
    for source in &binding.sources {
        if source.name.is_empty() {
            refusals.push(CapabilityError::EmptySourceName);
        } else if seen_names.contains(&source.name.as_str()) {
            refusals.push(CapabilityError::DuplicateSourceName {
                name: source.name.clone(),
            });
        } else {
            seen_names.push(&source.name);
        }

        let caps = medium_capabilities(source.medium_type);
        let medium_type = serde_json::to_value(source.medium_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();

        // A declared preparation is always unsupported (no implementation).
        if let Some(prep) = &source.preparation {
            refusals.push(CapabilityError::PreparationUnsupported {
                source_name: source.name.clone(),
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
                        source_name: source.name.clone(),
                        medium_type: medium_type.clone(),
                    });
                }
            }
        }

        // Glob deny_paths over a non-path-shaped namespace is illegal.
        if has_deny && !caps.glob_deny_legal {
            refusals.push(CapabilityError::GlobDenyIllegal {
                source_name: source.name.clone(),
                medium_type: medium_type.clone(),
                anchor_namespace: caps.anchor_namespace,
            });
        }

        // F1: requested `never-clobber` prune over a non-base-retrievable medium
        // is refused with the downgrade remedy — at validation, not run time.
        if requested_prune.is_some() && !caps.base_version_retrievable {
            refusals.push(CapabilityError::PruneGuaranteeUnsupported {
                source_name: source.name.clone(),
                medium_type: medium_type.clone(),
                requested: PruneGuarantee::NeverClobber.as_wire(),
                supported: prune_guarantee_for_medium(source.medium_type).as_wire(),
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

    fn allow(path: &str) -> PatternEntry {
        PatternEntry {
            path: path.to_string(),
            mode: PatternMode::Allow,
        }
    }

    fn source(
        name: &str,
        medium_type: MediumType,
        pointer: &str,
        scope: Vec<PatternEntry>,
        preparation: Option<&str>,
        change_detection: Option<&str>,
    ) -> Source {
        Source {
            name: name.to_string(),
            medium_type,
            pointer: pointer.to_string(),
            change_detection: change_detection.map(str::to_string),
            scope,
            engagement: None,
            preparation: preparation.map(str::to_string),
        }
    }

    fn codebase_source() -> Source {
        source(
            "source-tree",
            MediumType::Codebase,
            "../public",
            vec![allow("../public/**/*.rs")],
            None,
            None,
        )
    }

    fn binding() -> Binding {
        Binding {
            version: BINDING_VERSION,
            intent: Some("prose for the agent".to_string()),
            sources: vec![codebase_source()],
            reference_mems: vec!["engine".to_string()],
            destination_mem: "plugin".to_string(),
            deny_paths: vec!["VISION.md".to_string(), "dev/**".to_string()],
            coverage_semantics: CoverageSemantics::Exhaustive,
            rules: Some(serde_json::json!({ "routing": "…" })),
            prune: None,
            operations: Operations {
                build: Some(build_op()),
                sync: Some(SyncOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                }),
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                }),
            },
        }
    }

    // ---- Binding serde --------------------------------------------------

    /// A v2 binding round-trips: serialize → deserialize → equal.
    #[test]
    fn binding_round_trips() {
        let b = binding();
        let json = serde_json::to_string(&b).unwrap();
        let back: Binding = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }

    /// The plan's v2 wire example deserializes: inline sources with both
    /// halves, the operations block, and coverage semantics as declared.
    #[test]
    fn plan_shaped_v2_json_deserializes() {
        let src = r#"{
          "version": 2,
          "intent": "prose the building agent reads before every run",
          "sources": [
            {
              "name": "source-tree",
              "type": "codebase",
              "pointer": "../public",
              "change_detection": "auto",
              "scope": [
                { "path": "../public/**/*.rs", "mode": "allow" },
                { "path": "../public/target/**", "mode": "deny" }
              ]
            }
          ],
          "reference_mems": ["engineering"],
          "destination_mem": "engine",
          "deny_paths": ["../dev/**"],
          "coverage_semantics": "exhaustive",
          "operations": {
            "build":  { "mode": "discovery", "trigger": "loop", "batch_size": 20 },
            "sync":   { "trigger": "loop", "batch_size": 20 },
            "verify": { "trigger": "loop", "batch_size": 20,
                        "adjudication_cap": 50, "full_resync_every": 20 }
          }
        }"#;
        let b: Binding = serde_json::from_str(src).unwrap();
        assert_eq!(b.version, 2);
        assert_eq!(b.destination_mem, "engine");
        assert_eq!(b.sources.len(), 1);
        let s = &b.sources[0];
        assert_eq!(s.name, "source-tree");
        assert_eq!(s.medium_type, MediumType::Codebase);
        assert_eq!(s.pointer, "../public");
        assert_eq!(s.change_detection.as_deref(), Some("auto"));
        assert_eq!(s.scope.len(), 2);
        assert_eq!(b.reference_mems, vec!["engineering".to_string()]);
        assert_eq!(b.coverage_semantics, CoverageSemantics::Exhaustive);
        assert_eq!(
            b.operations.build.as_ref().unwrap().mode,
            BuildMode::Discovery
        );
        assert!(b.operations.sync.is_some());
        assert_eq!(b.operations.verify.as_ref().unwrap().adjudication_cap, 50);
    }

    /// `coverage_semantics` defaults to exhaustive when absent, and `one-shot`
    /// is the kebab wire form.
    #[test]
    fn coverage_defaults_and_one_shot_wire_form() {
        let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": { "build": { "mode": "one-shot", "trigger": "manual", "batch_size": 5 } }
        }"#;
        let b: Binding = serde_json::from_str(src).unwrap();
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

    /// The tier-3 knobs are additive: a `verify` block without them
    /// deserializes to the dogfood-tuned defaults, and a block that sets them
    /// round-trips its values.
    #[test]
    fn verify_tier3_knobs_default_and_round_trip() {
        let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": {
            "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 },
            "verify": { "trigger": "manual", "batch_size": 20 }
          }
        }"#;
        let b: Binding = serde_json::from_str(src).unwrap();
        let v = b.operations.verify.as_ref().unwrap();
        assert_eq!(v.adjudication_cap, DEFAULT_ADJUDICATION_CAP);
        assert_eq!(v.full_resync_every, DEFAULT_FULL_RESYNC_EVERY);

        // Explicit values round-trip.
        let explicit = VerifyOperation {
            trigger: IngestTrigger::Manual,
            batch_size: 10,
            adjudication_cap: 7,
            full_resync_every: 3,
        };
        let json = serde_json::to_string(&explicit).unwrap();
        let back: VerifyOperation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, explicit);
        assert!(json.contains("adjudication_cap"));
        assert!(json.contains("full_resync_every"));
    }

    /// The tier-3 scheduling knobs never change `hash(D)` — they are excluded
    /// with the rest of the `verify` block (scheduling never changes the claim).
    #[test]
    fn tier3_knobs_do_not_change_the_hash() {
        let base = hash_binding(&binding());
        let mut tuned = binding();
        let v = tuned.operations.verify.as_mut().unwrap();
        v.adjudication_cap = 999;
        v.full_resync_every = 1;
        assert_eq!(
            base,
            hash_binding(&tuned),
            "tier-3 verify knobs are excluded from hash(D)"
        );
    }

    /// `"mode": "refinement"` is a deleted value — deserialization fails.
    #[test]
    fn refinement_mode_is_rejected() {
        let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": { "build": { "mode": "refinement", "trigger": "loop", "batch_size": 20 } }
        }"#;
        let err = serde_json::from_str::<Binding>(src).unwrap_err();
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
        assert!(serde_json::from_str::<Binding>(src).is_err());
    }

    // ---- hash(D) --------------------------------------------------------

    /// `hash(D)` is stable and recomputable: the same binding hashes
    /// identically, and the digest is 64 lowercase hex chars.
    #[test]
    fn hash_is_stable_and_recomputable() {
        let b = binding();
        let h1 = hash_binding(&b);
        let h2 = hash_binding(&b);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(
            h1.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    /// Changing a source's selection pattern — now an input *inside* the one
    /// record — changes the hash.
    #[test]
    fn changing_a_source_pattern_changes_the_hash() {
        let base = hash_binding(&binding());
        let mut changed = binding();
        changed.sources[0].scope = vec![allow("../public/**/*.md")];
        assert_ne!(base, hash_binding(&changed));
    }

    /// Changing a source's pointer changes the hash.
    #[test]
    fn changing_a_source_pointer_changes_the_hash() {
        let base = hash_binding(&binding());
        let mut changed = binding();
        changed.sources[0].pointer = "../elsewhere".to_string();
        assert_ne!(base, hash_binding(&changed));
    }

    /// Changing `trigger`, `batch_size`, or `post_actions` does **not** change
    /// the hash — scheduling never changes what the mem claims. Neither does a
    /// source's `engagement` contract (the pre-consolidation exclusion carried
    /// forward).
    #[test]
    fn scheduling_knobs_do_not_change_the_hash() {
        let base = hash_binding(&binding());

        let mut b_trigger = binding();
        b_trigger.operations.build.as_mut().unwrap().trigger = IngestTrigger::Manual;
        assert_eq!(base, hash_binding(&b_trigger), "trigger is excluded");

        let mut b_batch = binding();
        b_batch.operations.build.as_mut().unwrap().batch_size = 999;
        assert_eq!(base, hash_binding(&b_batch), "batch_size is excluded");

        let mut b_post = binding();
        b_post.operations.build.as_mut().unwrap().post_actions =
            Some(serde_json::json!({ "archive_source": false }));
        assert_eq!(base, hash_binding(&b_post), "post_actions is excluded");

        // The sync/verify blocks are excluded too.
        let mut b_sync = binding();
        b_sync.operations.sync = None;
        assert_eq!(base, hash_binding(&b_sync), "sync block is excluded");

        // A source's engagement contract is excluded.
        let mut b_engage = binding();
        b_engage.sources[0].engagement = Some(serde_json::json!({ "readVerb": "Study" }));
        assert_eq!(base, hash_binding(&b_engage), "engagement is excluded");
    }

    /// Changing `operations.build.mode` — a content-defining input — **does**
    /// change the hash.
    #[test]
    fn changing_build_mode_changes_the_hash() {
        let base = hash_binding(&binding());
        let mut b = binding();
        b.operations.build.as_mut().unwrap().mode = BuildMode::OneShot;
        assert_ne!(base, hash_binding(&b));
    }

    /// An absent `build` block deserializes (serde default) and still hashes —
    /// the build mode simply does not participate in `hash(D)`.
    #[test]
    fn absent_build_deserializes_and_hashes() {
        let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": { "verify": { "trigger": "manual", "batch_size": 5 } }
        }"#;
        let b: Binding = serde_json::from_str(src).unwrap();
        assert!(b.operations.build.is_none(), "absent build parses to None");
        let h = hash_binding(&b);
        assert_eq!(h.len(), 64);
    }

    // ---- capability matrix + validate -----------------------------------

    /// The matrix rows are unchanged by the consolidation.
    #[test]
    fn capability_matrix_rows() {
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

    /// An empty source name refuses, and a duplicate source name refuses —
    /// per-source state keys must be present and collision-free.
    #[test]
    fn empty_and_duplicate_source_names_refuse() {
        let mut b = binding();
        b.deny_paths.clear();
        b.sources = vec![
            source("", MediumType::Codebase, "../a", vec![], None, None),
            source("dup", MediumType::Codebase, "../b", vec![], None, None),
            source("dup", MediumType::Codebase, "../c", vec![], None, None),
        ];
        let errs = validate_binding(&b).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, CapabilityError::EmptySourceName)),
            "expected EmptySourceName, got {errs:?}"
        );
        assert!(
            errs.iter().any(|e| matches!(
                e,
                CapabilityError::DuplicateSourceName { name } if name == "dup"
            )),
            "expected DuplicateSourceName, got {errs:?}"
        );
    }

    /// `sync` and `verify` over a `web` source each refuse as out-of-scope.
    #[test]
    fn sync_and_verify_over_web_refuse() {
        // Web binding, no deny_paths (globs illegal), no prep — isolate the op refusal.
        let mut b = binding();
        b.deny_paths.clear();
        b.sources = vec![source(
            "web-source",
            MediumType::Web,
            "https://example.com",
            vec![],
            None,
            None,
        )];
        let errs = validate_binding(&b).unwrap_err();
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

    /// Glob `deny_paths` over a `graph` source refuses.
    #[test]
    fn glob_deny_over_graph_refuses() {
        let mut b = binding();
        b.operations.sync = None;
        b.operations.verify = None;
        b.deny_paths = vec!["some/**".to_string()];
        b.sources = vec![source(
            "graph-source",
            MediumType::Graph,
            "home",
            vec![],
            None,
            None,
        )];
        let errs = validate_binding(&b).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, CapabilityError::GlobDenyIllegal { .. })),
            "expected GlobDenyIllegal, got {errs:?}"
        );
    }

    /// A declared source preparation refuses at validation time.
    #[test]
    fn declared_preparation_refuses() {
        let mut b = binding();
        b.operations.sync = None;
        b.operations.verify = None;
        b.deny_paths.clear();
        b.sources = vec![source(
            "manual-pages",
            MediumType::Filesystem,
            "../docs",
            vec![],
            Some("pdf-to-markdown"),
            None,
        )];
        let errs = validate_binding(&b).unwrap_err();
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
            let mut b = binding();
            b.sources = vec![source(
                "f",
                ty,
                "../src",
                vec![allow("../src/**")],
                None,
                None,
            )];
            assert!(
                validate_binding(&b).is_ok(),
                "{ty:?} build+sync+verify should validate clean"
            );
        }
        // graph — build+sync+verify legal, but only without glob deny_paths.
        let mut graph_binding = binding();
        graph_binding.deny_paths.clear();
        graph_binding.sources = vec![source("g", MediumType::Graph, "home", vec![], None, None)];
        assert!(
            validate_binding(&graph_binding).is_ok(),
            "graph build+sync+verify with no glob deny should validate clean"
        );
    }

    // ---- F1: prune guarantee -------------------------------------------

    /// F1 — the `prune` block is additive: a binding without it deserializes
    /// to `prune: None`, and a block that sets a guarantee round-trips
    /// (defaulting to `conflict-flag` when the guarantee is absent).
    #[test]
    fn prune_block_is_additive_and_round_trips() {
        let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": { "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 } }
        }"#;
        let b: Binding = serde_json::from_str(src).unwrap();
        assert!(b.prune.is_none(), "absent prune parses to None");

        // A prune block with no guarantee defaults to conflict-flag.
        let with_default = r#"{
          "version": 2,
          "destination_mem": "m",
          "prune": {},
          "operations": { "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 } }
        }"#;
        let b: Binding = serde_json::from_str(with_default).unwrap();
        assert_eq!(
            b.prune.as_ref().unwrap().guarantee,
            PruneGuarantee::ConflictFlag
        );

        // Explicit never-clobber round-trips.
        let explicit = PruneConfig {
            guarantee: PruneGuarantee::NeverClobber,
        };
        let json = serde_json::to_string(&explicit).unwrap();
        assert!(json.contains("never-clobber"));
        assert_eq!(
            serde_json::from_str::<PruneConfig>(&json).unwrap(),
            explicit
        );
    }

    /// F1 — the `prune` policy never changes `hash(D)` (it is a maintenance
    /// policy, excluded like the sync/verify blocks).
    #[test]
    fn prune_does_not_change_the_hash() {
        let base = hash_binding(&binding());
        let mut pruned = binding();
        pruned.prune = Some(PruneConfig {
            guarantee: PruneGuarantee::NeverClobber,
        });
        assert_eq!(
            base,
            hash_binding(&pruned),
            "prune policy is excluded from hash(D)"
        );
    }

    /// F1 — the strongest guarantee a medium supports is base-leg-retrievability:
    /// git-backed mediums support never-clobber; `web` supports only conflict-flag.
    #[test]
    fn prune_guarantee_per_medium_matches_capability_matrix() {
        for ty in [
            MediumType::Codebase,
            MediumType::Filesystem,
            MediumType::Git,
            MediumType::Graph,
        ] {
            assert_eq!(
                prune_guarantee_for_medium(ty),
                PruneGuarantee::NeverClobber,
                "{ty:?} can retrieve a base leg → never-clobber"
            );
        }
        assert_eq!(
            prune_guarantee_for_medium(MediumType::Web),
            PruneGuarantee::ConflictFlag,
            "web has no retrievable base leg → conflict-flag only"
        );
    }

    /// F1 REFUSAL — requesting `never-clobber` prune over a `web` source (no
    /// retrievable base leg) fails at binding validation with a remedy-bearing
    /// error naming the downgrade, never a runtime surprise.
    #[test]
    fn never_clobber_prune_over_web_refuses_with_remedy() {
        let mut b = binding();
        b.operations.sync = None; // isolate the prune refusal from op-out-of-scope
        b.operations.verify = None;
        b.deny_paths.clear();
        b.prune = Some(PruneConfig {
            guarantee: PruneGuarantee::NeverClobber,
        });
        b.sources = vec![source(
            "web-source",
            MediumType::Web,
            "https://example.com",
            vec![],
            None,
            None,
        )];
        let errs = validate_binding(&b).unwrap_err();
        let refusal = errs
            .iter()
            .find_map(|e| match e {
                CapabilityError::PruneGuaranteeUnsupported {
                    requested,
                    supported,
                    ..
                } => Some((*requested, *supported)),
                _ => None,
            })
            .expect("expected a PruneGuaranteeUnsupported refusal");
        assert_eq!(refusal, ("never-clobber", "conflict-flag"));
        // The message carries the concrete downgrade remedy.
        let msg = errs
            .iter()
            .find(|e| matches!(e, CapabilityError::PruneGuaranteeUnsupported { .. }))
            .unwrap()
            .to_string();
        assert!(
            msg.contains("conflict-flag"),
            "remedy names the downgrade: {msg}"
        );
    }

    /// F1 — `never-clobber` over a git-backed source validates clean, and
    /// `conflict-flag` (the always-supportable degradation) validates clean over
    /// `web` — the guarantee the matrix marks legal is accepted.
    #[test]
    fn prune_guarantee_supported_validates_clean() {
        // never-clobber over codebase — base retrievable, clean.
        let mut nc = binding();
        nc.prune = Some(PruneConfig {
            guarantee: PruneGuarantee::NeverClobber,
        });
        assert!(validate_binding(&nc).is_ok());

        // conflict-flag over web — always supportable (build-only to isolate).
        let mut cf = binding();
        cf.operations.sync = None;
        cf.operations.verify = None;
        cf.deny_paths.clear();
        cf.prune = Some(PruneConfig {
            guarantee: PruneGuarantee::ConflictFlag,
        });
        cf.sources = vec![source(
            "web-source",
            MediumType::Web,
            "https://example.com",
            vec![],
            None,
            None,
        )];
        assert!(validate_binding(&cf).is_ok());
    }

    /// A `web` binding scaffolded build-only (no sync/verify, no deny, no prep)
    /// validates clean — the matrix-filtered default.
    #[test]
    fn web_build_only_validates_clean() {
        let mut b = binding();
        b.operations.sync = None;
        b.operations.verify = None;
        b.deny_paths.clear();
        b.sources = vec![source(
            "web-source",
            MediumType::Web,
            "https://example.com",
            vec![],
            None,
            None,
        )];
        assert!(validate_binding(&b).is_ok());
    }
}
