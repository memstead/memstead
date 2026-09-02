//! `schema migrate` — rewrite an authoring package's retired keys into
//! the current schema language, by exactly the translations the loader
//! applies to sealed packages.
//!
//! The loader keeps every retired key as a serde sentinel on the type
//! structs (the `legacy_*` fields in `types.rs`): sealed content is
//! translated so shipped packages keep loading, authoring content
//! refuses with a rename pointer so the author acts. This module is the
//! act: it gives a directory package the same translation the sealed
//! path performs, as a reviewable rewrite of the author's own files.
//!
//! One table. [`LEGACY_KEYS`] is the only enumeration of retired keys
//! and their rewrites. The suite pins it against the sentinel
//! declarations in `types.rs` (every `legacy_*` sentinel names a table
//! row and vice versa), and the verb proves each rewrite faithful at
//! run time: the original loads through the tolerant sealed-style read,
//! the rewrite through the strict authoring read, and what the loader
//! resolved from each must agree. A rewrite that does not reproduce the
//! loader's translation refuses instead of writing.
//!
//! Text, not a YAML round-trip: `serde_yaml_ng` drops comments, and an
//! author package is source — comments, key order, and spacing are the
//! author's. The rewriter tracks the block-mapping path line by line,
//! edits only the lines the table names, and leaves every other byte
//! alone. Keys it cannot reach (quoted keys, flow-style mappings) are
//! caught by the faithfulness check, never silently skipped.
//!
//! Polarity. The retired `optional:` key existed only under the pre-flip
//! language, where an ABSENT key meant required; the sealed read of an
//! unmarked package still gives it that meaning
//! ([`MetadataPolarityFormat::Legacy`]). A package that carries
//! `optional:` anywhere was therefore written under that language, and
//! the migration conserves what it meant: every metadata field
//! declaring neither key gets `required: true`, so the package says
//! under the current language exactly what the sealed read made of it.
//! The dry run shows each inserted line; deleting one is the author's
//! call, not the engine's.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::loader::{
    MetadataPolarityFormat, SCHEMA_FORMAT_MARKER_FILE, SchemaLoadError,
    load_authoring_package_from_memory, load_schema_from_memory_with_format,
};
use crate::schema::Schema;

/// Where in a type file a retired key sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyScope {
    /// A top-level key of a type file.
    TypeTop,
    /// A key on one entry of `metadata_fields:`.
    MetadataField,
    /// A key on one entry of `exemplar: relations:`.
    ExemplarRelation,
}

/// What the loader's sealed translation does with the key — and
/// therefore what the rewriter writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyRewrite {
    /// The key was renamed; its value carries over unchanged.
    Rename { to: &'static str },
    /// Dead vocabulary: the loader consumes nothing from it, so the key
    /// and its value are removed. `pointer` names the current-language
    /// key an author reaches for instead.
    Drop { pointer: &'static str },
    /// A boolean key of inverted polarity: `<retired>: true` becomes
    /// absence (the current default), `<retired>: false` becomes
    /// `<to>: true`. When the entry already carries `<to>`, the loader
    /// lets it win and the retired key is simply removed.
    InvertBool { to: &'static str },
}

/// One retired key: its spelling, where it sits, what replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyKey {
    pub retired: &'static str,
    pub scope: LegacyScope,
    pub rewrite: LegacyRewrite,
}

/// The retired keys the loader still reads on sealed content, with the
/// translation it applies — the single definition the migrate verb
/// rewrites from. Mirrors the `legacy_*` serde sentinels in `types.rs`
/// one-to-one; the suite fails when either side gains a key the other
/// lacks.
pub const LEGACY_KEYS: &[LegacyKey] = &[
    LegacyKey {
        retired: "propagating_relationships",
        scope: LegacyScope::TypeTop,
        rewrite: LegacyRewrite::Rename {
            to: "no_self_loop_relationships",
        },
    },
    LegacyKey {
        retired: "examples",
        scope: LegacyScope::TypeTop,
        rewrite: LegacyRewrite::Drop {
            pointer: "exemplar",
        },
    },
    LegacyKey {
        retired: "to",
        scope: LegacyScope::ExemplarRelation,
        rewrite: LegacyRewrite::Rename { to: "target" },
    },
    LegacyKey {
        retired: "type",
        scope: LegacyScope::ExemplarRelation,
        rewrite: LegacyRewrite::Rename { to: "rel_type" },
    },
    LegacyKey {
        retired: "optional",
        scope: LegacyScope::MetadataField,
        rewrite: LegacyRewrite::InvertBool { to: "required" },
    },
];

/// Why a migration could not be computed or written.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error("{path} is not a schema package directory (no schema.yaml)")]
    NotAPackage { path: PathBuf },

    #[error(
        "{path} is a sealed schema package (it carries `{marker}`, the seal marker), not \
         authoring input — `schema migrate` rewrites the directories you author, never a \
         sealed copy. Migrate the package's source directory instead.",
        marker = SCHEMA_FORMAT_MARKER_FILE
    )]
    SealedPackage { path: PathBuf },

    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{file} is not valid UTF-8; the rewriter edits text only")]
    NotUtf8 { file: String },

    #[error(
        "{file}:{line}: `{key}: {value}` cannot be rewritten mechanically — the retired \
         `{key}` key takes a YAML boolean here. Fix the value by hand, then retry."
    )]
    UnmigratableValue {
        file: String,
        line: usize,
        key: &'static str,
        value: String,
    },

    /// The package fails to load even under the tolerant sealed-style
    /// read that translates every retired key — so something other than
    /// a retired key is wrong, and rewriting spellings would not help.
    #[error(
        "the package does not load even with every retired key translated, so its problem is \
         not a retired key — run `memstead schema validate` and fix that first: {source}"
    )]
    PackageDoesNotLoad {
        #[source]
        source: SchemaLoadError,
    },

    /// The rewrite still refuses under the strict authoring read. The
    /// rewriter reached every key it can (block-style, unquoted); what
    /// remains is a spelling it does not edit.
    #[error(
        "the rewrite still refuses under the authoring read, so a retired key sits where the \
         rewriter does not edit (a quoted key, a flow-style mapping) — nothing was written; \
         fix that occurrence by hand and retry: {source}"
    )]
    RewriteLeavesViolations {
        #[source]
        source: SchemaLoadError,
    },

    /// The rewrite loads but resolves differently from the loader's
    /// own translation of the original — an internal defect, never an
    /// authoring problem. Nothing is written.
    #[error(
        "the rewrite does not reproduce the loader's translation of the original ({detail}); \
         nothing was written — this is an engine defect, please report it"
    )]
    Unfaithful { detail: String },
}

/// What one rewrite did to one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteAction {
    /// The key was renamed on its line; the value is untouched.
    Renamed { to: &'static str },
    /// The line (and any block value under it) was removed.
    Removed { reason: String },
    /// The line was replaced by another key/value pair.
    Replaced { with: String },
    /// A line was inserted after this one (polarity conservation).
    Inserted { line: String, reason: String },
}

impl fmt::Display for RewriteAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RewriteAction::Renamed { to } => write!(f, "renamed to `{to}`"),
            RewriteAction::Removed { reason } => write!(f, "removed ({reason})"),
            RewriteAction::Replaced { with } => write!(f, "replaced by `{with}`"),
            RewriteAction::Inserted { line, reason } => {
                write!(f, "gets `{line}` inserted ({reason})")
            }
        }
    }
}

/// One rewritten occurrence of a retired key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    /// 1-based line in the original file.
    pub line: usize,
    /// The retired key as written — or the key inserted, for
    /// [`RewriteAction::Inserted`].
    pub key: &'static str,
    /// Where the key sat, as a human path
    /// (`metadata_fields[source_family].optional`).
    pub path: String,
    pub action: RewriteAction,
}

impl fmt::Display for Rewrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: `{}` {}", self.line, self.path, self.action)
    }
}

/// One type file's migration: the rewrites and the resulting text.
#[derive(Debug, Clone)]
pub struct FileMigration {
    /// Package-relative path (`types/anchor.yaml`).
    pub rel_path: String,
    pub rewrites: Vec<Rewrite>,
    /// The file after the rewrites; equal to the original when
    /// `rewrites` is empty.
    pub new_text: String,
}

/// A metadata field that declared neither `optional:` nor `required:`
/// in a package written under the pre-flip language (it carried the
/// retired `optional:` key), and therefore received `required: true` —
/// the meaning the sealed read gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BareField {
    pub type_name: String,
    pub field: String,
}

/// The computed migration of one package. Computing never writes;
/// [`write_migration`] applies it.
#[derive(Debug, Clone)]
pub struct MigrateReport {
    pub package: PathBuf,
    /// `name@version` from the manifest.
    pub schema: String,
    /// Every type file, in stem order; files without rewrites carry an
    /// empty `rewrites` and their original text.
    pub files: Vec<FileMigration>,
    /// Whether the package was read as pre-flip content (it carries the
    /// retired `optional:` key), which is what made `required_added`
    /// non-empty.
    pub legacy_polarity: bool,
    /// See [`BareField`]. Empty unless `legacy_polarity`.
    pub required_added: Vec<BareField>,
}

impl MigrateReport {
    pub fn rewrite_count(&self) -> usize {
        self.files.iter().map(|f| f.rewrites.len()).sum()
    }

    pub fn is_noop(&self) -> bool {
        self.rewrite_count() == 0
    }

    /// The files that actually change.
    pub fn changed_files(&self) -> impl Iterator<Item = &FileMigration> {
        self.files.iter().filter(|f| !f.rewrites.is_empty())
    }
}

/// Compute the migration of the authoring package at `dir`. Reads only;
/// the report carries the rewritten texts for [`write_migration`].
pub fn migrate_package(dir: &Path) -> Result<MigrateReport, MigrateError> {
    let manifest_path = dir.join("schema.yaml");
    if !manifest_path.is_file() {
        return Err(MigrateError::NotAPackage {
            path: dir.to_path_buf(),
        });
    }
    if dir.join(SCHEMA_FORMAT_MARKER_FILE).is_file() {
        return Err(MigrateError::SealedPackage {
            path: dir.to_path_buf(),
        });
    }
    let manifest = read_text(&manifest_path, "schema.yaml")?;
    let schema_id = manifest_id(&manifest);

    let types_dir = dir.join("types");
    let mut stems: Vec<String> = Vec::new();
    if types_dir.is_dir() {
        let entries = std::fs::read_dir(&types_dir).map_err(|source| MigrateError::Io {
            path: types_dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| MigrateError::Io {
                path: types_dir.clone(),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".yaml") {
                stems.push(stem.to_string());
            }
        }
    }
    stems.sort();

    let mut before: Vec<(String, String)> = Vec::with_capacity(stems.len());
    for stem in &stems {
        let rel = format!("types/{stem}.yaml");
        before.push((
            stem.clone(),
            read_text(&types_dir.join(format!("{stem}.yaml")), &rel)?,
        ));
    }
    // Generation is a package-wide fact: one retired `optional:`
    // anywhere means the whole package was written pre-flip.
    let legacy_polarity = before.iter().any(|(_, text)| carries_legacy_optional(text));

    let mut files = Vec::with_capacity(stems.len());
    let mut required_added = Vec::new();
    for (stem, text) in &before {
        let rel = format!("types/{stem}.yaml");
        let outcome = rewrite_type_file(&rel, text, legacy_polarity)?;
        required_added.extend(outcome.required_added.into_iter().map(|field| BareField {
            type_name: stem.clone(),
            field,
        }));
        files.push(FileMigration {
            rel_path: rel,
            rewrites: outcome.rewrites,
            new_text: outcome.new_text,
        });
    }

    let report = MigrateReport {
        package: dir.to_path_buf(),
        schema: schema_id,
        files,
        legacy_polarity,
        required_added,
    };
    if report.is_noop() {
        return Ok(report);
    }

    // Faithfulness: the loader's own translation of the original — the
    // sealed-style read under the generation the package was written
    // in — is the specification; the rewrite must resolve to the same
    // schema under the strict authoring read.
    let after: Vec<(String, String)> = report
        .files
        .iter()
        .map(|f| {
            let stem = f
                .rel_path
                .trim_start_matches("types/")
                .trim_end_matches(".yaml");
            (stem.to_string(), f.new_text.clone())
        })
        .collect();
    let reference_format = if legacy_polarity {
        MetadataPolarityFormat::Legacy
    } else {
        MetadataPolarityFormat::RequiredOptIn
    };
    let original = load_schema_from_memory_with_format(&manifest, &before, reference_format)
        .map_err(|source| MigrateError::PackageDoesNotLoad { source })?;
    let migrated = load_authoring_package_from_memory(&manifest, &after)
        .map_err(|source| MigrateError::RewriteLeavesViolations { source })?;
    if let Some(detail) = first_difference(&original, &migrated) {
        return Err(MigrateError::Unfaithful { detail });
    }
    Ok(report)
}

/// Write the report's changed files in place. Files without rewrites
/// are not touched.
pub fn write_migration(report: &MigrateReport) -> Result<(), MigrateError> {
    for file in report.changed_files() {
        let path = report.package.join(&file.rel_path);
        std::fs::write(&path, &file.new_text).map_err(|source| MigrateError::Io {
            path: path.clone(),
            source,
        })?;
    }
    Ok(())
}

/// The author's follow-up commands after a `--write`, in order.
pub fn next_steps(package: &Path, schema: &str) -> Vec<String> {
    let dir = package.display();
    vec![
        format!("memstead schema validate {dir}"),
        format!("memstead schema install {dir}"),
        format!("memstead mem set-schema <mem> {schema}"),
    ]
}

fn read_text(path: &Path, rel: &str) -> Result<String, MigrateError> {
    let bytes = std::fs::read(path).map_err(|source| MigrateError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    String::from_utf8(bytes).map_err(|_| MigrateError::NotUtf8 {
        file: rel.to_string(),
    })
}

fn manifest_id(manifest: &str) -> String {
    #[derive(serde::Deserialize)]
    struct Head {
        #[serde(default)]
        name: String,
        #[serde(default)]
        version: String,
    }
    match serde_yaml_ng::from_str::<Head>(manifest) {
        Ok(h) if !h.name.is_empty() => format!("{}@{}", h.name, h.version),
        _ => "<unparsed manifest>".to_string(),
    }
}

/// The first place two loaded schemas disagree on what the legacy
/// translations resolve, or `None` when they agree.
fn first_difference(a: &Schema, b: &Schema) -> Option<String> {
    let mut names: Vec<&String> = a.types.keys().collect();
    names.sort();
    let mut other: Vec<&String> = b.types.keys().collect();
    other.sort();
    if names != other {
        return Some(format!("type set {names:?} vs {other:?}"));
    }
    for name in names {
        let ta = &a.types[name];
        let tb = &b.types[name];
        if ta.no_self_loop_relationships != tb.no_self_loop_relationships {
            return Some(format!(
                "type '{name}': no_self_loop_relationships {:?} vs {:?}",
                ta.no_self_loop_relationships, tb.no_self_loop_relationships
            ));
        }
        let fa: Vec<(&str, bool)> = ta
            .metadata_fields
            .iter()
            .map(|f| (f.key.as_str(), f.is_required()))
            .collect();
        let fb: Vec<(&str, bool)> = tb
            .metadata_fields
            .iter()
            .map(|f| (f.key.as_str(), f.is_required()))
            .collect();
        if fa != fb {
            return Some(format!(
                "type '{name}': metadata field requiredness {fa:?} vs {fb:?}"
            ));
        }
        let ra: Option<Vec<(&str, &str, Option<&str>)>> = ta.exemplar.as_ref().map(|e| {
            e.relations
                .iter()
                .map(|r| (r.target_slug(), r.rel_type_name(), r.description.as_deref()))
                .collect()
        });
        let rb: Option<Vec<(&str, &str, Option<&str>)>> = tb.exemplar.as_ref().map(|e| {
            e.relations
                .iter()
                .map(|r| (r.target_slug(), r.rel_type_name(), r.description.as_deref()))
                .collect()
        });
        if ra != rb {
            return Some(format!(
                "type '{name}': exemplar relations {ra:?} vs {rb:?}"
            ));
        }
    }
    None
}

// ---------------------------------------------------------------------
// The line-level rewriter
// ---------------------------------------------------------------------

/// One `key:` line the scanner found, with its block-mapping path.
#[derive(Debug)]
struct KeyLine<'a> {
    /// 0-based line index.
    idx: usize,
    /// Column of the key's first character.
    col: usize,
    key: &'a str,
    /// The inline value, comment stripped and trimmed; empty when the
    /// value is a nested block.
    value: &'a str,
    /// Frame names from the root down to and including this key;
    /// sequence items appear as `[]`.
    path: Vec<&'a str>,
    /// Line index of the frame this key is a child of — identifies the
    /// sequence item (or mapping) that groups sibling keys.
    parent: usize,
}

struct Frame<'a> {
    indent: usize,
    name: &'a str,
    line: usize,
}

const ITEM: &str = "[]";

/// Walk a block-style YAML document and report every `key:` line with
/// its path. Block scalars, plain multi-line scalars, and flow
/// collections are skipped as opaque values; comment and blank lines
/// are ignored.
fn scan_key_lines(text: &str) -> Vec<KeyLine<'_>> {
    let mut frames: Vec<Frame<'_>> = Vec::new();
    let mut out = Vec::new();
    // While `Some(col)`, lines indented deeper than `col` are the body
    // of an opaque value (block scalar, continuation, flow collection).
    let mut opaque_deeper_than: Option<usize> = None;

    for (idx, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start_matches(' ');
        if trimmed.trim().is_empty() {
            continue;
        }
        let indent = raw.len() - trimmed.len();
        if let Some(limit) = opaque_deeper_than {
            if indent > limit {
                continue;
            }
            opaque_deeper_than = None;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        let starts_item = trimmed == "-" || trimmed.starts_with("- ");
        // Close frames this line is a sibling (or outdent) of. A `- `
        // at the parent key's own indent is the compact sequence form
        // and keeps that key frame open.
        while let Some(top) = frames.last() {
            let close =
                top.indent > indent || (top.indent == indent && !(starts_item && top.name != ITEM));
            if close {
                frames.pop();
            } else {
                break;
            }
        }

        let mut content = trimmed;
        let mut col = indent;
        while content == "-" || content.starts_with("- ") {
            frames.push(Frame {
                indent: col,
                name: ITEM,
                line: idx,
            });
            let rest = &content[1..];
            let rest_trimmed = rest.trim_start_matches(' ');
            col += content.len() - rest_trimmed.len();
            content = rest_trimmed;
        }
        if content.is_empty() {
            continue;
        }
        let Some((key, value)) = split_key(content) else {
            // A scalar item or a flow value: opaque, may continue deeper.
            opaque_deeper_than = Some(col);
            continue;
        };
        let value = strip_comment(value).trim();
        let parent = frames.last().map(|f| f.line).unwrap_or(usize::MAX);
        frames.push(Frame {
            indent: col,
            name: key,
            line: idx,
        });
        out.push(KeyLine {
            idx,
            col,
            key,
            value,
            path: frames.iter().map(|f| f.name).collect(),
            parent,
        });
        if !value.is_empty() {
            // Scalar (single or multi-line), block scalar, or flow
            // collection: anything deeper belongs to it.
            opaque_deeper_than = Some(col);
        }
    }
    out
}

/// Split `key: value` on the first `:` that ends a plain, unquoted key.
fn split_key(content: &str) -> Option<(&str, &str)> {
    let first = content.chars().next()?;
    if matches!(
        first,
        '"' | '\'' | '[' | '{' | '&' | '*' | '!' | '|' | '>' | '%' | '@' | '`' | '?'
    ) {
        return None;
    }
    let bytes = content.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && i > 0 && bytes[i - 1] == b' ' {
            return None;
        }
        if *b == b':' && (i + 1 == bytes.len() || bytes[i + 1] == b' ') {
            let key = content[..i].trim_end();
            if key.is_empty() {
                return None;
            }
            return Some((key, &content[i + 1..]));
        }
    }
    None
}

/// Cut a trailing ` # comment` off a scalar value, outside quotes.
fn strip_comment(value: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for (i, c) in value.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && (prev == ' ' || i == 0) => return &value[..i],
            None => {}
        }
        prev = c;
    }
    value
}

fn matches_scope(path: &[&str], scope: LegacyScope) -> bool {
    match scope {
        LegacyScope::TypeTop => path.len() == 1,
        LegacyScope::MetadataField => {
            path.len() == 3 && path[0] == "metadata_fields" && path[1] == ITEM
        }
        LegacyScope::ExemplarRelation => {
            path.len() == 4 && path[0] == "exemplar" && path[1] == "relations" && path[2] == ITEM
        }
    }
}

enum Edit {
    Replace(usize, String),
    /// Inclusive line range.
    Delete(usize, usize),
    /// A new line placed after the given one.
    InsertAfter(usize, String),
}

struct FileOutcome {
    rewrites: Vec<Rewrite>,
    new_text: String,
    required_added: Vec<String>,
}

/// Does this type file carry the retired `optional:` key on a metadata
/// field? The generation signal for the whole package.
fn carries_legacy_optional(text: &str) -> bool {
    scan_key_lines(text)
        .iter()
        .any(|k| k.key == "optional" && matches_scope(&k.path, LegacyScope::MetadataField))
}

fn rewrite_type_file(
    rel: &str,
    text: &str,
    legacy_polarity: bool,
) -> Result<FileOutcome, MigrateError> {
    let lines: Vec<&str> = text.lines().collect();
    let keys = scan_key_lines(text);

    // Sibling knowledge per grouping frame: the `key:` value naming a
    // metadata field, and whether its entry carries `required:`.
    let field_name = |parent: usize| -> Option<&str> {
        keys.iter()
            .find(|k| k.parent == parent && k.key == "key" && k.path.len() == 3)
            .map(|k| k.value.trim_matches(|c| c == '"' || c == '\''))
    };
    let has_sibling = |parent: usize, key: &str| -> bool {
        keys.iter().any(|k| k.parent == parent && k.key == key)
    };

    let mut edits: Vec<Edit> = Vec::new();
    let mut rewrites: Vec<Rewrite> = Vec::new();

    for k in &keys {
        let Some(row) = LEGACY_KEYS
            .iter()
            .find(|row| row.retired == k.key && matches_scope(&k.path, row.scope))
        else {
            continue;
        };
        let raw = lines[k.idx];
        let human_path = match row.scope {
            LegacyScope::TypeTop => k.key.to_string(),
            LegacyScope::MetadataField => format!(
                "metadata_fields[{}].{}",
                field_name(k.parent).unwrap_or("?"),
                k.key
            ),
            LegacyScope::ExemplarRelation => format!("exemplar.relations[].{}", k.key),
        };
        let action = match row.rewrite {
            LegacyRewrite::Rename { to } => {
                let mut new_line = String::with_capacity(raw.len() + to.len());
                new_line.push_str(&raw[..k.col]);
                new_line.push_str(to);
                new_line.push_str(&raw[k.col + k.key.len()..]);
                edits.push(Edit::Replace(k.idx, new_line));
                RewriteAction::Renamed { to }
            }
            LegacyRewrite::Drop { pointer } => {
                edits.push(Edit::Delete(k.idx, block_end(&lines, k.idx, k.col)));
                RewriteAction::Removed {
                    reason: format!(
                        "retired and never consumed; author `{pointer}:` instead if you want it"
                    ),
                }
            }
            LegacyRewrite::InvertBool { to } => {
                let Ok(flag) = serde_yaml_ng::from_str::<bool>(k.value) else {
                    return Err(MigrateError::UnmigratableValue {
                        file: rel.to_string(),
                        line: k.idx + 1,
                        key: row.retired,
                        value: k.value.to_string(),
                    });
                };
                if has_sibling(k.parent, to) {
                    edits.push(Edit::Delete(k.idx, k.idx));
                    RewriteAction::Removed {
                        reason: format!("the entry's own `{to}:` wins"),
                    }
                } else if flag {
                    edits.push(Edit::Delete(k.idx, k.idx));
                    RewriteAction::Removed {
                        reason: "absence means optional".to_string(),
                    }
                } else {
                    let value_start = k.col + k.key.len();
                    let after_key = &raw[value_start..];
                    let colon = after_key.find(':').unwrap_or(0);
                    let tail = &after_key[colon + 1..];
                    let comment_at = tail.find(" #").map(|i| &tail[i..]).unwrap_or("");
                    let with = format!("{to}: true");
                    let new_line = format!("{}{with}{comment_at}", &raw[..k.col]);
                    edits.push(Edit::Replace(k.idx, new_line));
                    RewriteAction::Replaced { with }
                }
            }
        };
        rewrites.push(Rewrite {
            line: k.idx + 1,
            key: row.retired,
            path: human_path,
            action,
        });
    }

    // Polarity conservation: in a pre-flip package a field declaring
    // neither key meant required. Insert the current-language spelling
    // right under the field's `key:` line, at the same indentation.
    let mut required_added = Vec::new();
    if legacy_polarity {
        let mut seen = std::collections::BTreeSet::new();
        for k in &keys {
            if k.path.len() == 3
                && k.path[0] == "metadata_fields"
                && k.path[1] == ITEM
                && k.key == "key"
                && seen.insert(k.parent)
                && !has_sibling(k.parent, "optional")
                && !has_sibling(k.parent, "required")
            {
                let name = k.value.trim_matches(|c| c == '"' || c == '\'');
                let line = "required: true".to_string();
                edits.push(Edit::InsertAfter(
                    k.idx,
                    format!("{}{line}", " ".repeat(k.col)),
                ));
                rewrites.push(Rewrite {
                    line: k.idx + 1,
                    key: "required",
                    path: format!("metadata_fields[{name}]"),
                    action: RewriteAction::Inserted {
                        line,
                        reason: "written when an absent key meant required; delete the line where you did not mean it".to_string(),
                    },
                });
                required_added.push(name.to_string());
            }
        }
        rewrites.sort_by_key(|r| r.line);
    }

    Ok(FileOutcome {
        new_text: apply_edits(text, &lines, edits),
        rewrites,
        required_added,
    })
}

/// Last line (inclusive) of the block value that starts at `start`:
/// every following line indented deeper than `col`, blank lines
/// between them included, trailing blanks excluded.
fn block_end(lines: &[&str], start: usize, col: usize) -> usize {
    let mut end = start;
    for (j, raw) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = raw.trim_start_matches(' ');
        if trimmed.trim().is_empty() {
            continue;
        }
        if raw.len() - trimmed.len() > col {
            end = j;
        } else {
            break;
        }
    }
    end
}

fn apply_edits(text: &str, lines: &[&str], edits: Vec<Edit>) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    // Per original line: the (possibly replaced or deleted) line, then
    // anything inserted after it.
    let mut out: Vec<(Option<String>, Vec<String>)> = lines
        .iter()
        .map(|l| (Some((*l).to_string()), Vec::new()))
        .collect();
    for edit in edits {
        match edit {
            Edit::Replace(i, s) => out[i].0 = Some(s),
            Edit::Delete(a, b) => {
                for slot in out.iter_mut().take(b + 1).skip(a) {
                    slot.0 = None;
                }
            }
            Edit::InsertAfter(i, s) => out[i].1.push(s),
        }
    }
    let mut result = out
        .into_iter()
        .flat_map(|(line, inserted)| line.into_iter().chain(inserted))
        .collect::<Vec<_>>()
        .join(newline);
    if text.ends_with('\n') {
        result.push_str(newline);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::load_schema_from_dir;

    /// Every `legacy_*` serde sentinel in `types.rs`, by its retired
    /// spelling — read from the source so the table cannot drift from
    /// the loader: a sentinel added without a table row (or a row
    /// without a sentinel) fails here.
    fn sentinel_keys_in_source() -> std::collections::BTreeSet<String> {
        let source = include_str!("types.rs");
        let re = regex::Regex::new(
            r#"#\[serde\([^\n]*rename = "([^"]+)"[^\n]*\)\]\s*(?:#\[[^\n]*\]\s*)*pub legacy_\w+"#,
        )
        .unwrap();
        re.captures_iter(source).map(|c| c[1].to_string()).collect()
    }

    #[test]
    fn table_and_loader_sentinels_are_the_same_set() {
        let sentinels = sentinel_keys_in_source();
        let table: std::collections::BTreeSet<String> =
            LEGACY_KEYS.iter().map(|k| k.retired.to_string()).collect();
        assert_eq!(
            sentinels, table,
            "LEGACY_KEYS and the legacy_* sentinels in types.rs must name the same retired keys"
        );
        assert_eq!(sentinels.len(), 5, "sentinel scan lost a declaration");
    }

    fn manifest() -> String {
        r#"name: example
version: 1.0.0
description: Example schema for tests
when_to_use: In migrate tests only
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: Hierarchical containment
      default_weight: 3.0
    - name: _default
      description: Fallback weight
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
        .to_string()
    }

    /// A type file carrying one occurrence of the given retired key in
    /// its scope, with a comment beside it.
    fn type_with(row: &LegacyKey) -> String {
        let top = match (row.scope, row.retired) {
            (LegacyScope::TypeTop, "propagating_relationships") => {
                "propagating_relationships: [PART_OF] # keep me\n".to_string()
            }
            (LegacyScope::TypeTop, "examples") => {
                "examples: # dead list\n  - title: One\n    body: x\n\n  - title: Two\n".to_string()
            }
            (LegacyScope::TypeTop, _) => format!("{}: []\n", row.retired),
            _ => String::new(),
        };
        let field_extra = match row.scope {
            LegacyScope::MetadataField => format!("    {}: true # was optional\n", row.retired),
            _ => String::new(),
        };
        let exemplar = match row.scope {
            LegacyScope::ExemplarRelation => {
                let (to_key, type_key) = match row.retired {
                    "to" => ("to", "rel_type"),
                    _ => ("target", "type"),
                };
                format!(
                    "exemplar:\n  title: Sample one\n  metadata:\n    status: active\n  sections:\n    body: Text.\n  relations:\n    - {to_key}: other-thing\n      {type_key}: PART_OF\n"
                )
            }
            _ => String::new(),
        };
        format!(
            r#"name: sample
description: Sample type for tests
when_to_use: Whenever a minimal type is needed
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules:
      - One sentence describing the body.
metadata_fields:
  - key: status
    description: Lifecycle state
    field_type: string
    default_value: active
{field_extra}title_weight: 100.0
text_fields: [body]
hierarchy_relationship: PART_OF
{top}updatable_fields: [title, body, status]
health_required_fields: [body]
staleness_threshold_days: 90
{exemplar}write_rules:
  - Keep it short.
"#
        )
    }

    fn write_package(dir: &Path, type_text: &str) {
        std::fs::create_dir_all(dir.join("types")).unwrap();
        std::fs::write(dir.join("schema.yaml"), manifest()).unwrap();
        std::fs::write(dir.join("types/sample.yaml"), type_text).unwrap();
    }

    /// Per table row: the sealed read translates the key, the authoring
    /// read refuses it, and the verb's rewrite validates afterwards —
    /// the behavioural half of the one-table guarantee.
    #[test]
    fn every_row_translates_sealed_refuses_authoring_and_rewrites_clean() {
        for row in LEGACY_KEYS {
            let text = type_with(row);
            let types = vec![("sample".to_string(), text.clone())];
            // The sealed read of an unmarked package: Legacy polarity
            // when it carries `optional:`, current otherwise.
            let format = if carries_legacy_optional(&text) {
                MetadataPolarityFormat::Legacy
            } else {
                MetadataPolarityFormat::RequiredOptIn
            };
            load_schema_from_memory_with_format(&manifest(), &types, format)
                .unwrap_or_else(|e| panic!("sealed read must translate `{}`: {e}", row.retired));
            load_authoring_package_from_memory(&manifest(), &types)
                .expect_err(&format!("authoring read must refuse `{}`", row.retired));

            let dir = tempfile::tempdir().unwrap();
            write_package(dir.path(), &text);
            let report = migrate_package(dir.path())
                .unwrap_or_else(|e| panic!("migrate must compute for `{}`: {e}", row.retired));
            assert_eq!(
                report.rewrite_count(),
                1,
                "exactly one rewrite for `{}`: {:?}",
                row.retired,
                report.files
            );
            assert_eq!(
                std::fs::read_to_string(dir.path().join("types/sample.yaml")).unwrap(),
                text,
                "computing never writes"
            );
            write_migration(&report).unwrap();
            load_schema_from_dir(dir.path())
                .unwrap_or_else(|e| panic!("migrated `{}` must validate: {e}", row.retired));
        }
    }

    #[test]
    fn rename_keeps_value_comment_and_neighbours() {
        let row = &LEGACY_KEYS[0];
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &type_with(row));
        let report = migrate_package(dir.path()).unwrap();
        let file = report.changed_files().next().unwrap();
        assert!(
            file.new_text
                .contains("no_self_loop_relationships: [PART_OF] # keep me\n"),
            "{}",
            file.new_text
        );
        assert!(!file.new_text.contains("propagating_relationships"));
        assert_eq!(file.rewrites[0].path, "propagating_relationships");
        assert_eq!(
            file.rewrites[0].action,
            RewriteAction::Renamed {
                to: "no_self_loop_relationships"
            }
        );
        // Every other line survives byte for byte.
        let original = type_with(row);
        let before: Vec<&str> = original.lines().collect();
        let after: Vec<&str> = file.new_text.lines().collect();
        assert_eq!(before.len(), after.len());
        for (b, a) in before.iter().zip(&after) {
            if !b.starts_with("propagating_relationships") {
                assert_eq!(b, a);
            }
        }
    }

    #[test]
    fn drop_removes_the_whole_block_including_inner_blank_lines() {
        let row = LEGACY_KEYS
            .iter()
            .find(|r| r.retired == "examples")
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &type_with(row));
        let report = migrate_package(dir.path()).unwrap();
        let file = report.changed_files().next().unwrap();
        assert!(!file.new_text.contains("examples"));
        assert!(!file.new_text.contains("title: One"));
        assert!(!file.new_text.contains("title: Two"));
        assert!(
            file.new_text
                .contains("hierarchy_relationship: PART_OF\nupdatable_fields:"),
            "{}",
            file.new_text
        );
    }

    #[test]
    fn optional_false_becomes_required_true_and_sibling_required_wins() {
        let base = type_with(&LEGACY_KEYS[4]);
        let text = base.replace(
            "    optional: true # was optional\n",
            "    optional: false # must be set\n  - key: other\n    description: Another\n    field_type: string\n    required: true\n    optional: true\n",
        );
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &text);
        let report = migrate_package(dir.path()).unwrap();
        let file = report.changed_files().next().unwrap();
        assert!(
            file.new_text.contains("    required: true # must be set\n"),
            "{}",
            file.new_text
        );
        assert!(!file.new_text.contains("optional"));
        assert_eq!(file.rewrites.len(), 2);
        assert_eq!(file.rewrites[0].path, "metadata_fields[status].optional");
        assert_eq!(
            file.rewrites[0].action,
            RewriteAction::Replaced {
                with: "required: true".to_string()
            }
        );
        assert_eq!(file.rewrites[1].path, "metadata_fields[other].optional");
        assert!(matches!(
            &file.rewrites[1].action,
            RewriteAction::Removed { reason } if reason.contains("`required:` wins")
        ));
        assert!(report.required_added.is_empty());
    }

    /// A pre-flip package (it carries `optional:`) meant "required" by
    /// absence; the migration writes that meaning down, and the result
    /// resolves exactly as the sealed Legacy read of the original.
    #[test]
    fn bare_fields_get_required_true_only_in_a_pre_flip_package() {
        let text = type_with(&LEGACY_KEYS[4]).replace(
            "    optional: true # was optional\n",
            "    optional: true\n  - key: bare_one\n    description: No polarity key\n    field_type: string\n",
        );
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &text);
        let report = migrate_package(dir.path()).unwrap();
        assert!(report.legacy_polarity);
        assert_eq!(
            report.required_added,
            vec![BareField {
                type_name: "sample".to_string(),
                field: "bare_one".to_string(),
            }]
        );
        let file = report.changed_files().next().unwrap();
        assert!(
            file.new_text.contains(
                "  - key: bare_one\n    required: true\n    description: No polarity key\n"
            ),
            "{}",
            file.new_text
        );
        assert_eq!(file.rewrites.len(), 2);
        assert_eq!(file.rewrites[1].path, "metadata_fields[bare_one]");
        write_migration(&report).unwrap();
        let migrated = load_schema_from_dir(dir.path()).unwrap();
        let fields: Vec<(String, bool)> = migrated.types["sample"]
            .metadata_fields
            .iter()
            .filter(|f| f.key == "status" || f.key == "bare_one")
            .map(|f| (f.key.clone(), f.is_required()))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("status".to_string(), false),
                ("bare_one".to_string(), true)
            ]
        );

        // The same bare field in a current-language package (no
        // `optional:` anywhere) is left alone: nothing signals pre-flip.
        let current = type_with(&LEGACY_KEYS[0]).replace(
            "    default_value: active\n",
            "    default_value: active\n  - key: bare_one\n    description: No polarity key\n    field_type: string\n",
        );
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &current);
        let report = migrate_package(dir.path()).unwrap();
        assert!(!report.legacy_polarity);
        assert!(report.required_added.is_empty());
        assert_eq!(report.rewrite_count(), 1);
    }

    #[test]
    fn compact_sequence_form_is_tracked() {
        let text = type_with(&LEGACY_KEYS[4])
            .replace("metadata_fields:\n  - key: status", "metadata_fields:\n- key: status")
            .replace("    description: Lifecycle state", "  description: Lifecycle state")
            .replace("    field_type: string\n    default_value: active\n    optional: true # was optional\n", "  field_type: string\n  default_value: active\n  optional: true\n");
        assert!(text.contains("\n- key: status\n  description"), "{text}");
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &text);
        let report = migrate_package(dir.path()).unwrap();
        assert_eq!(report.rewrite_count(), 1);
        assert_eq!(
            report.changed_files().next().unwrap().rewrites[0].path,
            "metadata_fields[status].optional"
        );
    }

    #[test]
    fn nothing_to_migrate_is_a_noop_and_touches_nothing() {
        let clean = type_with(&LegacyKey {
            retired: "nothing",
            scope: LegacyScope::TypeTop,
            rewrite: LegacyRewrite::Rename { to: "nothing" },
        });
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &clean);
        let report = migrate_package(dir.path()).unwrap();
        assert!(report.is_noop());
        assert_eq!(report.schema, "example@1.0.0");
        write_migration(&report).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("types/sample.yaml")).unwrap(),
            clean
        );
    }

    #[test]
    fn non_boolean_optional_refuses_by_name() {
        let text =
            type_with(&LEGACY_KEYS[4]).replace("optional: true # was optional", "optional: maybe");
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &text);
        let err = migrate_package(dir.path()).unwrap_err();
        assert!(
            matches!(
                &err,
                MigrateError::UnmigratableValue { file, key, value, .. }
                    if file == "types/sample.yaml" && *key == "optional" && value == "maybe"
            ),
            "{err}"
        );
    }

    #[test]
    fn sealed_package_and_non_package_refuse() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            migrate_package(dir.path()).unwrap_err(),
            MigrateError::NotAPackage { .. }
        ));
        write_package(dir.path(), &type_with(&LEGACY_KEYS[0]));
        std::fs::write(dir.path().join(SCHEMA_FORMAT_MARKER_FILE), "{}\n").unwrap();
        assert!(matches!(
            migrate_package(dir.path()).unwrap_err(),
            MigrateError::SealedPackage { .. }
        ));
    }

    /// A retired key the rewriter does not edit (quoted) is caught by
    /// the faithfulness check instead of being written past.
    #[test]
    fn unreachable_spelling_refuses_instead_of_writing() {
        let text = type_with(&LEGACY_KEYS[0]).replace(
            "propagating_relationships: [PART_OF] # keep me",
            "\"propagating_relationships\": [PART_OF]",
        ) + "examples: []\n";
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &text);
        let err = migrate_package(dir.path()).unwrap_err();
        assert!(
            matches!(err, MigrateError::RewriteLeavesViolations { .. }),
            "{err}"
        );
    }

    #[test]
    fn scanner_paths() {
        let text = "a:\n  b: 1 # c\n  list:\n    - k: v\n      opt: true\n    - k: w\n  text: |\n    key: not a key\n    - nope: x\n  after: 2\n";
        let keys = scan_key_lines(text);
        let paths: Vec<String> = keys.iter().map(|k| k.path.join("/")).collect();
        assert_eq!(
            paths,
            vec![
                "a",
                "a/b",
                "a/list",
                "a/list/[]/k",
                "a/list/[]/opt",
                "a/list/[]/k",
                "a/text",
                "a/after"
            ]
        );
        assert_eq!(keys[1].value, "1");
        let items: Vec<usize> = keys
            .iter()
            .filter(|k| k.key == "k")
            .map(|k| k.parent)
            .collect();
        assert_ne!(items[0], items[1], "each sequence item groups its own keys");
    }
}
