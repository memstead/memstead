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
//!    vocabulary) and the validation entry point: capability refusals,
//!    in-record source validation (empty / duplicate source names), and the
//!    preparation-registry check (a declared `preparation` must be one
//!    [`crate::preparation`] knows, over a medium it can apply to).
//!
//! The findings store (`memstead_projection::findings`, the maintenance-loop crate above this one) keys on `hash(D)`, so the
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
/// `3` since the code-map flavour (`code-map`, touchpoint A on path grains)
/// landed; `2` was the delivery flavour (`dated-entries`, touchpoint B); `1`
/// the first registered preparation (`entity-load-bearing`, see
/// [`crate::preparation`]); `0` meant "none". It participates in
/// [`hash_binding`] for every source: because the declared identifier and
/// this version are both hashed, landing or changing an implementation
/// invalidates every prior finding keyed on the old `hash(D)` by
/// construction — the findings store keys on `hash(D)` alone, so the old
/// batch is segregated as superseded and never mixed into the current view
/// (pinned by `ingest::findings`'s
/// `impl_version_bump_invalidates_findings_by_construction`). Bump it once
/// per landed or changed implementation, never per registry entry that
/// merely exists.
pub const PREPARATION_IMPL_VERSION: u32 = 4;

// ---------------------------------------------------------------------------
// The v2 record
// ---------------------------------------------------------------------------

/// Coverage semantics — whether the binding claims to cover *everything* in
/// its declared scope (`exhaustive`) or a deliberately partial slice
/// (`curated`).
///
/// On the [`Binding`] record the field is **optional**: absent means "not
/// stated", which is a different fact from "stated as exhaustive". The
/// effective value is resolved per medium by
/// [`effective_coverage_semantics`] — there is deliberately no `Default`
/// impl, because a default is exactly the silence-as-assertion this
/// design retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageSemantics {
    /// Every artifact in scope is expected to be accounted for.
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

/// Default per-run tier-3 adjudication cap. Tuned against this project's own
/// live `engine/graph` binding (524 source
/// artifacts): a fully-drifted mem of that scale clears its adjudication backlog
/// in ~11 verify runs while each run's asserted-drift work stays bounded and its
/// token cost predictable. `0` disables the cap (adjudicate every candidate).
pub const DEFAULT_ADJUDICATION_CAP: u32 = 50;

/// Default `full_resync_every`:
/// fire a guaranteed full-enumeration coverage sweep every N verify runs.
/// Tuned against this project's own `engine/graph` (524 artifacts, sample batch 20 → a
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

/// The **verify** operation — measurement. Optional: an absent `verify`
/// block means engine defaults, never a refusal (verify has no mutating
/// operation to gate). Mutates no entity, but records findings, backfills
/// observed anchor hashes and writes a `#verified` baseline. Carries no mode.
///
/// `adjudication_cap` and `full_resync_every` are the tier-3 operations knobs:
/// scheduling attributes on the
/// measurement side only — like `trigger` / `batch_size`, they never change what
/// the mem claims, so they are excluded from [`hash_binding`] (the whole
/// `verify` block is). Both are additive: an older `verify` block without them
/// deserializes to the defaults above.
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

/// The **prune** configuration of a [`Binding`] — additive, optional. An
/// absent `prune` block means prune is not enabled for the binding (no deletion
/// proposals are produced); a present block enables it. Prune produces
/// **proposals** only: the source no longer holds the artifacts an entity
/// describes, both sides are shown in the sync brief, and the agent acting on
/// the brief decides. Prune has no independent schedule: it rides the sync
/// brief (the sole maintenance-writer channel), so it carries no `trigger` /
/// `batch_size`, and it carries no guarantee level either: a source artifact
/// and the agent-authored entity about it share no common ancestor a merge
/// could compare, so there is nothing for the engine to decide on the
/// entity's behalf (see the prune module docs). Like the `sync` / `verify`
/// blocks it is **excluded from [`hash_binding`]**: a maintenance policy never
/// changes what the mem claims. The retired `guarantee` key of earlier
/// records is ignored on read; every binding behaves as `conflict-flag`
/// always did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PruneConfig {}

/// The operations block of a [`Binding`]: every operation is **optional**.
/// An absent `build` / `sync` block makes that *mutating* operation
/// refuse at run time with a `projection enable <op>` remedy; an absent
/// `verify` block means engine defaults (verify has no mutating operation to
/// gate — never a refusal). `build` is optional in serde so an absent block yields the
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

/// Default `deny_paths` scaffolded onto a fresh enumerable
/// (`codebase` / `filesystem`) binding: ordinary platform/tooling
/// debris that would otherwise flood a first denominator. A default,
/// not an invariant — the scaffold materialises the list into the
/// binding record, so an author who wants one of these in scope
/// deletes the entry and gets the files back; bindings created before
/// the default existed keep their recorded (empty) list. Engine state
/// (`.memstead/`, `.memstead.cache/`, mount storage) is NOT on this
/// list — its exclusion is unconditional in the strategy layer, never
/// a deletable record entry.
pub const DEFAULT_SCAFFOLD_DENY_PATHS: &[&str] = &[
    "**/.DS_Store",
    "**/.git/**",
    "**/node_modules/**",
    "**/Thumbs.db",
];

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
    /// typed error (`PROJECTION_STORE_LEGACY`).
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
    /// Optional: `None` means **not stated** — a different fact from
    /// "stated as exhaustive". Consumers never read this raw; they read
    /// [`effective_coverage_semantics`], which resolves `None` per
    /// medium (all sources enumerable → exhaustive; any non-enumerable
    /// source → curated). An explicit `exhaustive` over a
    /// non-enumerable source is refused by [`validate_binding`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_semantics: Option<CoverageSemantics>,
    /// Free-form binding rules (e.g. a one-shot lens `routing` string).
    /// Opaque to the engine — consumed only by the one-shot brief renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<serde_json::Value>,
    /// The **prune** policy — additive, optional. Absent = prune disabled (no
    /// deletion proposals). Present = prune produces deletion proposals in the
    /// sync brief; see [`PruneConfig`]. Excluded from [`hash_binding`] (a
    /// maintenance policy, not content-defining).
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
    hash_binding_at_impl_version(binding, PREPARATION_IMPL_VERSION)
}

/// [`hash_binding`] under an explicit preparation-implementation version.
/// The live hash is always [`PREPARATION_IMPL_VERSION`]'s; this exists so a
/// caller can name the hash a prior engine generation keyed its findings on
/// (the invalidation-by-construction pin, a migration report) without
/// re-deriving the canonical projection.
pub fn hash_binding_at_impl_version(binding: &Binding, preparation_impl_version: u32) -> String {
    let sources: Vec<HashSource<'_>> = binding
        .sources
        .iter()
        .map(|s| HashSource {
            source: &s.name,
            patterns: &s.scope,
            preparation: &s.preparation,
            preparation_impl_version,
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
        // The RESOLVED effective value, never the `Option`: a binding
        // over enumerable sources that never declared the field keeps
        // its pre-optionality hash byte-for-byte (resolved
        // `exhaustive` == the old default), so its findings survive. A
        // non-enumerable-source binding that declared nothing rehashes
        // exactly once — correct, its asserted coverage genuinely
        // changed.
        coverage_semantics: effective_coverage_semantics(binding).value,
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
            // not supported: a web source has no change signal.
            enumerable: false,
            change_signal: false,
            base_version_retrievable: false,
            anchor_namespace: "url",
            glob_deny_legal: false,
        },
    }
}

/// The effective coverage of a binding plus its provenance — whether the
/// value was declared by the author or resolved from the sources' media.
/// The fidelity report renders the distinction; every other consumer
/// reads only [`Self::value`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveCoverage {
    /// The coverage every consumer acts on.
    pub value: CoverageSemantics,
    /// `true` when the binding declared the field; `false` when the
    /// value was resolved from the medium capabilities.
    pub declared: bool,
}

/// Resolve a binding's **effective** coverage semantics. A declared value
/// wins (validation has already refused an illegal `exhaustive`). An
/// undeclared value resolves per binding, not per source: all sources on
/// enumerable media → `exhaustive`; at least one non-enumerable source →
/// `curated` — a mixed binding can only honestly claim the weaker of its
/// parts, because coverage is an obligation of the binding as a whole
/// (the artifact that is measured, reported, and keyed).
pub fn effective_coverage_semantics(binding: &Binding) -> EffectiveCoverage {
    if let Some(declared) = binding.coverage_semantics {
        return EffectiveCoverage {
            value: declared,
            declared: true,
        };
    }
    let all_enumerable = binding
        .sources
        .iter()
        .all(|s| medium_capabilities(s.medium_type).enumerable);
    EffectiveCoverage {
        value: if all_enumerable {
            CoverageSemantics::Exhaustive
        } else {
            CoverageSemantics::Curated
        },
        declared: false,
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
    /// A scope pattern is not a compilable glob. Refused at validation so a
    /// malformed pattern never reaches the enumerator, where an all-or-nothing
    /// glob set turned one bad allow into an empty denominator and one bad
    /// deny into no denies at all — both silently.
    #[error(
        "source '{source_name}' declares a scope pattern that is not a valid glob: \
         '{pattern}' ({reason})"
    )]
    MalformedScopePattern {
        /// The source declaring it.
        source_name: String,
        /// The pattern as written.
        pattern: String,
        /// The glob compiler's own reason.
        reason: String,
    },
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
    /// support it (a `web` source has no change signal). The
    /// out-of-scope statement is said out loud, never a silent mtime-over-URL.
    #[error(
        "operation '{operation}' is out of scope for source '{source_name}' over a '{medium_type}' \
         medium: this medium has no change signal"
    )]
    OperationOutOfScope {
        /// The offending operation.
        operation: &'static str,
        /// The source declaring it.
        source_name: String,
        /// The medium type that cannot support the operation.
        medium_type: String,
    },
    /// A `graph` source's scope carries a pattern the entity-namespace
    /// vocabulary does not define. Refused at declaration rather than
    /// silently selecting nothing: a scope that looks like selection but
    /// reaches nothing is the defect this rule exists to prevent.
    #[error(
        "scope pattern '{pattern}' on source '{source_name}' is not a legal entity selector: a \
         graph medium selects entities, not paths — write '*' for the whole mem, \
         'type:<entity_type>', or 'id:<glob>'"
    )]
    GraphScopeNotEntitySelector {
        /// The source declaring it.
        source_name: String,
        /// The offending pattern, verbatim.
        pattern: String,
    },
    /// A source's scope carries a pattern its medium has no vocabulary to
    /// express at all, so nothing anywhere can interpret it. Distinct from
    /// [`Self::GraphScopeNotEntitySelector`], which names the legal forms
    /// because a legal form exists; here there is none, so the only honest
    /// scope is no scope.
    #[error(
        "scope pattern '{pattern}' on source '{source_name}' cannot be interpreted: a \
         '{medium_type}' medium has no scope vocabulary, so the pattern would select \
         nothing while looking like selection — remove the scope rule"
    )]
    ScopeNotInterpretable {
        /// The source declaring it.
        source_name: String,
        /// The offending pattern, verbatim.
        pattern: String,
        /// The medium with no scope vocabulary.
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
    /// A source declares a preparation identifier the engine's preparation
    /// registry ([`crate::preparation`]) does not know. The refusal is
    /// exactly "not in this engine's registry": a registered identifier
    /// validates clean, an unknown one refuses, and the message names the
    /// registered set.
    ///
    /// Raised by [`validate_binding`], which the edit/validate paths call —
    /// NOT `projection init` (which has no `--preparation` flag). The brief
    /// renderer mirrors the same rule for a record that acquired an unknown
    /// identifier by hand (accepted at rest, reported unsupported and
    /// skipped at run time with exit 0; see `GLOSSARY.md` and
    /// `crate::pipeline::Source::preparation`), so both refusal paths carry
    /// one semantics and move together.
    #[error(
        "source '{source_name}' declares preparation '{preparation}', which is not in this \
         engine's preparation registry (registered: {}; preparation impl version {impl_version})",
        crate::preparation::registered_identifiers().join(", ")
    )]
    PreparationUnsupported {
        /// The offending source.
        source_name: String,
        /// The declared preparation identifier.
        preparation: String,
        /// The current preparation-implementation version.
        impl_version: u32,
    },
    /// A registered preparation is declared over a medium whose anchor
    /// namespace admits none of the grains it prepares (`entity-load-bearing`
    /// over a `codebase` source). It would never meet an anchor it applies
    /// to, so the declaration is refused at validation rather than accepted
    /// and silently never applying.
    #[error(
        "source '{source_name}' declares preparation '{preparation}' over a '{medium_type}' \
         medium whose '{anchor_namespace}' anchor namespace admits none of the grains it \
         prepares"
    )]
    PreparationGrainMismatch {
        /// The offending source.
        source_name: String,
        /// The declared (registered) preparation identifier.
        preparation: String,
        /// The medium type it was declared over.
        medium_type: String,
        /// That medium's anchor namespace.
        anchor_namespace: &'static str,
    },
    /// The binding declares `coverage_semantics: exhaustive` while at least
    /// one source sits on a medium whose scope the engine cannot enumerate
    /// (`web`) — `S(D)` is not computable, so exhaustive coverage cannot be
    /// asserted over it. Refused at binding-validation time with `curated`
    /// as the remedy. An *undeclared* field never trips this: it resolves
    /// per medium via [`effective_coverage_semantics`].
    #[error(
        "coverage_semantics 'exhaustive' is unsupported for source '{source_name}' over a \
         '{medium_type}' medium: its scope is not enumerable (S(D) is not computable), so \
         exhaustive coverage cannot be asserted — declare 'curated', or omit the field to \
         resolve per medium"
    )]
    CoverageExhaustiveUnsupported {
        /// The offending source.
        source_name: String,
        /// The medium type whose scope is not enumerable.
        medium_type: String,
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
/// - a source preparation the engine's registry does not know
///   ([`CapabilityError::PreparationUnsupported`]), or a registered one
///   over a medium whose anchor namespace admits none of its grains
///   ([`CapabilityError::PreparationGrainMismatch`]);
/// - a declared `coverage_semantics: exhaustive` over a non-enumerable
///   medium ([`CapabilityError::CoverageExhaustiveUnsupported`]).
pub fn validate_binding(binding: &Binding) -> Result<(), Vec<CapabilityError>> {
    let mut refusals = Vec::new();
    let has_deny = !binding.deny_paths.is_empty();
    let sync_declared = binding.operations.sync.is_some();
    let verify_declared = binding.operations.verify.is_some();
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

        // A scope pattern must compile. Path-shaped mediums only: a graph
        // source's scope entries are entity selectors, a different grammar
        // with its own parser.
        if !matches!(source.medium_type, MediumType::Graph | MediumType::Web) {
            for rule in &source.scope {
                if let Err(e) = globset::Glob::new(&rule.path) {
                    refusals.push(CapabilityError::MalformedScopePattern {
                        source_name: source.name.clone(),
                        pattern: rule.path.clone(),
                        reason: e.to_string(),
                    });
                }
            }
        }

        let caps = medium_capabilities(source.medium_type);
        let medium_type = serde_json::to_value(source.medium_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();

        // A declared preparation must be one the registry knows — the
        // touchpoints consult the registry by identifier, so an unknown one
        // could never be applied — and one that can apply over this medium's
        // anchor namespace.
        if let Some(prep) = &source.preparation {
            match crate::preparation::lookup(prep) {
                None => refusals.push(CapabilityError::PreparationUnsupported {
                    source_name: source.name.clone(),
                    preparation: prep.clone(),
                    impl_version: PREPARATION_IMPL_VERSION,
                }),
                Some(registered)
                    if !crate::preparation::applies_to_namespace(
                        registered,
                        caps.anchor_namespace,
                    ) =>
                {
                    refusals.push(CapabilityError::PreparationGrainMismatch {
                        source_name: source.name.clone(),
                        preparation: prep.clone(),
                        medium_type: medium_type.clone(),
                        anchor_namespace: caps.anchor_namespace,
                    });
                }
                Some(_) => {}
            }
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

        // A scope rule must be one its medium's namespace can express. The
        // engine used to accept any string here and interpret none of them,
        // so `**/*` scaffolded onto a graph facet looked like scope and
        // selected nothing. Refuse the undefined form at declaration.
        //
        // Checked for every medium whose namespace is not path-shaped, not for
        // graph alone: `web` has no selector vocabulary either, and gating on
        // one medium is how the class survived a round — fixed where it had
        // been demonstrated and left standing one row over.
        match source.medium_type {
            MediumType::Graph => {
                for rule in &source.scope {
                    if crate::source_scope::parse_entity_selector(&rule.path).is_none() {
                        refusals.push(CapabilityError::GraphScopeNotEntitySelector {
                            source_name: source.name.clone(),
                            pattern: rule.path.clone(),
                        });
                    }
                }
            }
            MediumType::Web => {
                for rule in &source.scope {
                    refusals.push(CapabilityError::ScopeNotInterpretable {
                        source_name: source.name.clone(),
                        pattern: rule.path.clone(),
                        medium_type: medium_type.clone(),
                    });
                }
            }
            MediumType::Codebase | MediumType::Filesystem | MediumType::Git => {}
        }

        // Glob deny_paths over a non-path-shaped namespace is illegal.
        if has_deny && !caps.glob_deny_legal {
            refusals.push(CapabilityError::GlobDenyIllegal {
                source_name: source.name.clone(),
                medium_type: medium_type.clone(),
                anchor_namespace: caps.anchor_namespace,
            });
        }

        // A declared `exhaustive` over a non-enumerable medium is refused —
        // the engine cannot compute S(D) there, so the claim is unassertable.
        // Fires only on what the author actually wrote (`Some(Exhaustive)`);
        // an undeclared field resolves per medium instead of refusing.
        if binding.coverage_semantics == Some(CoverageSemantics::Exhaustive) && !caps.enumerable {
            refusals.push(CapabilityError::CoverageExhaustiveUnsupported {
                source_name: source.name.clone(),
                medium_type: medium_type.clone(),
            });
        }
    }

    if refusals.is_empty() {
        Ok(())
    } else {
        Err(refusals)
    }
}

/// What a caller wants scaffolded: one binding over one source. Everything
/// else — deny defaults, the capability-matrix filter, the prune block — is
/// the engine's to decide, so that every front door that scaffolds a binding
/// scaffolds the same one.
#[derive(Debug, Clone)]
pub struct ScaffoldParams<'a> {
    /// The mem the binding writes into — the `<mem>` half of the binding id.
    pub destination_mem: &'a str,
    /// The single source's `name` (unique within the record; keys per-source
    /// state). Conventionally the binding stem.
    pub source_name: &'a str,
    /// The medium pointer — workspace-relative path, mem id, or URL.
    pub pointer: &'a str,
    /// The medium type, which decides the capability matrix.
    pub medium_type: MediumType,
    /// Intent prose for the agent, or `None`.
    pub intent: Option<String>,
    /// Deny globs to add beyond [`DEFAULT_SCAFFOLD_DENY_PATHS`], for a caller
    /// that knows something about the tree the engine cannot infer. Materialised
    /// into the record exactly like the defaults — visible, editable, deletable.
    /// Engine state and mount storage locations never belong here: their
    /// exclusion is unconditional in the strategy layer.
    pub additional_deny_paths: Vec<String>,
}

/// A scaffolded binding, ready to write: the record, the operations it ended
/// up declaring, and the warnings the caller must surface.
#[derive(Debug, Clone)]
pub struct ScaffoldedBinding {
    /// The record to write.
    pub binding: Binding,
    /// The operation names the record declares, in `build, sync, verify` order
    /// — the matrix may have stripped some.
    pub operations: Vec<&'static str>,
    /// Capability refusals the scaffold resolved by stripping an operation,
    /// rendered for the caller's output. Never a failure: a scaffold that
    /// declares less than asked says so rather than refusing.
    pub warnings: Vec<String>,
}

/// Scaffold the default binding record for one source — the single
/// definition of "a fresh binding", shared by every front door that creates
/// one (`memstead projection init`, the guided `memstead quickstart` path,
/// any embedder).
///
/// The record: one inline [`Source`] scoped `**/*` (a scoped default — an
/// unscoped source refuses at run time), the enumerable-medium deny defaults
/// materialised into the record (see [`DEFAULT_SCAFFOLD_DENY_PATHS`] for why
/// they are recorded rather than injected), unstated `coverage_semantics`
/// (the scaffold asserts nothing), and `build` + `sync` + `verify` filtered
/// through the capability matrix — a `web` source loses sync/verify and the
/// deferral rides `warnings`. Prune is scaffolded wherever sync survived.
pub fn scaffold_binding(params: ScaffoldParams<'_>) -> ScaffoldedBinding {
    let ScaffoldParams {
        destination_mem,
        source_name,
        pointer,
        medium_type,
        intent,
        additional_deny_paths,
    } = params;

    let source = Source {
        name: source_name.to_string(),
        medium_type,
        pointer: pointer.to_string(),
        change_detection: None,
        // Scope is medium-shaped. A path glob over a graph source is not a
        // narrower scope — it is an uninterpreted string: nothing anywhere
        // matches globs against entity ids, so `**/*` scaffolded a facet that
        // looked scoped and selected nothing. The graph namespace gets its own
        // whole-mem selector; every path medium keeps `**/*` byte-for-byte.
        //
        // `web` gets no scope rule at all. Its namespace is `url`, nothing
        // enumerates it, and no selector vocabulary exists for it — so any
        // pattern scaffolded here would be decorative in exactly the way the
        // graph glob was, and the brief would print it at an agent as
        // selection. An absent scope is the honest scaffold: it renders as
        // unmonitored rather than as a scope that reaches nothing.
        scope: match medium_type {
            MediumType::Graph => vec![PatternEntry {
                path: "*".to_string(),
                mode: crate::pipeline::PatternMode::Allow,
            }],
            MediumType::Web => Vec::new(),
            _ => vec![PatternEntry {
                path: "**/*".to_string(),
                mode: crate::pipeline::PatternMode::Allow,
            }],
        },
        engagement: None,
        preparation: None,
    };

    let mut deny_paths: Vec<String> =
        if matches!(medium_type, MediumType::Codebase | MediumType::Filesystem) {
            DEFAULT_SCAFFOLD_DENY_PATHS
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            Vec::new()
        };
    for extra in additional_deny_paths {
        if !deny_paths.contains(&extra) {
            deny_paths.push(extra);
        }
    }

    let mut binding = Binding {
        version: BINDING_VERSION,
        intent,
        sources: vec![source],
        reference_mems: Vec::new(),
        destination_mem: destination_mem.to_string(),
        deny_paths,
        coverage_semantics: None,
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
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 20,
                adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
            }),
        },
    };

    let mut warnings: Vec<String> = Vec::new();
    if let Err(refusals) = validate_binding(&binding) {
        for r in &refusals {
            if let CapabilityError::OperationOutOfScope { operation, .. } = r {
                match *operation {
                    "sync" => binding.operations.sync = None,
                    "verify" => binding.operations.verify = None,
                    _ => {}
                }
            }
            warnings.push(r.to_string());
        }
    }

    if binding.operations.sync.is_some() {
        binding.prune = Some(PruneConfig::default());
    }

    let mut operations: Vec<&'static str> = vec!["build"];
    if binding.operations.sync.is_some() {
        operations.push("sync");
    }
    if binding.operations.verify.is_some() {
        operations.push("verify");
    }

    ScaffoldedBinding {
        binding,
        operations,
        warnings,
    }
}

#[cfg(test)]
mod scaffold_tests;
#[cfg(test)]
mod tests;
