//! Mem configuration loading, validation, and top-level CRUD.
//!
//! Handles `.memstead/config.json` parsing, cross-field validation, and the
//! `update_config_field` write helper. Projections/mediums, their
//! validators, and the pre-rework migration have been dropped by the
//! workspace rewrite — `projections` / `mediums`
//! survive as unknown keys captured into `MemConfig.extra` so legacy
//! configs still round-trip, but the engine does not interpret them.
//!
//! Port of @memstead/config (config-contract.js, index.js) and
//! @agent-adapters/config-mcp (workspace.js).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The per-mem engine-internal directory under a folder mem's
/// root — `<mem_root>/.memstead/` holds `config.json` and
/// `changes.jsonl`. Defined here (rather than in `memstead-base`)
/// because mem-config loading lives in this crate and `memstead-base`
/// depends on it; `memstead-base` re-exports the constant for
/// downstream consumers. Distinct from the workspace store directory
/// (`memstead_base::WORKSPACE_STORE_DIR`) and from the in-zip member
/// paths inside sealed archives ([`ARCHIVE_META_DIR`]), which are a
/// separate on-disk format and never use this constant.
pub const MEM_META_DIR: &str = ".memstead";

// ---------------------------------------------------------------------------
// Sealed-archive surface constants
// ---------------------------------------------------------------------------
//
// A sealed archive is a zip whose engine-internal members live under one
// meta directory: `.memstead/config.json` plus the embedded schema tree
// `.memstead/schema/…` — the sole member layout. The file extension is
// `.mem` — the sole spelling, read and written. Defined here because
// this is the lowest crate every archive reader/writer (memstead-base,
// memstead-git-branch, memstead-registry, memstead-wasm, the CLIs)
// already depends on.

/// In-zip meta directory of a sealed archive — the only spelling.
pub const ARCHIVE_META_DIR: &str = ".memstead";
/// Member path of the published config inside a sealed archive.
pub const ARCHIVE_CONFIG_PATH: &str = ".memstead/config.json";
/// Member-path prefix of the embedded schema tree (manifest at
/// `<prefix>schema.yaml`, type files under `<prefix>types/`).
pub const ARCHIVE_SCHEMA_PREFIX: &str = ".memstead/schema/";
/// Member path of the optional authoring-provenance payload inside a
/// sealed archive (see [`crate::archive_provenance`]). Additive: archives
/// predating provenance omit it, and an engine that does not recognise it
/// tolerates it as an unknown meta member.
pub const ARCHIVE_PROVENANCE_PATH: &str = ".memstead/provenance.json";
/// File extension (without dot) of a sealed archive — the sole spelling.
/// The one deliberately-distinct token in a project that is otherwise
/// "memstead" everywhere — short, and derived from the project name.
pub const ARCHIVE_EXTENSION: &str = "mem";

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    NotFound(String),
    #[error("invalid JSON in config file: {0}")]
    InvalidJson(String),
    #[error("config validation failed:\n{}", .0.iter().map(|e| format!("  - {e}")).collect::<Vec<_>>().join("\n"))]
    ValidationFailed(Vec<String>),
    #[error("{0}")]
    Other(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Config check result
// ---------------------------------------------------------------------------

/// Result of config validation — errors are fatal, warnings are informational.
#[derive(Debug, Clone)]
pub struct ConfigCheckResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Stable `UPPER_SNAKE_CASE` envelope code when the validator
    /// detects a categorical failure that callers should branch on.
    /// Currently set to `"LEGACY_FIELD_PRESENT"` when any entry in
    /// `LEGACY_TOMBSTONE_KEYS` is present.
    pub error_code: Option<String>,
}

// ---------------------------------------------------------------------------
// Mem config types (deserialized from .memstead/config.json)
// ---------------------------------------------------------------------------

/// Role-based publish config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleConfig {
    pub include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

/// Publish config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishConfig {
    pub roles: HashMap<String, RoleConfig>,
}

/// Community detection override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
}

/// One entry in `MemConfig.read_mems` — a read-only sealed mem
/// archive attached to the primary mem as reference material.
///
/// The engine resolves each entry to a cache file: when `cache_key` is
/// present, `<mem_cache_dir>/<name>-<cache_key>.mem` (content-addressed
/// — see [`ReadMemSpec::cache_key`]); otherwise the legacy
/// `<mem_cache_dir>/<name>.mem`.
///
/// Kept as a struct (rather than collapsing to a bare `ReadMemSource`)
/// so forward-compatible fields can be added without another schema break.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadMemSpec {
    pub source: ReadMemSource,
    /// Content-address of the installed archive — a short hex digest of
    /// the validator's canonical bytes. The install path writes the cache
    /// file at `<cache>/<name>-<cache_key>.mem`, so two distinct archives
    /// sharing an internal mem name land in distinct files (no collision)
    /// and re-installing identical bytes resolves to the same file (dedup).
    /// `None` for legacy registrations written before content-addressing;
    /// the loader then falls back to the bare `<name>.mem` path.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "cacheKey")]
    pub cache_key: Option<String>,
}

/// How the app reconstitutes a read mem's cache file when missing.
///
/// The engine itself never fetches; `source` is metadata consumed by the
/// app's installer. A `Registry` variant with scope/name identifiers
/// will be added once the memstead.io registry ships.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ReadMemSource {
    /// User dropped an archive file onto the app. App cannot auto-reinstall —
    /// it prompts the user to drop the original file again.
    Local,
    /// Fetched from an HTTPS URL (GitHub Releases, shared drive, any static
    /// host). Engine-side no-op; the app's installer re-fetches on attach.
    Url { url: String },
    // `Registry` variant reserved for when the memstead.io registry ships.
    // The exact shape (fields, id format like `@scope/name`) is designed
    // then — declaring it up front without semantics would be
    // speculative, and pre-1.0 adding a variant later is not a breaking
    // change for anyone.
}

/// Reference to a schema by exact name and version — `name@x.y.z`.
///
/// Serializes/deserializes as a single string so mem configs read
/// `{ "schema": "default@1.0.0" }` on disk. Range syntax (`^`, `~`,
/// `latest`) is rejected — schema pinning is strict and explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaRef {
    pub name: String,
    pub version: semver::Version,
}

impl SchemaRef {
    pub fn new(name: impl Into<String>, version: semver::Version) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }

    pub fn as_display(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

impl std::fmt::Display for SchemaRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

impl std::str::FromStr for SchemaRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("schema reference must not be empty (expected \"name@x.y.z\")".into());
        }
        let (name, version_str) = trimmed.split_once('@').ok_or_else(|| {
            format!(
                "schema reference '{trimmed}' must include an exact version — expected \"name@x.y.z\""
            )
        })?;
        if name.is_empty() {
            return Err("schema reference name must not be empty".into());
        }
        if version_str == "latest" {
            return Err(format!(
                "schema reference '{trimmed}' uses 'latest' — exact semver versions only"
            ));
        }
        if version_str.starts_with(['^', '~', '>', '<', '=', '*']) {
            return Err(format!(
                "schema reference '{trimmed}' uses range syntax — exact semver only (e.g. 'default@1.0.0')"
            ));
        }
        let version = semver::Version::parse(version_str).map_err(|e| {
            format!("schema reference '{trimmed}' has invalid semver version '{version_str}': {e}")
        })?;
        Ok(Self {
            name: name.to_string(),
            version,
        })
    }
}

impl Serialize for SchemaRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_display())
    }
}

impl<'de> Deserialize<'de> for SchemaRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse::<SchemaRef>().map_err(serde::de::Error::custom)
    }
}

// `MemSchemaPin` (the two-variant pin with a name-only fallback) was
// retired here. Mem configs now declare a strict `<name>@<version>`
// pin parsed directly through [`SchemaRef`]; bare-name pins are
// rejected at config load.

/// VCS layout for a writable mem — optional `{ gitdir, worktree }` pair
/// in `.memstead/config.json`. When absent, the engine resolves the default:
/// `.git/` at mem root with `.` as worktree.
///
/// Paths are relative to mem root and interpreted by `memstead-git-branch` —
/// this crate just carries them through serde. Masterplan §3.4 is
/// explicit that the primitive is a pair of paths; the two canonical
/// idioms (isolated `{ ".git", "." }` and shared `{ "../.git", ".." }`)
/// are idioms, not enum variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VcsConfig {
    /// Path to the gitdir relative to mem root. Required when the
    /// `vcs` block is present.
    pub gitdir: String,
    /// Path to the worktree relative to mem root. Optional within the
    /// `vcs` block — defaults to `"."` (mem root) when omitted.
    #[serde(default = "vcs_worktree_default")]
    pub worktree: String,
}

fn vcs_worktree_default() -> String {
    ".".to_string()
}

/// Tolerant deserializer for the `vcs` field: accepts the object form
/// (`{ gitdir, worktree? }`) and returns `None` for any non-object value
/// (string, number, boolean, null). A missing field is also `None`.
///
/// Motivation: an older macOS Mem-mode UI wrote `"vcs": "system"`
/// (and similar sentinel strings) into `.memstead/config.json` files that
/// now must continue to load without editing those files by hand.
/// Strict validation of the object form — unknown keys, missing
/// `gitdir`, etc. — still surfaces as a hard serde error.
fn deserialize_vcs_tolerant<'de, D>(deserializer: D) -> Result<Option<VcsConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(_)) => {
            let v = value.unwrap();
            Ok(Some(
                serde_json::from_value(v).map_err(serde::de::Error::custom)?,
            ))
        }
        Some(_) => Ok(None),
    }
}

/// Full mem configuration loaded from .memstead/config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemConfig {
    /// Optional mem name. The leaf folder name under
    /// `__MEMSTEAD:mems/` (and the disk basename on the legacy disk
    /// path) is authoritative; engine-written configs omit this
    /// field. Tolerated on read for pre-cutover configs and for the
    /// [`PublishedMemConfig`] conversion path that still requires
    /// an explicit identity (the caller passes the name in when
    /// projecting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Semver version of the mem content. Read at mem-archive export
    /// time so the engine always knows the current version without manual
    /// tracking. Parsed at config load — invalid version strings fail fast
    /// with a source-attributed serde error rather than slipping through to
    /// export (where the issue only surfaces when a downstream loader tries
    /// to resolve a `semver::VersionReq` against the mem).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<semver::Version>,

    /// One-line description of the mem, surfaced in mem-archive metadata
    /// and UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional author attribution, surfaced in mem-archive metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,

    /// Schema this mem is pinned to. Exact `<name>@<version>` pin
    /// only — bare-name forms are rejected at config load. Exactly one
    /// schema per mem. The `Option` keeps serde tolerant so a missing
    /// key surfaces as a structured error from `check_config` rather
    /// than a deserialize panic; a `None` value is a validation error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaRef>,

    /// Opaque string-map passed through by the engine. Agents and
    /// plugin prompt renderers are free to invent their own keys; the
    /// engine does not parse, validate, or interpret any value inside.
    /// Stripped from `PublishedMemConfig` — guidance is workspace-
    /// local authorship metadata, not part of the published identity.
    ///
    /// Pre-2026-04-24 this field was `Option<Value>`; the workspace
    /// rewrite normalised it to a map so the shape on
    /// the wire is stable and the engine's pass-through guarantee is
    /// type-checked.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub write_guidance: HashMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Read-only sealed-archive mems attached to this mem as reference
    /// material. Key is the mem name (matches the archive's
    /// config name). Engine resolves each entry to
    /// `<mem_cache_dir>/<name>.mem` at init time. An empty or omitted
    /// map means no attached mems — a graph with no reference material.
    ///
    /// `BTreeMap` (not `HashMap`) so iteration and serialization order
    /// are stable — reproducible log output and diff-friendly config on
    /// disk. Explicit `rename = "readMems"` documents the on-disk name
    /// at the field (the struct-level `rename_all = "camelCase"` already
    /// handles it, but explicit rename is greppable from either side).
    #[serde(
        rename = "readMems",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub read_mems: BTreeMap<String, ReadMemSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community: Option<CommunityOverride>,

    /// Optional VCS layout override. When absent, `memstead-git-branch` resolves
    /// the default at init time: `.git/` at mem root with `.` as
    /// worktree. When present, `gitdir` and `worktree` are paths
    /// relative to the mem root. Stripped from `PublishedMemConfig`
    /// — VCS layout is workspace-local mechanics, not part of the
    /// published mem's identity.
    ///
    /// Deserialization is tolerant of legacy non-object values (e.g.
    /// `"vcs": "system"` — the sentinel an older macOS Mem-mode
    /// UI wrote): any non-object form deserializes to `None` and falls
    /// back to the default-resolution path. The object form is validated
    /// strictly.
    #[serde(
        default,
        deserialize_with = "deserialize_vcs_tolerant",
        skip_serializing_if = "Option::is_none"
    )]
    pub vcs: Option<VcsConfig>,

    /// Tombstone marker written by `memstead mem unregister`. ISO-8601
    /// UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) recorded at the moment
    /// the mem was unregistered while its storage was preserved.
    /// When `memstead mem init <same-name>`
    /// probes the storage and finds an `unregistered_at` value, it
    /// treats the residue as deliberate operator state and defaults
    /// to the `Reattach` recovery action (adopting the preserved
    /// entities and clearing the tombstone). Absence (`None`) on
    /// otherwise-present residue triggers `MEM_STORAGE_RESIDUE_DETECTED`
    /// unless the caller passes an explicit `recovery` flag. Stripped
    /// from `PublishedMemConfig` — tombstones are workspace-local
    /// lifecycle state, not part of the published mem's identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unregistered_at: Option<String>,

    /// Per-source "last successfully synced source state", written by
    /// the ingest layer and surfaced verbatim on the workspace dump.
    /// The engine never parses, validates, or interprets a value:
    /// each token is opaque, its meaning owned by the medium-type
    /// layer that produced it (git → commit id, graph → snapshot
    /// token, filesystem → a small stat digest the plugin
    /// JSON-stringifies). The key is likewise opaque — the ingest
    /// layer keys per `(ingest, facet)` (conventionally
    /// `"<ingest>/<facet>"`), but the engine treats it as an arbitrary
    /// string. This is the durable, shared baseline against which a
    /// fresh ingest iteration diffs "what changed since last time";
    /// it survives a skill-cache wipe and a machine change because it
    /// lives in engine-held mem config, not ephemeral plugin cache.
    ///
    /// Stripped from `PublishedMemConfig` — sync state is
    /// workspace-local ingest bookkeeping, not part of a published
    /// mem's identity. `BTreeMap` (not `HashMap`) for stable
    /// serialization order: diff-friendly config on disk and
    /// reproducible dump output.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sync_state: BTreeMap<String, String>,

    /// Extra fields not in the known set (captured for round-tripping).
    ///
    /// Historical tombstones:
    /// - `defaultSchema` (pre-2026-04): legacy per-mem default type.
    ///   Per-entity `type:` frontmatter is authoritative now.
    /// - `types: [...]` (pre-schema-artifact, 2026-04): replaced by
    ///   `schema: "<name>@<version>"`. Legacy entries are hard-rejected
    ///   by `check_config`.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Published (archive) mem config
// ---------------------------------------------------------------------------

/// Strict-ingress shape of a mem config. This is the **only** metadata
/// form that enters a `.mem` archive. `MemConfig` carries author-only
/// fields (writeGuidance, rules, publish, readMems, language,
/// community, defaultSchema, vcs, plus any key captured in
/// `extra`) that never belong in a published archive;
/// `published_config_from` projects `MemConfig` →
/// `PublishedMemConfig`, dropping everything outside the whitelist.
///
/// `deny_unknown_fields` + no `serde(flatten)` on purpose: the validator
/// re-parses this shape with the same struct as defense-in-depth, so any
/// legacy author key smuggled into an archive surfaces as a rejection
/// instead of a silently-tolerated payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublishedMemConfig {
    pub format: u32,
    pub name: String,
    pub version: semver::Version,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    pub schema: SchemaRef,
}

/// Archive format integer written to the archive config's `format`
/// field. Bumped to `3` for the schema-path relocation (embedded schema
/// moved from top-level `schema/` to the meta-dir schema tree). `format: 1` (V1)
/// and `format: 2` (V2, top-level `schema/` tree) archives are rejected
/// cleanly (pre-release, no external users to migrate).
pub const PUBLISHED_MEM_FORMAT: u32 = 3;

/// Errors returned by `published_config_from`. Actionable messages —
/// the caller (export pipeline, publish pipeline) surfaces these
/// directly to the user without wrapping a raw serde error.
#[derive(Debug, thiserror::Error)]
pub enum PublishConversionError {
    #[error("config.version is required for mem publish — set it in .memstead/config.json")]
    MissingVersion,
    #[error(
        "config must declare `schema` (e.g. \"default@1.0.0\") — set it in .memstead/config.json"
    )]
    MissingSchema,
    #[error(
        "publish requires an explicit mem name — caller must pass the leaf folder name (Goal 3 of mem-repo-restructure dropped the in-config `name` requirement)"
    )]
    MissingName,
}

/// The whitelist projection. Everything author-only is discarded; only
/// the fields that make sense outside the author's working directory
/// ride into the archive. `format` is pinned at `PUBLISHED_MEM_FORMAT`.
///
/// `name` is supplied explicitly by the caller — the on-disk `name`
/// field is optional and the engine no longer treats it as the
/// mem-identity source. The published archive still needs an
/// identity, so the publishing path passes the leaf folder name
/// (`__MEMSTEAD:mems/<path>/<leaf>/config.json`'s `<leaf>`, or the
/// disk basename on the legacy disk path) here. Falls back to the
/// in-config `name` field when the caller passes an empty string and
/// the config still carries a legacy `name` value (so pre-cutover
/// archives published before the migration land cleanly).
pub fn published_config_from(
    config: &MemConfig,
    name: &str,
) -> Result<PublishedMemConfig, PublishConversionError> {
    let version = config
        .version
        .clone()
        .ok_or(PublishConversionError::MissingVersion)?;
    let schema = config
        .schema
        .clone()
        .ok_or(PublishConversionError::MissingSchema)?;
    let resolved_name = if name.is_empty() {
        config
            .name
            .clone()
            .ok_or(PublishConversionError::MissingName)?
    } else {
        name.to_string()
    };
    Ok(PublishedMemConfig {
        format: PUBLISHED_MEM_FORMAT,
        name: resolved_name,
        version,
        description: config.description.clone(),
        authors: config.authors.clone(),
        schema,
    })
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const KNOWN_TOP_LEVEL_KEYS: &[&str] = &[
    "version",
    "description",
    "authors",
    "schema",
    "writeGuidance",
    "rules",
    "publish",
    "language",
    "readMems",
    "community",
    "vcs",
    "syncState",
];

/// Keys that are explicitly rejected with a `LEGACY_FIELD_PRESENT`
/// envelope when present in a config. The validator surfaces a hard
/// error (not a soft "unknown key" warning) so agents that recreate
/// the legacy shape from training-set examples see a structured
/// rejection instead of silent acceptance with drift.
///
/// Each entry pairs the rejected key with an actionable error message.
/// New tombstones land here when a top-level key migrates from
/// "deprecated but tolerated" to "must not be re-authored". The table
/// holds entries for `name` (the field is now path-derived) and
/// `types: [...]` (the pre-existing tombstone, preserved verbatim).
const LEGACY_TOMBSTONE_KEYS: &[(&str, &str)] = &[
    (
        "types",
        "Legacy `types: [...]` field detected — replace with `schema: \"<name>@<version>\"` \
         (e.g. `\"schema\": \"default@1.0.0\"`).",
    ),
    (
        "name",
        "Legacy `name` field detected — the mem leaf folder under `__MEMSTEAD:mems/` (or the \
         disk basename on the legacy disk path) is path-derived under the unified layout; \
         remove the field from `.memstead/config.json`.",
    ),
    (
        "belongsTo",
        "Legacy `belongsTo` field detected — cross-mem authorization moved to the \
         workspace-level `[cross_mem_links]` section in `.memstead/workspace.toml`. Remove the \
         field from `.memstead/config.json` and add an entry under `[cross_mem_links]` \
         instead.",
    ),
];

// ---------------------------------------------------------------------------
// checkConfig — the main validator
// ---------------------------------------------------------------------------

/// Validate a raw config JSON value. Returns structured errors and warnings.
pub fn check_config(config: &Value) -> ConfigCheckResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let obj = match config.as_object() {
        Some(o) => o,
        None => {
            errors.push("(root): config must be an object".to_string());
            return ConfigCheckResult {
                valid: false,
                errors,
                warnings,
                error_code: None,
            };
        }
    };

    // 2. Legacy tombstones — keys that must not be re-authored. Each
    //    hit produces a hard error and pins the `LEGACY_FIELD_PRESENT`
    //    envelope code so callers branch on a stable identifier rather
    //    than the human-readable error message. See
    //    `LEGACY_TOMBSTONE_KEYS` for the reject list.
    let mut legacy_field_hit = false;
    for (key, message) in LEGACY_TOMBSTONE_KEYS {
        if obj.contains_key(*key) {
            errors.push((*message).to_string());
            legacy_field_hit = true;
        }
    }

    // 3. Schema field: exact `name@x.y.z` reference required. Bare-name
    //    pins are rejected at parse time via SchemaRef::from_str.
    match obj.get("schema") {
        Some(Value::String(s)) => {
            if let Err(e) = s.parse::<SchemaRef>() {
                errors.push(format!("schema: {e}"));
            }
        }
        Some(_) => errors.push(
            "schema: must be a string of the form \"<name>@<x.y.z>\" \
             (exact version pin, e.g. \"default@1.0.0\")"
                .to_string(),
        ),
        None => errors.push(
            "Config must declare `schema` — exact pin of the form \
             \"<name>@<x.y.z>\" (e.g. \"default@1.0.0\")"
                .to_string(),
        ),
    }

    // 4. Read-mems map — source presence and shape.
    //    Cache-file existence is checked at engine init (`Engine::init`
    //    via `mem_cache`), not here, so isolated schema tests don't
    //    need real archive fixture files. The cached archive's config is
    //    authoritative for the version; no `version` or `path` is
    //    recorded in the config entry.
    if let Some(Value::Object(mems)) = obj.get("readMems") {
        for (name, spec) in mems {
            let entry_path = format!("readMems.{name}");

            let spec_obj = match spec.as_object() {
                Some(o) => o,
                None => {
                    errors.push(format!("{entry_path}: read-mem entry must be an object"));
                    continue;
                }
            };

            let source = match spec_obj.get("source").and_then(|v| v.as_object()) {
                Some(s) => s,
                None => {
                    errors.push(format!(
                        "{entry_path}.source: read-mem entry must declare a source \
                         (e.g. {{\"type\": \"local\"}} or {{\"type\": \"url\", \"url\": \"…\"}})"
                    ));
                    continue;
                }
            };

            match source.get("type").and_then(|v| v.as_str()) {
                Some("local") => {}
                Some("url") => match source.get("url").and_then(|v| v.as_str()) {
                    Some(u) if !u.is_empty() => {}
                    _ => errors.push(format!(
                        "{entry_path}.source.url: url source must declare a non-empty 'url' string"
                    )),
                },
                // `registry` type is reserved for future use but not
                // accepted yet — fails here alongside any other unknown.
                Some(other) => errors.push(format!(
                    "{entry_path}.source.type: unknown source type '{other}' \
                     (expected 'local' or 'url')"
                )),
                None => errors.push(format!(
                    "{entry_path}.source.type: source must declare a 'type' \
                     ('local' or 'url')"
                )),
            }
        }
    }

    // 5. `belongsTo` is now a tombstone (see `LEGACY_TOMBSTONE_KEYS`).
    //    Cross-mem authorization moved to the workspace-level
    //    `[cross_mem_links]` section in `.memstead/workspace.toml`. Per-mem config
    //    blobs that still carry the field are rejected with the
    //    tombstone error above; no shape validation runs here.

    // 7. Unknown key warnings. Tombstone keys are rejected above and
    //    skipped here so callers don't see a redundant warning alongside
    //    the hard error.
    for key in obj.keys() {
        if KNOWN_TOP_LEVEL_KEYS.contains(&key.as_str())
            || LEGACY_TOMBSTONE_KEYS
                .iter()
                .any(|(k, _)| *k == key.as_str())
        {
            continue;
        }
        warnings.push(format!(
            "Unknown config key '{key}' \u{2014} will be ignored"
        ));
    }

    let error_code = if legacy_field_hit {
        Some("LEGACY_FIELD_PRESENT".to_string())
    } else {
        None
    };

    ConfigCheckResult {
        valid: errors.is_empty(),
        errors,
        warnings,
        error_code,
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Load and parse a config from a mem directory.
/// Reads `<mem_dir>/.memstead/config.json`.
pub fn load_config(mem_dir: &Path) -> Result<(Value, PathBuf), ConfigError> {
    let config_path = mem_dir.join(MEM_META_DIR).join("config.json");
    let raw = std::fs::read_to_string(&config_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfigError::NotFound(config_path.display().to_string())
        } else {
            ConfigError::Io(e)
        }
    })?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|_| ConfigError::InvalidJson(config_path.display().to_string()))?;
    Ok((parsed, config_path))
}

/// Parse a raw JSON value into a MemConfig.
pub fn parse_mem_config(value: &Value) -> Result<MemConfig, ConfigError> {
    serde_json::from_value(value.clone()).map_err(|e| ConfigError::Other(e.to_string()))
}

/// Load, validate, and parse a mem config from disk.
pub fn load_and_validate(mem_dir: &Path) -> Result<MemConfig, ConfigError> {
    let (raw, _path) = load_config(mem_dir)?;

    let result = check_config(&raw);
    if !result.valid {
        return Err(ConfigError::ValidationFailed(result.errors));
    }

    parse_mem_config(&raw)
}

// ---------------------------------------------------------------------------
// Config writing
// ---------------------------------------------------------------------------

/// Write a config JSON value to disk (pretty-printed with trailing newline).
fn write_config(config_path: &Path, config: &Value) -> Result<(), ConfigError> {
    let json = serde_json::to_string_pretty(config)? + "\n";
    std::fs::write(config_path, json)?;
    Ok(())
}

/// Validate and write a config to disk. Returns check result.
fn commit_config(
    config_path: &Path,
    config: &Value,
    dry_run: bool,
) -> Result<ConfigCheckResult, ConfigError> {
    let check = check_config(config);
    if !check.valid {
        return Ok(check);
    }
    if !dry_run {
        write_config(config_path, config)?;
    }
    Ok(check)
}

// ---------------------------------------------------------------------------
// Config CRUD operations
// ---------------------------------------------------------------------------

/// Allowed top-level fields for `update_config_field`. Mirrored by the
/// macOS app's `WorkspaceService.allowedUpdateFields`. The
/// workspace rewrite dropped `mediums` and
/// `projections` here: the engine no longer recognises those blocks so
/// they are not writable through the update surface either.
const ALLOWED_UPDATE_FIELDS: &[&str] = &[
    "version",
    "description",
    "authors",
    "writeGuidance",
    "rules",
    "readMems",
    "schema",
    "language",
    "publish",
];

const PROTECTED_FIELDS: &[&str] = &["name"];

/// Update a top-level config field.
pub fn update_config_field(
    config_path: &Path,
    config: &mut Value,
    field: &str,
    value: Value,
    dry_run: bool,
) -> Result<ConfigCheckResult, ConfigError> {
    if PROTECTED_FIELDS.contains(&field) {
        return Err(ConfigError::Other(format!("Field '{field}' is protected")));
    }

    let obj = config
        .as_object_mut()
        .ok_or_else(|| ConfigError::Other("config must be an object".into()))?;

    if !ALLOWED_UPDATE_FIELDS.contains(&field) {
        return Err(ConfigError::Other(format!(
            "Field '{field}' is not a recognized config field. Allowed: {}",
            ALLOWED_UPDATE_FIELDS.join(", ")
        )));
    }

    obj.insert(field.to_string(), value);
    commit_config(config_path, config, dry_run)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- check_config tests ---

    fn minimal_valid_config() -> Value {
        json!({
            "schema": "default@1.0.0"
        })
    }

    #[test]
    fn check_valid_minimal_config() {
        let result = check_config(&minimal_valid_config());
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    /// The in-config `name` field is optional — configs without a
    /// `name` key are valid; the leaf folder name under
    /// `__MEMSTEAD:mems/` (or the disk basename on the legacy disk
    /// path) is the authoritative identifier instead.
    #[test]
    fn check_missing_name_now_valid() {
        let config = json!({"schema": "default@1.0.0"});
        let result = check_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    /// `parse_mem_config` produces a `MemConfig` whose `name` is
    /// `None` when the on-disk config omits the field. Pins the
    /// Goal 3 wire-shape contract.
    #[test]
    fn parse_mem_config_name_none_when_field_absent() {
        let config = json!({"schema": "default@1.0.0"});
        let parsed = parse_mem_config(&config).expect("name-less config parses");
        assert!(parsed.name.is_none());
    }

    /// Round-trip: a `MemConfig` whose `name` is `None` serialises
    /// without the `name` key (skip-if-none on the serde attribute).
    /// Pins the on-disk minimisation contract.
    #[test]
    fn mem_config_omits_name_when_none_on_serialize() {
        let cfg = MemConfig {
            name: None,
            version: None,
            description: None,
            authors: None,
            schema: Some(SchemaRef::new("default", semver::Version::new(1, 0, 0))),
            write_guidance: Default::default(),
            rules: None,
            publish: None,
            language: None,
            read_mems: Default::default(),
            community: None,
            vcs: None,
            unregistered_at: None,
            sync_state: Default::default(),
            extra: Default::default(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            !json.contains("\"name\""),
            "serialized config must omit `name` when None, got: {json}"
        );
    }

    /// A stray `name` field is rejected with `LEGACY_FIELD_PRESENT`
    /// regardless of its value (empty or non-empty). Both empty and
    /// non-empty shapes collapse onto the legacy tombstone reject.
    #[test]
    fn check_legacy_name_field_rejected() {
        for value in [json!(""), json!("@test/mem")] {
            let config = json!({"name": value, "schema": "default@1.0.0"});
            let result = check_config(&config);
            assert!(!result.valid, "name={value}: expected reject");
            assert_eq!(
                result.error_code.as_deref(),
                Some("LEGACY_FIELD_PRESENT"),
                "name={value}: expected LEGACY_FIELD_PRESENT envelope"
            );
            assert!(
                result.errors.iter().any(|e| e.contains("Legacy `name`")),
                "name={value}: errors {:?}",
                result.errors
            );
        }
    }

    #[test]
    fn check_missing_schema() {
        let config = json!({});
        let result = check_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("`schema`")));
    }

    #[test]
    fn check_legacy_types_array_rejected() {
        let config = json!({"types": ["spec"], "schema": "default@1.0.0"});
        let result = check_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("Legacy `types:")));
        assert_eq!(
            result.error_code.as_deref(),
            Some("LEGACY_FIELD_PRESENT"),
            "expected LEGACY_FIELD_PRESENT envelope for legacy `types`"
        );
    }

    #[test]
    fn check_schema_wrong_shape() {
        let config = json!({"schema": ["default@1.0.0"]});
        let result = check_config(&config);
        assert!(!result.valid);
    }

    #[test]
    fn check_schema_bare_name_rejected() {
        // Bare-name pins are rejected at load — every mem config must
        // declare an exact `<name>@<version>` pin so cross-mem link
        // matching and archive identity are unambiguous.
        let config = json!({"schema": "default"});
        let result = check_config(&config);
        assert!(!result.valid, "expected bare-name pin to be rejected");
        assert!(
            result.errors.iter().any(|e| e.contains("schema")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn check_schema_range_syntax_rejected() {
        for s in [
            "default@^1.0.0",
            "default@~1.0.0",
            "default@latest",
            "default@>=1.0.0",
        ] {
            let config = json!({"schema": s});
            let result = check_config(&config);
            assert!(!result.valid, "expected '{s}' to be rejected");
        }
    }

    #[test]
    fn check_schema_valid_exact_pin() {
        let config = json!({"schema": "default@1.0.0"});
        let result = check_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn schema_pin_versioned_parses() {
        let pin: SchemaRef = "software@1.2.3".parse().unwrap();
        assert_eq!(pin.name, "software");
        assert_eq!(pin.version, semver::Version::new(1, 2, 3));
        assert_eq!(pin.as_display(), "software@1.2.3");
    }

    #[test]
    fn schema_pin_bare_name_rejected() {
        // Bare-name pins are rejected at parse — agents must declare the
        // exact version. Bogus name shapes (uppercase, slash, empty) fall
        // through the same gate.
        for bad in ["software", "Default", "foo/bar", "", "  "] {
            assert!(
                bad.parse::<SchemaRef>().is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn schema_pin_serde_round_trip() {
        let versioned: SchemaRef = serde_json::from_str(r#""software@1.0.0""#).unwrap();
        assert_eq!(versioned.as_display(), "software@1.0.0");
        let as_json = serde_json::to_string(&versioned).unwrap();
        assert_eq!(as_json, r#""software@1.0.0""#);
    }

    #[test]
    fn publish_rejects_missing_schema() {
        // Archives record a concrete schema version; a config without a
        // `schema` field cannot be published.
        let json = json!({ "version": "1.0.0" });
        let config = parse_mem_config(&json).unwrap();
        let err = published_config_from(&config, "demo").unwrap_err();
        assert!(matches!(err, PublishConversionError::MissingSchema));
    }

    #[test]
    fn publish_accepts_versioned_pin() {
        let json = json!({
            "version": "1.0.0",
            "schema": "software@2.3.4"
        });
        let config = parse_mem_config(&json).unwrap();
        let published = published_config_from(&config, "demo").expect("versioned pin publishes");
        assert_eq!(published.name, "demo");
        assert_eq!(published.schema.name, "software");
        assert_eq!(published.schema.version, semver::Version::new(2, 3, 4));
    }

    #[test]
    fn check_legacy_default_schema_field_is_ignored() {
        // `defaultSchema` was an author-only tombstone field pre-2026-04.
        // It's captured into `extra` and surfaces as an unknown-field
        // warning without invalidating an otherwise well-formed config.
        let config = json!({
            "schema": "default@1.0.0",
            "defaultSchema": "spec"
        });
        let result = check_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn config_preserves_unknown_fields_on_roundtrip() {
        // Any unknown top-level field (legacy `defaultSchema`, future
        // fields, typos) is captured into `extra` and re-emitted
        // unchanged — guarantees no silent data loss on read-modify-write.
        let raw = json!({
            "schema": "default@1.0.0",
            "defaultSchema": "concept"
        });
        let cfg: MemConfig = serde_json::from_value(raw).expect("config deserialized");
        assert!(
            cfg.extra.contains_key("defaultSchema"),
            "legacy field should be preserved in extra: {:?}",
            cfg.extra
        );

        let reserialized = serde_json::to_value(&cfg).expect("config reserialized");
        assert_eq!(
            reserialized.get("defaultSchema").and_then(|v| v.as_str()),
            Some("concept"),
            "round-trip should preserve the legacy field"
        );
    }

    #[test]
    fn check_unknown_keys_warned() {
        let config = json!({
            "schema": "default@1.0.0",
            "unknownKey": "value"
        });
        let result = check_config(&config);
        assert!(result.valid);
        assert!(result.warnings.iter().any(|w| w.contains("unknownKey")));
    }

    /// Tombstone keys produce a hard error, not a soft "unknown key"
    /// warning. The unknown-key sweep must skip them so callers see
    /// exactly one signal per legacy key.
    #[test]
    fn legacy_tombstone_does_not_double_warn() {
        let config = json!({ "name": "x", "schema": "default@1.0.0" });
        let result = check_config(&config);
        assert!(!result.valid);
        let unknown_warning = result
            .warnings
            .iter()
            .any(|w| w.contains("Unknown config key 'name'"));
        assert!(
            !unknown_warning,
            "legacy tombstone must not also surface as unknown-key warning: {:?}",
            result.warnings
        );
    }

    // --- MemConfig slimdown ---

    #[test]
    fn slim_config_with_only_retained_core_fields_loads() {
        // The post-slimdown engine reads a minimal config carrying only
        // the fields it actually uses. `schema`, `vcs`, `writeGuidance`
        // are the intent-level retained set; serde-required collection
        // defaults fill the rest. The mem leaf identity is path-derived
        // (Goal 3 of mem-repo-restructure) so the in-config `name`
        // field is now a tombstone (Goal 10). Cross-mem authorization
        // moved to `.memstead/workspace.toml`'s `[cross_mem_links]` section.
        let raw = json!({
            "schema": "default@1.0.0",
            "writeGuidance": {
                "style": "structured",
                "audience": "agent"
            },
            "vcs": { "gitdir": ".git", "worktree": "." }
        });
        let check = check_config(&raw);
        assert!(check.valid, "errors: {:?}", check.errors);
        let parsed = parse_mem_config(&raw).expect("slim config parses");
        assert!(parsed.name.is_none());
        assert_eq!(parsed.write_guidance.len(), 2);
        assert_eq!(
            parsed.write_guidance.get("style").and_then(|v| v.as_str()),
            Some("structured")
        );
        assert!(parsed.vcs.is_some());
        assert!(parsed.extra.is_empty());
    }

    #[test]
    fn legacy_projections_block_lands_in_extra_without_error() {
        // Pre-rewrite configs carrying `projections` / `mediums` blocks
        // are no longer interpreted by the engine, but round-tripping
        // them must not fail — the blocks fall into `MemConfig.extra`
        // so read-modify-write preserves authorship. check_config emits
        // a "Unknown config key" warning per unrecognised top-level key.
        let raw = json!({
            "schema": "default@1.0.0",
            "mediums": {
                "codebase": {
                    "type": "codebase",
                    "scope": { "tree": [{ "path": "src/", "mode": "allow" }] }
                }
            },
            "projections": {
                "p1": {
                    "intent": "test",
                    "sources": [{ "medium_ref": "codebase" }],
                    "destination": { "medium_ref": "graph" }
                }
            }
        });
        let check = check_config(&raw);
        assert!(
            check.valid,
            "legacy projections/mediums must load without errors: {:?}",
            check.errors
        );
        let projection_warned = check.warnings.iter().any(|w| w.contains("projections"));
        let mediums_warned = check.warnings.iter().any(|w| w.contains("mediums"));
        assert!(
            projection_warned && mediums_warned,
            "unknown-key warnings expected for projections and mediums: {:?}",
            check.warnings
        );

        let parsed = parse_mem_config(&raw).expect("legacy config parses");
        assert!(
            parsed.extra.contains_key("projections"),
            "legacy `projections` must land in extra: {:?}",
            parsed.extra.keys().collect::<Vec<_>>()
        );
        assert!(
            parsed.extra.contains_key("mediums"),
            "legacy `mediums` must land in extra: {:?}",
            parsed.extra.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn write_guidance_round_trips_as_string_map() {
        // `writeGuidance` is an opaque `HashMap<String, Value>` now.
        // Round-trip a map with string / array / object / number values
        // to confirm every JSON shape survives verbatim — the engine
        // must not interpret or normalise its contents.
        let raw = json!({
            "schema": "default@1.0.0",
            "writeGuidance": {
                "style": "structured",
                "patterns": ["extract", "summarise"],
                "nested": { "depth": 2, "flag": true },
                "count": 42
            }
        });
        let parsed = parse_mem_config(&raw).expect("config parses");
        assert_eq!(parsed.write_guidance.len(), 4);
        assert_eq!(
            parsed.write_guidance.get("style").and_then(|v| v.as_str()),
            Some("structured")
        );
        let wire = serde_json::to_value(&parsed).expect("reserialize");
        let guidance = wire
            .get("writeGuidance")
            .and_then(|v| v.as_object())
            .expect("writeGuidance present in wire form");
        assert_eq!(guidance.len(), 4);
        assert_eq!(
            guidance.get("style").and_then(|v| v.as_str()),
            Some("structured")
        );
        assert_eq!(
            guidance
                .get("patterns")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(2)
        );
        assert_eq!(
            guidance
                .get("nested")
                .and_then(|v| v.get("depth"))
                .and_then(|v| v.as_u64()),
            Some(2)
        );
    }

    #[test]
    fn write_guidance_empty_map_omits_from_wire() {
        // `skip_serializing_if = "HashMap::is_empty"` keeps an unset
        // writeGuidance off the wire entirely so existing minimal
        // configs don't gain an empty `{}` after a round-trip.
        let parsed: MemConfig = serde_json::from_value(minimal_valid_config()).unwrap();
        assert!(parsed.write_guidance.is_empty());
        let wire = serde_json::to_value(&parsed).unwrap();
        assert!(
            wire.get("writeGuidance").is_none(),
            "empty writeGuidance must be omitted from the wire: {wire}"
        );
    }

    #[test]
    fn published_config_strips_extra_and_write_guidance() {
        // `PublishedMemConfig` uses `deny_unknown_fields` with a
        // fixed whitelist — the catchall `extra` and the pass-through
        // `writeGuidance` both fall off the projection. This guards
        // against a future reviewer adding either to the whitelist by
        // mistake.
        let mut extra = HashMap::new();
        extra.insert(
            "projections".to_string(),
            json!({ "p1": { "intent": "x" } }),
        );
        let mut guidance = HashMap::new();
        guidance.insert("style".to_string(), json!("structured"));
        let mut sync_state = BTreeMap::new();
        sync_state.insert(
            "engine-graph/source-files".to_string(),
            "deadbeef".to_string(),
        );
        let cfg = MemConfig {
            name: Some("demo".to_string()),
            version: Some(semver::Version::new(0, 1, 0)),
            description: None,
            authors: None,
            schema: Some("default@1.0.0".parse().unwrap()),
            write_guidance: guidance,
            rules: None,
            publish: None,
            language: None,
            read_mems: BTreeMap::new(),
            community: None,
            vcs: None,
            unregistered_at: None,
            sync_state,
            extra,
        };
        let published = published_config_from(&cfg, "").expect("publish projection");
        let wire = serde_json::to_value(&published).expect("serialize");
        assert!(
            wire.get("projections").is_none(),
            "extra must not leak into published wire: {wire}"
        );
        assert!(
            wire.get("writeGuidance").is_none(),
            "writeGuidance must not leak into published wire: {wire}"
        );
        assert!(
            wire.get("syncState").is_none(),
            "syncState must not leak into published wire: {wire}"
        );
    }

    // --- legacy tombstones — kept to lock behaviour after slimdown ---

    // --- migration tests ---

    // --- flatten tests ---

    // --- shadow detection tests ---

    // --- CRUD dry run tests ---

    #[test]
    fn update_config_field_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");

        let mut config = minimal_valid_config();
        let err =
            update_config_field(&config_path, &mut config, "name", json!("new"), true).unwrap_err();
        assert!(err.to_string().contains("protected"));
    }

    #[test]
    fn update_config_field_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");

        let mut config = minimal_valid_config();
        let err = update_config_field(&config_path, &mut config, "banana", json!("yellow"), true)
            .unwrap_err();
        assert!(err.to_string().contains("not a recognized"));
    }

    #[test]
    fn update_config_field_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");

        let mut config = minimal_valid_config();
        let result =
            update_config_field(&config_path, &mut config, "language", json!("en"), true).unwrap();
        assert!(result.valid, "errors: {:?}", result.errors);
        assert_eq!(config["language"], "en");
    }

    // --- is_encompassed_by tests ---

    // --- Graph medium scope validation ---

    // --- version parsing (semver) ---

    #[test]
    fn parse_accepts_valid_semver_version() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "version": "1.2.3-beta.4"
        });
        let parsed = parse_mem_config(&cfg).expect("valid semver should parse");
        let v = parsed.version.expect("version present");
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert!(!v.pre.is_empty());
    }

    #[test]
    fn parse_rejects_invalid_semver_version() {
        // "1.2" is not valid semver — must be MAJOR.MINOR.PATCH.
        let cfg = json!({
            "schema": "default@1.0.0",
            "version": "1.2"
        });
        let err = parse_mem_config(&cfg).expect_err("invalid semver must fail at parse");
        let msg = format!("{err}");
        assert!(
            msg.contains("version"),
            "error should mention version: {msg}"
        );
    }

    #[test]
    fn parse_rejects_non_semver_garbage_version() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "version": "potato"
        });
        let err = parse_mem_config(&cfg).expect_err("garbage must fail at parse");
        let msg = format!("{err}");
        assert!(
            msg.contains("version"),
            "error should mention version: {msg}"
        );
    }

    // --- readMems: `{ source: { type, … } }` entries — no path or
    //     version fields (the cached archive's config is authoritative) ---

    #[test]
    fn parse_accepts_read_mems_with_local_source() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "readMems": {
                "internal-notes": { "source": { "type": "local" } }
            }
        });
        let check = check_config(&cfg);
        assert!(check.valid, "errors: {:?}", check.errors);
        let parsed = parse_mem_config(&cfg).expect("valid readMems must parse");
        let spec = parsed
            .read_mems
            .get("internal-notes")
            .expect("entry present");
        assert!(matches!(spec.source, ReadMemSource::Local));
    }

    #[test]
    fn parse_accepts_read_mems_with_url_source() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "readMems": {
                "aws-patterns": {
                    "source": {
                        "type": "url",
                        "url": "https://example.com/aws-patterns.mem"
                    }
                }
            }
        });
        let check = check_config(&cfg);
        assert!(check.valid, "errors: {:?}", check.errors);
        let parsed = parse_mem_config(&cfg).expect("valid readMems must parse");
        let spec = parsed.read_mems.get("aws-patterns").expect("entry present");
        match &spec.source {
            ReadMemSource::Url { url } => {
                assert_eq!(url, "https://example.com/aws-patterns.mem")
            }
            _ => panic!("expected Url source, got {:?}", spec.source),
        }
    }

    #[test]
    fn parse_accepts_empty_read_mems_map() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "readMems": {}
        });
        let parsed = parse_mem_config(&cfg).expect("empty readMems must parse");
        assert!(parsed.read_mems.is_empty());
    }

    #[test]
    fn parse_accepts_omitted_read_mems() {
        let cfg = json!({
            "schema": "default@1.0.0"
        });
        let parsed = parse_mem_config(&cfg).expect("omitted readMems must parse");
        assert!(parsed.read_mems.is_empty());
    }

    #[test]
    fn check_rejects_read_mem_without_source() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "readMems": { "p": {} }
        });
        let check = check_config(&cfg);
        assert!(!check.valid);
        assert!(
            check.errors.iter().any(|e| e.contains("source")),
            "errors: {:?}",
            check.errors
        );
    }

    #[test]
    fn check_rejects_read_mem_with_unknown_source_type() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "readMems": {
                "p": { "source": { "type": "ftp", "url": "ftp://..." } }
            }
        });
        let check = check_config(&cfg);
        assert!(!check.valid);
        assert!(
            check
                .errors
                .iter()
                .any(|e| e.contains("unknown source type")),
            "errors: {:?}",
            check.errors
        );
    }

    #[test]
    fn check_rejects_url_source_with_empty_url() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "readMems": {
                "p": { "source": { "type": "url", "url": "" } }
            }
        });
        let check = check_config(&cfg);
        assert!(!check.valid);
        assert!(
            check.errors.iter().any(|e| e.contains("url source")),
            "errors: {:?}",
            check.errors
        );
    }

    #[test]
    fn check_rejects_registry_source_type_reserved_for_phase_d() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "readMems": {
                "p": { "source": { "type": "registry" } }
            }
        });
        let check = check_config(&cfg);
        // `registry` is a reserved future source type but is not yet
        // accepted by the schema — validation must reject it until the
        // registry ships.
        assert!(!check.valid);
        assert!(
            check
                .errors
                .iter()
                .any(|e| e.contains("unknown source type")),
            "errors: {:?}",
            check.errors
        );
    }

    /// Guards the `BTreeMap` choice: serialized read_mems must come out
    /// in key-sorted order regardless of insertion order, so config files
    /// on disk and log output are reproducible. A future "optimisation"
    /// that reintroduces `HashMap` would break this.
    #[test]
    fn read_mems_serialization_order_is_key_sorted() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "readMems": {
                "zebra": { "source": { "type": "local" } },
                "alpha": { "source": { "type": "local" } },
                "mango": { "source": { "type": "local" } }
            }
        });
        let parsed = parse_mem_config(&cfg).expect("valid config must parse");
        let reserialized = serde_json::to_string(&parsed).expect("serialization must succeed");
        let alpha = reserialized.find("alpha").expect("alpha present");
        let mango = reserialized.find("mango").expect("mango present");
        let zebra = reserialized.find("zebra").expect("zebra present");
        assert!(
            alpha < mango && mango < zebra,
            "expected alpha < mango < zebra, got: {reserialized}"
        );
    }

    // ----- vcs field -----

    #[test]
    fn vcs_config_round_trips_through_serde_with_both_fields() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "vcs": { "gitdir": "../.git", "worktree": ".." }
        });
        let parsed = parse_mem_config(&cfg).expect("valid config must parse");
        let vcs = parsed.vcs.as_ref().expect("vcs must be Some");
        assert_eq!(vcs.gitdir, "../.git");
        assert_eq!(vcs.worktree, "..");

        // Round-trip.
        let reserialized = serde_json::to_value(&parsed).unwrap();
        let round = parse_mem_config(&reserialized).expect("round-trip parse");
        assert_eq!(round.vcs.as_ref().unwrap().gitdir, "../.git");
        assert_eq!(round.vcs.as_ref().unwrap().worktree, "..");
    }

    #[test]
    fn vcs_config_worktree_defaults_to_dot_when_omitted() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "vcs": { "gitdir": ".git" }
        });
        let parsed = parse_mem_config(&cfg).expect("valid config must parse");
        let vcs = parsed.vcs.as_ref().expect("vcs must be Some");
        assert_eq!(vcs.gitdir, ".git");
        assert_eq!(vcs.worktree, ".", "worktree must default to \".\"");
    }

    #[test]
    fn vcs_config_absent_is_none() {
        let cfg = json!({ "schema": "default@1.0.0"  });
        let parsed = parse_mem_config(&cfg).expect("valid config must parse");
        assert!(parsed.vcs.is_none(), "missing vcs must deserialize to None");
    }

    #[test]
    fn vcs_field_tolerates_legacy_string_value() {
        // Legacy macOS-app sentinel. The tolerant deserializer must keep
        // the config loadable (returning None) without touching the
        // user's file by hand.
        let cfg = json!({
            "schema": "default@1.0.0",
            "vcs": "system"
        });
        let parsed = parse_mem_config(&cfg).expect("legacy vcs string must parse");
        assert!(
            parsed.vcs.is_none(),
            "legacy string must deserialize to None"
        );
    }

    #[test]
    fn published_config_strips_vcs() {
        // `published_config_from` must drop the `vcs` block — VCS layout
        // is workspace-local mechanics, never part of the published
        // mem's identity. This is guaranteed by the whitelist
        // projection: `PublishedMemConfig` has no `vcs` field, so a
        // MemConfig carrying `vcs: Some(...)` projects to a
        // PublishedMemConfig with no `vcs` on the wire.
        let mut cfg = MemConfig {
            name: Some("demo".to_string()),
            version: Some(semver::Version::new(0, 1, 0)),
            description: None,
            authors: None,
            schema: Some("default@1.0.0".parse().unwrap()),
            write_guidance: HashMap::new(),
            rules: None,
            publish: None,
            language: None,
            read_mems: BTreeMap::new(),
            community: None,
            vcs: None,
            unregistered_at: None,
            sync_state: BTreeMap::new(),
            extra: HashMap::new(),
        };
        cfg.vcs = Some(VcsConfig {
            gitdir: ".git".to_string(),
            worktree: ".".to_string(),
        });
        let published = published_config_from(&cfg, "").expect("valid projection");
        let wire = serde_json::to_value(&published).expect("serialize");
        assert!(
            wire.get("vcs").is_none(),
            "published wire form must not carry vcs: got {wire}"
        );
    }

    // ----- belongsTo legacy tombstone -----

    /// `belongsTo` is now a tombstone — cross-mem authorization
    /// migrated to the workspace-level `[cross_mem_links]` section in
    /// `.memstead/workspace.toml`. A per-mem config blob carrying `belongsTo` is
    /// rejected with `LEGACY_FIELD_PRESENT`.
    #[test]
    fn belongs_to_field_is_legacy_tombstone() {
        let cfg = json!({
            "schema": "default@1.0.0",
            "belongsTo": ["main"]
        });
        let result = check_config(&cfg);
        assert!(!result.valid, "belongsTo presence must fail validation");
        assert_eq!(result.error_code.as_deref(), Some("LEGACY_FIELD_PRESENT"));
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("belongsTo") && e.contains("cross_mem_links")),
            "tombstone error must name the field and the replacement section: {:?}",
            result.errors
        );
    }
}
