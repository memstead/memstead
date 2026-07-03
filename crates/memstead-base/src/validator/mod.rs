//! Strict ingress validator for sealed `.mem` archives.
//!
//! Single trust boundary: every byte that enters any cache passes
//! through `validate_and_normalize_archive`. Parses the zip, validates
//! each markdown file against its declared schema over the **raw
//! bytes** (tolerant-parser fallbacks are the wrong default at
//! ingress), builds a `Store`, runs community detection, and
//! canonically re-packs. Fails hard on any violation.
//!
//! Pure function: takes `&[u8]`, returns `Result<ValidatedMem,
//! ValidationError>`, performs no I/O.

use crate::entity::Entity;
use crate::graph::LouvainOutput;
use crate::store::Store;

pub mod archive;
pub mod canonical;
pub mod config;
pub mod graph;
pub mod ids;
pub mod strict;

pub use graph::DanglingCrossMemEdge;
pub use memstead_schema::PublishedMemConfig;

/// Numeric limits enforced by archive-level checks. Callers that need
/// different caps (e.g. enterprise registry) construct a custom
/// instance; the default matches the published registry limits.
#[derive(Debug, Clone, Copy)]
pub struct ValidatorLimits {
    pub max_compressed_archive: u64,
    pub max_uncompressed_archive: u64,
    pub max_uncompressed_entry: u64,
    pub max_config_file: u64,
    pub max_file_count: u32,
    pub max_path_length: usize,
    pub max_path_depth: usize,
}

impl ValidatorLimits {
    pub const DEFAULT: Self = Self {
        max_compressed_archive: 2 * 1024 * 1024,
        max_uncompressed_archive: 20 * 1024 * 1024,
        max_uncompressed_entry: 1024 * 1024,
        max_config_file: 64 * 1024,
        max_file_count: 10_000,
        max_path_length: 512,
        max_path_depth: 16,
    };
}

impl Default for ValidatorLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Outcome of a bounded zip-entry read.
pub(crate) enum BoundedZipRead {
    Within(Vec<u8>),
    /// The entry's decompressed content exceeds the cap. The actual
    /// size is unknowable without reading it all — which is the attack —
    /// so only the cap is reported.
    ExceedsCap,
}

/// Read a zip entry with a hard cap on decompressed bytes.
///
/// Every direct archive read path shares this so decompression is never
/// sized by an attacker-declared header: the buffer grows only with
/// bytes actually decompressed, and reading stops at `cap + 1`. The same
/// idiom `archive::extract_entries` uses for the ingress validator.
pub(crate) fn read_zip_entry_bounded(
    reader: &mut impl std::io::Read,
    cap: u64,
) -> std::io::Result<BoundedZipRead> {
    use std::io::Read as _;
    let mut buf = Vec::new();
    reader.take(cap + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Ok(BoundedZipRead::ExceedsCap);
    }
    Ok(BoundedZipRead::Within(buf))
}

/// Which size cap a given `SizeCapExceeded` refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeCapKind {
    CompressedArchive,
    UncompressedArchive,
    UncompressedEntry,
    ConfigFile,
    EntryCount,
}

/// Aggregate statistics reported alongside a successful validation.
#[derive(Debug, Clone)]
pub struct MemStats {
    pub entities: usize,
    pub edges: usize,
    pub communities: usize,
    pub schema: memstead_schema::SchemaRef,
}

/// The successful output of validation: strict-checked entities, a
/// built graph, community assignments, and the canonical bytes that
/// should replace the input in any cache.
#[derive(Debug)]
pub struct ValidatedMem {
    pub config: PublishedMemConfig,
    pub entities: Vec<Entity>,
    pub store: Store,
    pub communities: LouvainOutput,
    pub stats: MemStats,
    pub canonical_bytes: Vec<u8>,
    /// Schema source files found under `.memstead/schema/` in the
    /// archive.
    /// Empty only if the archive was never meant to carry a schema
    /// (callers typically short-circuit before reaching here).
    /// `format: 2` archives (top-level `schema/` tree) are rejected
    /// upstream in `check_format` and never materialize here.
    pub schema_files: Vec<archive::SchemaFile>,
    /// Relationships whose target lives outside this mem — edges that
    /// can't resolve inside the single-mem archive. Always empty when
    /// produced via the strict (`validate_and_normalize_archive`) path:
    /// that path refuses on the first such edge. Populated only via
    /// [`validate_and_normalize_archive_lenient`] (the export side),
    /// which collects them so `export` can warn
    /// (`DANGLING_CROSS_MEM_EDGE_IN_EXPORT`) rather than refuse.
    pub dangling_cross_mem_edges: Vec<graph::DanglingCrossMemEdge>,
    /// Raw bytes of the archive's authoring-provenance payload
    /// (`.memstead/provenance.json`), or `None` when the archive carries
    /// none. Surfaced so the registry and other validation consumers can
    /// expose provenance without re-extracting the archive.
    pub provenance_bytes: Option<Vec<u8>>,
}

/// Every reason the validator can reject an archive. Each variant
/// carries enough context (file path, reason string, size numbers) to
/// give the caller an actionable error.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    // Archive-level
    #[error("zip error: {0}")]
    Zip(String),
    #[error("symlink entry is not allowed: {0}")]
    Symlink(String),
    #[error("unknown file type in archive: {0}")]
    UnknownFile(String),
    #[error("duplicate entry path: {0}")]
    DuplicateEntry(String),
    #[error("path too long ({len} > {limit}): {path}")]
    PathTooLong {
        path: String,
        len: usize,
        limit: usize,
    },
    #[error("path too deep ({depth} > {limit}): {path}")]
    PathTooDeep {
        path: String,
        depth: usize,
        limit: usize,
    },
    #[error("size cap exceeded ({kind:?}): {got} > {limit}")]
    SizeCapExceeded {
        kind: SizeCapKind,
        got: u64,
        limit: u64,
    },
    #[error("invalid UTF-8 at {path} offset {offset}")]
    Utf8 { path: String, offset: usize },

    // Config-level
    #[error("archive is missing .memstead/config.json")]
    MissingConfig,
    #[error("invalid config: {reason}")]
    InvalidConfig { reason: String },
    #[error("invalid name: {reason}")]
    InvalidName { reason: String },
    #[error("invalid version: {reason}")]
    InvalidVersion { reason: String },
    #[error("unsupported format: {got} (expected {expected})")]
    UnsupportedFormat { got: u32, expected: u32 },
    #[error("unknown schema '{name}@{version}' — not registered")]
    UnknownSchema {
        name: String,
        version: semver::Version,
    },
    #[error("unknown type: {name}")]
    UnknownType { name: String },

    #[error("embedded schema failed validation: {reason}")]
    EmbeddedSchemaInvalid { reason: String },
    #[error(
        "embedded schema pins '{embedded}' but `.memstead/config.json` declares '{declared}' — archive is inconsistent"
    )]
    EmbeddedSchemaMismatch { embedded: String, declared: String },

    // Entity-level
    #[error("missing frontmatter at {path}")]
    MissingFrontmatter { path: String },
    #[error("invalid frontmatter at {path}: {reason}")]
    InvalidFrontmatter { path: String, reason: String },
    #[error("unknown frontmatter key at {path}: {key}")]
    UnknownFrontmatterKey { path: String, key: String },
    #[error("missing required field at {path}: {field}")]
    MissingRequiredField { path: String, field: String },
    #[error("field type mismatch at {path}: {field} (expected {expected})")]
    FieldTypeMismatch {
        path: String,
        field: String,
        expected: String,
    },
    #[error("enum violation at {path}: {field} = {got}")]
    EnumViolation {
        path: String,
        field: String,
        got: String,
    },
    #[error("missing `# Title` at {path}")]
    MissingTitle { path: String },
    #[error("missing required section at {path}: {section}")]
    MissingRequiredSection { path: String, section: String },
    #[error("unknown section at {path}: {section}")]
    UnknownSection { path: String, section: String },
    #[error("malformed relationship line at {path}: {line}")]
    InvalidRelationshipLine { path: String, line: String },
    #[error("invalid relationship type at {path}: {rel_type}")]
    InvalidRelationshipType { path: String, rel_type: String },
    #[error("invalid wiki-link at {path}: {link} ({reason})")]
    InvalidWikiLink {
        path: String,
        link: String,
        reason: String,
    },
    #[error("unbalanced brackets at {path}")]
    UnbalancedBrackets { path: String },

    // Cross-archive / graph
    #[error("duplicate entity id {id} from {} and {}", paths.0, paths.1)]
    DuplicateEntityId { id: String, paths: (String, String) },
    #[error("cross-mem relationship at {path}: target {target}")]
    CrossMemRelationship { path: String, target: String },
    #[error("graph construction failed: {0}")]
    GraphConstructionFailed(String),
    #[error("community detection failed: {0}")]
    CommunityDetectionFailed(String),
}

/// The single ingress entry point. Callers (registry publish, CLI
/// install, MCP read-mem attach, macOS drop-to-install) hand in
/// bytes and receive either a `ValidatedMem` with canonical bytes
/// to install, or a typed `ValidationError`.
///
/// Runs every check with `ValidatorLimits::DEFAULT`. Use
/// `validate_and_normalize_archive_with_limits` to supply custom caps
/// (the registry may allow larger archives; the CLI keeps defaults).
pub fn validate_and_normalize_archive(bytes: &[u8]) -> Result<ValidatedMem, ValidationError> {
    validate_and_normalize_archive_with_limits(bytes, &ValidatorLimits::DEFAULT)
}

pub fn validate_and_normalize_archive_with_limits(
    bytes: &[u8],
    limits: &ValidatorLimits,
) -> Result<ValidatedMem, ValidationError> {
    // Strict posture: a cross-mem edge refuses the archive — the
    // install / archive-load contract.
    validate_impl(bytes, limits, true)
}

/// Export-side validation: identical strict checks, except a cross-mem
/// edge whose target won't travel inside this single-mem archive is
/// **collected** onto [`ValidatedMem::dangling_cross_mem_edges`]
/// instead of refused. Lets `export` warn
/// (`DANGLING_CROSS_MEM_EDGE_IN_EXPORT`) and still produce the archive,
/// while `install` keeps refusing the same edge — one predicate, two
/// postures. Every other strict
/// check (schema drift, malformed markdown, …) still refuses, so export
/// never emits an otherwise-invalid archive.
pub fn validate_and_normalize_archive_lenient(
    bytes: &[u8],
) -> Result<ValidatedMem, ValidationError> {
    validate_impl(bytes, &ValidatorLimits::DEFAULT, false)
}

/// Cross-mem-only scan over an archive's bytes: extract + tolerant
/// parse + the shared cross-mem predicate
/// ([`graph::dangling_cross_mem_edges_in`]), with **no** strict
/// section/field validation and no store construction. Returns every
/// edge whose target won't travel inside this single-mem archive.
///
/// This is the lightweight export-side detector for backends that don't
/// otherwise run the full archive validator (the git-branch export):
/// it surfaces exactly the edges `install` will refuse on, without
/// taking on the strict-validation refusal posture (which is a separate,
/// pre-existing concern). Tolerant parse means section/field drift does
/// not refuse here — only genuinely-unparseable markdown does.
pub fn collect_dangling_cross_mem_edges_from_bytes(
    bytes: &[u8],
) -> Result<Vec<graph::DanglingCrossMemEdge>, ValidationError> {
    let limits = &ValidatorLimits::DEFAULT;
    let entries = archive::extract_entries(bytes, limits)?;
    let config = config::parse_config_bytes(&entries.config_bytes)?;
    let embedded_schema = check_embedded_schema(&entries.schema_files, &config)?;
    let fallback_schema = graph::resolve_fallback_type(None);

    let mut parse_results = Vec::with_capacity(entries.markdown_files.len());
    for md in &entries.markdown_files {
        let raw = md.content.as_str();
        let raw_stripped = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let type_name = crate::entity::parser::peek_type_from_frontmatter(raw_stripped);
        let peeked_schema = type_name
            .as_deref()
            .and_then(|n| {
                embedded_schema
                    .as_ref()
                    .and_then(|s| s.get_type(n))
                    .or_else(|| memstead_schema::type_by_name(n))
            })
            .unwrap_or_else(|| fallback_schema.clone());

        let parse_result = crate::entity::parser::parse_markdown(
            raw_stripped,
            &md.path,
            &peeked_schema,
            &config.name,
        )
        .map_err(|e| map_parse_error(&md.path, &e))?;
        parse_results.push(parse_result);
    }

    Ok(graph::dangling_cross_mem_edges_in(
        &parse_results,
        &config.name,
    ))
}

fn validate_impl(
    bytes: &[u8],
    limits: &ValidatorLimits,
    cross_mem_as_error: bool,
) -> Result<ValidatedMem, ValidationError> {
    // 1. Archive-level: unzip, enforce caps + whitelist, UTF-8 decode.
    let entries = archive::extract_entries(bytes, limits)?;

    // 2. Config: strict-parse the meta-dir config bytes.
    let config = config::parse_config_bytes(&entries.config_bytes)?;

    // 2b. Embedded schema integrity. Any `.memstead/schema/` tree in the
    //     archive must (a) parse via the full loader and (b) declare
    //     the same `(name, version)` as `config.schema`. An archive
    //     whose embedded schema doesn't match its declared pin would
    //     extract schema-a into the cache while loading entities
    //     against schema-b — exactly the silent corruption the strict
    //     ingress boundary exists to prevent. The returned schema
    //     (when an embedded `.memstead/schema/` tree was present) is reused below as the
    //     authoritative source for per-entity type resolution so a
    //     user-defined schema's types validate against its own
    //     metadata rules, not against the builtin `default` fallback.
    let embedded_schema = check_embedded_schema(&entries.schema_files, &config)?;

    // 3. Decide the fallback type the Store will use for
    //    inline-link relationships and edge weights. Every listed
    //    type in the config has already resolved — graph.rs picks
    //    the first one; the runtime does the same during bulk-load
    //    via `engine_fallback_type` (spec), but we prefer the
    //    author's declared choice when available.
    let fallback_schema = graph::resolve_fallback_type(None);

    // 4. Per-entity: tolerant-parse + strict-check against raw bytes.
    //    The same parse_markdown call the runtime uses; strict layer
    //    catches what the tolerant parser papers over. When the
    //    archive carries an embedded schema, its type table wins over
    //    the builtin-default lookup; this lets a `recipe` archive
    //    validate its `recipe` entities against the `recipe` type
    //    definition instead of silently falling back to `spec`.
    let mut parse_results = Vec::with_capacity(entries.markdown_files.len());
    for md in &entries.markdown_files {
        let raw = md.content.as_str();
        let raw_stripped = raw.strip_prefix('\u{feff}').unwrap_or(raw);

        let type_name = crate::entity::parser::peek_type_from_frontmatter(raw_stripped);
        let peeked_schema = type_name
            .as_deref()
            .and_then(|n| {
                embedded_schema
                    .as_ref()
                    .and_then(|s| s.get_type(n))
                    .or_else(|| memstead_schema::type_by_name(n))
            })
            .unwrap_or_else(|| fallback_schema.clone());

        let parse_result = crate::entity::parser::parse_markdown(
            raw_stripped,
            &md.path,
            &peeked_schema,
            &config.name,
        )
        .map_err(|e| map_parse_error(&md.path, &e))?;

        strict::validate_strict(raw, &parse_result.entity, &peeked_schema, &md.path)?;

        parse_results.push(parse_result);
    }

    // 5. Entity-ID uniqueness (after all files parsed, before store
    //    construction — a duplicate would silently overwrite in
    //    upsert).
    let parsed_entities: Vec<Entity> = parse_results.iter().map(|pr| pr.entity.clone()).collect();
    ids::check_unique_ids(&parsed_entities)?;

    // 6. Graph: build store, detect communities, cross-mem guard.
    let graph_result = graph::build_and_check(
        parse_results,
        &fallback_schema,
        &config.name,
        cross_mem_as_error,
    )?;

    let (entity_count, edge_count) = graph::tally(&graph_result.store);

    let stats = MemStats {
        entities: entity_count,
        edges: edge_count,
        communities: graph_result.communities.count,
        schema: config.schema.clone(),
    };

    // 7. Canonical re-pack: regenerate markdown + canonical JSON,
    //    propagate schema files verbatim, write sorted zip with fixed
    //    mtime. Pinned by golden tests.
    let entities_for_canonical: Vec<Entity> = graph_result
        .store
        .all_entities()
        .filter(|e| !e.stub)
        .cloned()
        .collect();
    let canonical_bytes = canonical::canonical_bytes(
        &config,
        &entities_for_canonical,
        &entries.schema_files,
        embedded_schema.as_ref(),
        entries.provenance_bytes.as_deref(),
    )?;

    Ok(ValidatedMem {
        config,
        entities: parsed_entities,
        store: graph_result.store,
        communities: graph_result.communities,
        stats,
        canonical_bytes,
        schema_files: entries.schema_files,
        dangling_cross_mem_edges: graph_result.dangling_cross_mem_edges,
        provenance_bytes: entries.provenance_bytes,
    })
}

/// Run the embedded schema through the full loader and confirm its
/// manifest identity matches the config's `schema` pin. Returns the
/// loaded schema so downstream passes (entity parse/strict/canonical
/// repack) can resolve user-defined types against it. Empty
/// `schema_files` yields `Ok(None)` — the Engine's load-side
/// extraction pass then looks up the pin in the existing registry.
/// The `format: 3` publish path always embeds under `.memstead/schema/`,
/// so post-migration archives always hit the integrity branch.
fn check_embedded_schema(
    schema_files: &[archive::SchemaFile],
    config: &PublishedMemConfig,
) -> Result<Option<std::sync::Arc<memstead_schema::Schema>>, ValidationError> {
    if schema_files.is_empty() {
        return Ok(None);
    }
    let mut manifest: Option<&str> = None;
    let mut types: Vec<(String, String)> = Vec::with_capacity(schema_files.len());
    for sf in schema_files {
        if sf.archive_path == ".memstead/schema/schema.yaml" {
            manifest = Some(sf.content.as_str());
        } else if let Some(rest) = sf.archive_path.strip_prefix(".memstead/schema/types/")
            && let Some(stem) = rest.strip_suffix(".yaml")
        {
            types.push((stem.to_string(), sf.content.clone()));
        }
    }
    let Some(manifest_yaml) = manifest else {
        return Err(ValidationError::EmbeddedSchemaInvalid {
            reason:
                "`.memstead/schema/` tree present but `.memstead/schema/schema.yaml` is missing"
                    .into(),
        });
    };

    let schema = memstead_schema::load_schema_from_memory(manifest_yaml, &types).map_err(|e| {
        ValidationError::EmbeddedSchemaInvalid {
            reason: e.to_string(),
        }
    })?;

    let (embedded_name, embedded_version) = schema.id();
    if embedded_name != config.schema.name || embedded_version != config.schema.version {
        return Err(ValidationError::EmbeddedSchemaMismatch {
            embedded: format!("{embedded_name}@{embedded_version}"),
            declared: config.schema.as_display(),
        });
    }
    Ok(Some(std::sync::Arc::new(schema)))
}

fn map_parse_error(path: &str, e: &crate::entity::parser::ParseError) -> ValidationError {
    use crate::entity::parser::ParseError;
    match e {
        ParseError::MissingFrontmatter => ValidationError::MissingFrontmatter {
            path: path.to_string(),
        },
        ParseError::InvalidFrontmatter(reason) => ValidationError::InvalidFrontmatter {
            path: path.to_string(),
            reason: reason.clone(),
        },
        ParseError::MissingTitle => ValidationError::MissingTitle {
            path: path.to_string(),
        },
        ParseError::Io(err) => ValidationError::InvalidFrontmatter {
            path: path.to_string(),
            reason: err.to_string(),
        },
    }
}
