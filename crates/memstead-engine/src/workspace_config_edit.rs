//! `toml_edit`-backed writer for `.memstead/workspace.toml`.
//!
//! Backs the `memstead workspace allow-create / revoke-create / allow-delete /
//! revoke-delete / grant-cross-link / revoke-cross-link / set-mutations`
//! subcommand family. Every operation is a load → mutate → write triple;
//! `toml_edit` preserves operator-authored comments and formatting on
//! sections the CLI doesn't touch (cross-mem forward-reference
//! rationale, ingest-namespace pairings, operator-mode bypass semantics,
//! pattern-grammar examples).
//!
//! Errors carry symbolic codes (`WORKSPACE_NOT_INITIALISED`,
//! `RULE_ALREADY_EXISTS`, `RULE_NOT_FOUND`, `BEFORE_PATTERN_NOT_FOUND`,
//! `CROSS_LINK_ALREADY_GRANTED`, `CROSS_LINK_NOT_GRANTED`,
//! `CROSS_LINK_CONFLICT`, `INVALID_TOML`) so the CLI's typed exit envelope
//! lifts them as `code` in the `--json` payload. The CLI layer maps the
//! enum variants onto `CliError` / `ExitKind`.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, Value};

/// Errors returned by the writer.
///
/// The idempotency cases (`RuleAlreadyExists`, `RuleNotFound`,
/// `CrossLinkAlreadyGranted`, `CrossLinkNotGranted`) live on
/// [`WorkspaceEditWarning`] rather than here — re-grant / re-revoke /
/// re-add / re-remove return success-with-warning rather than refusing,
/// letting CLI scripts and MCP agents retry safely without
/// branching on prior state. `CrossLinkConflict` stays an error
/// because it's a real semantic conflict (wildcard vs. specific
/// list), not an idempotency case.
#[derive(Debug)]
pub enum WorkspaceEditError {
    /// `.memstead/workspace.toml` missing or unreadable. The workspace
    /// must be initialised before the CLI can edit its config.
    WorkspaceNotInitialised { path: PathBuf },
    /// Existing file failed to parse as TOML.
    InvalidToml { path: PathBuf, message: String },
    /// `add_create_rule` with `--before <p>` where `<p>` isn't an
    /// existing pattern in the section.
    BeforePatternNotFound {
        section: &'static str,
        pattern: String,
    },
    /// `grant_cross_link` with `*` against an existing specific list,
    /// or with a specific target against an existing `*`. Operators
    /// pick a single shape per `from`-mem.
    CrossLinkConflict { from: String, message: String },
    /// `add_create_rule` called for a pattern that already exists but
    /// with a **different** schema set. Refused rather than silently
    /// no-op'd: changing a pattern's schema pins is a security-relevant
    /// policy change, so it must be explicit (revoke the rule, then
    /// re-add with the new schemas) — never a silent success echoing a
    /// change that did not land. The genuine no-op (identical schemas)
    /// stays [`WorkspaceEditWarning::RuleAlreadyPresent`].
    RuleExistsSchemasDiffer {
        section: &'static str,
        pattern: String,
        stored: Vec<String>,
        requested: Vec<String>,
    },
    /// IO failure writing the file back.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Idempotency notices emitted by the writer when a call lands on
/// a state the caller intended (re-grant of an existing grant,
/// re-revoke of an absent grant, etc.). Surfacing these as warnings
/// rather than errors lets agents and scripts retry without branching
/// on prior state.
#[derive(Debug, Clone)]
pub enum WorkspaceEditWarning {
    /// `add_create_rule` / `add_delete_rule` called with a pattern
    /// that already has a matching entry. File unchanged.
    RuleAlreadyPresent {
        section: &'static str,
        pattern: String,
    },
    /// `remove_create_rule` / `remove_delete_rule` called with a
    /// pattern that has no matching entry. File unchanged.
    RuleNotFoundNoop {
        section: &'static str,
        pattern: String,
    },
    /// `grant_cross_link` called with a `(from, to)` pair already
    /// permitted (target already in the allowlist, or `*` already
    /// set). File unchanged.
    GrantAlreadyPresent { from: String, to: String },
    /// `revoke_cross_link` called with a `(from, to)` pair that
    /// isn't currently permitted. File unchanged.
    GrantNotFound { from: String, to: String },
    /// `grant_cross_link` named a `to` target that isn't a registered
    /// mem (and isn't the `*` wildcard). The grant still persists —
    /// the forward-reference workflow (grant before the target mem
    /// exists) is legitimate — but a likely typo is surfaced.
    CrossLinkTargetUnregistered { to: String },
    /// `grant_cross_link` named `to == from` — a self-grant. Intra-mem
    /// links never traverse the cross-link gate, so the grant is a
    /// no-op. It still persists; the meaninglessness is surfaced.
    CrossLinkSelfGrantNoop { mem: String },
}

impl WorkspaceEditWarning {
    /// Stable UPPER_SNAKE_CASE code surfaced as the CLI's stderr
    /// notice and as the MCP wrapper's warning envelope.
    pub fn code(&self) -> &'static str {
        match self {
            Self::RuleAlreadyPresent { .. } => "RULE_ALREADY_PRESENT",
            Self::RuleNotFoundNoop { .. } => "RULE_NOT_FOUND_NOOP",
            Self::GrantAlreadyPresent { .. } => "GRANT_ALREADY_PRESENT",
            Self::GrantNotFound { .. } => "GRANT_NOT_FOUND",
            Self::CrossLinkTargetUnregistered { .. } => "CROSS_LINK_TARGET_UNREGISTERED",
            Self::CrossLinkSelfGrantNoop { .. } => "CROSS_LINK_SELF_GRANT_NOOP",
        }
    }
}

impl std::fmt::Display for WorkspaceEditWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuleAlreadyPresent { section, pattern } => write!(
                f,
                "`[[{section}]]` already contains an entry for pattern `{pattern}` — file unchanged"
            ),
            Self::RuleNotFoundNoop { section, pattern } => write!(
                f,
                "`[[{section}]]` has no entry for pattern `{pattern}` — file unchanged"
            ),
            Self::GrantAlreadyPresent { from, to } => write!(
                f,
                "`[cross_mem_links]` already grants {from} → {to} — file unchanged"
            ),
            Self::GrantNotFound { from, to } => write!(
                f,
                "`[cross_mem_links]` does not grant {from} → {to} — file unchanged"
            ),
            Self::CrossLinkTargetUnregistered { to } => write!(
                f,
                "cross-link target `{to}` is not a registered mem — the grant is persisted (forward-reference is allowed) but will validate no relate until `{to}` exists"
            ),
            Self::CrossLinkSelfGrantNoop { mem } => write!(
                f,
                "self-grant `{mem} → {mem}` is a no-op — intra-mem links never traverse the cross-link gate; the grant is persisted but has no effect"
            ),
        }
    }
}

impl std::fmt::Display for WorkspaceEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorkspaceNotInitialised { path, .. } => write!(
                f,
                "no `.memstead/workspace.toml` at {} — run `memstead mem-repo init` or `memstead init` first",
                path.display()
            ),
            Self::InvalidToml { path, message } => {
                write!(f, "{}: failed to parse TOML — {message}", path.display())
            }
            Self::BeforePatternNotFound { section, pattern } => write!(
                f,
                "`--before {pattern}` did not match any existing `[[{section}]]` entry"
            ),
            Self::CrossLinkConflict { from, message } => write!(
                f,
                "`[cross_mem_links]` rejects edit for `{from}`: {message}"
            ),
            Self::RuleExistsSchemasDiffer {
                section,
                pattern,
                stored,
                requested,
            } => write!(
                f,
                "`[[{section}]]` already has a rule for pattern `{pattern}` pinned to schemas [{}], \
                 which differs from the requested [{}] — refusing to silently change the schema pins. \
                 To change them, revoke the rule first (`revoke_create {pattern}`) then re-add it with the new schemas",
                stored.join(", "),
                requested.join(", "),
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for WorkspaceEditError {}

impl WorkspaceEditError {
    /// Stable symbolic code used by the CLI exit envelope.
    pub fn code(&self) -> &'static str {
        match self {
            Self::WorkspaceNotInitialised { .. } => "WORKSPACE_NOT_INITIALISED",
            Self::InvalidToml { .. } => "INVALID_TOML",
            Self::BeforePatternNotFound { .. } => "BEFORE_PATTERN_NOT_FOUND",
            Self::CrossLinkConflict { .. } => "CROSS_LINK_CONFLICT",
            Self::RuleExistsSchemasDiffer { .. } => "RULE_EXISTS_SCHEMAS_DIFFER",
            Self::Io { .. } => "IO_ERROR",
        }
    }
}

/// Path of the workspace config file relative to the workspace root.
pub fn workspace_toml_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(memstead_base::WORKSPACE_STORE_DIR)
        .join("workspace.toml")
}

fn load(workspace_root: &Path) -> Result<(PathBuf, DocumentMut), WorkspaceEditError> {
    let path = workspace_toml_path(workspace_root);
    let text = fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            WorkspaceEditError::WorkspaceNotInitialised { path: path.clone() }
        } else {
            WorkspaceEditError::Io {
                path: path.clone(),
                source,
            }
        }
    })?;
    let doc: DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| WorkspaceEditError::InvalidToml {
                path: path.clone(),
                message: e.to_string(),
            })?;
    Ok((path, doc))
}

fn save(path: &Path, doc: &DocumentMut) -> Result<(), WorkspaceEditError> {
    fs::write(path, doc.to_string()).map_err(|source| WorkspaceEditError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Either-or shape mirroring `[cross_mem_links]` semantics on disk:
/// `<from> = "*"` (wildcard) or `<from> = ["a", "b"]` (allowlist). The
/// CLI exposes both via `--target *` and `--target <name>` on
/// `grant-cross-link`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossLinkTarget {
    /// `*` — every current writable target is permitted.
    Wildcard,
    /// One named target. Multiple grants accumulate into the
    /// underlying `<from> = ["a", "b", ...]` list on disk.
    Named(String),
}

impl CrossLinkTarget {
    /// Parse a CLI-supplied target token. `*` maps to the wildcard
    /// shape; anything else maps to the named shape verbatim. The
    /// caller validates name shape elsewhere (the engine's existing
    /// mem-name validation runs on load).
    pub fn parse(raw: &str) -> Self {
        if raw == "*" {
            Self::Wildcard
        } else {
            Self::Named(raw.to_string())
        }
    }
}

/// `memstead workspace allow-create <pattern> --schema <pin>[,…] [--cross-link …]
/// [--before <pattern>]` — append a `[[mem_management.create]]` rule.
/// Default ordering is append (lowest priority); `before` flags lift it
/// above the named pattern.
pub fn add_create_rule(
    workspace_root: &Path,
    pattern: &str,
    schemas: &[String],
    default_cross_links: Option<&[CrossLinkTarget]>,
    before: Option<&str>,
) -> Result<Vec<WorkspaceEditWarning>, WorkspaceEditError> {
    let (path, mut doc) = load(workspace_root)?;
    let section = ensure_array_of_tables(&mut doc, "mem_management", "create");

    if let Some(idx) = find_pattern_index(section, pattern) {
        // Pattern already present. Compare the stored schema set against
        // the requested one: identical → clean idempotent no-op; a
        // *difference* must not silently no-op (and must not echo a
        // change that did not land), so refuse with an actionable typed
        // error pointing at the revoke-then-readd recovery.
        let stored = read_rule_schemas(section, idx);
        if schema_sets_equal(&stored, schemas) {
            return Ok(vec![WorkspaceEditWarning::RuleAlreadyPresent {
                section: "mem_management.create",
                pattern: pattern.to_string(),
            }]);
        }
        return Err(WorkspaceEditError::RuleExistsSchemasDiffer {
            section: "mem_management.create",
            pattern: pattern.to_string(),
            stored,
            requested: schemas.to_vec(),
        });
    }

    let mut table = Table::new();
    table["pattern"] = Item::Value(Value::from(pattern));
    let mut arr = Array::new();
    for s in schemas {
        arr.push(s.as_str());
    }
    table["schemas"] = Item::Value(Value::Array(arr));
    if let Some(cross_links) = default_cross_links {
        table["default_cross_links"] = cross_link_value_item(cross_links);
    }

    if let Some(before_pattern) = before {
        let idx = find_pattern_index(section, before_pattern).ok_or_else(|| {
            WorkspaceEditError::BeforePatternNotFound {
                section: "mem_management.create",
                pattern: before_pattern.to_string(),
            }
        })?;
        // `ArrayOfTables` has no `insert(idx, table)`; emulate it by
        // detaching every entry from `idx` onward, pushing the new
        // table, then pushing the detached entries back.
        let mut tail = Vec::with_capacity(section.len() - idx);
        while section.len() > idx {
            let last = section.get(section.len() - 1).cloned().unwrap();
            tail.push(last);
            section.remove(section.len() - 1);
        }
        section.push(table);
        for entry in tail.into_iter().rev() {
            section.push(entry);
        }
    } else {
        section.push(table);
    }

    save(&path, &doc)?;
    Ok(Vec::new())
}

/// `memstead workspace revoke-create <pattern>` — remove a
/// `[[mem_management.create]]` rule by pattern.
pub fn remove_create_rule(
    workspace_root: &Path,
    pattern: &str,
) -> Result<Vec<WorkspaceEditWarning>, WorkspaceEditError> {
    let (path, mut doc) = load(workspace_root)?;
    let section = ensure_array_of_tables(&mut doc, "mem_management", "create");
    let idx = match find_pattern_index(section, pattern) {
        Some(i) => i,
        None => {
            return Ok(vec![WorkspaceEditWarning::RuleNotFoundNoop {
                section: "mem_management.create",
                pattern: pattern.to_string(),
            }]);
        }
    };
    section.remove(idx);
    save(&path, &doc)?;
    Ok(Vec::new())
}

/// `memstead workspace allow-delete <pattern>` — append a
/// `[[mem_management.delete]]` rule.
pub fn add_delete_rule(
    workspace_root: &Path,
    pattern: &str,
) -> Result<Vec<WorkspaceEditWarning>, WorkspaceEditError> {
    let (path, mut doc) = load(workspace_root)?;
    let section = ensure_array_of_tables(&mut doc, "mem_management", "delete");
    if find_pattern_index(section, pattern).is_some() {
        return Ok(vec![WorkspaceEditWarning::RuleAlreadyPresent {
            section: "mem_management.delete",
            pattern: pattern.to_string(),
        }]);
    }
    let mut table = Table::new();
    table["pattern"] = Item::Value(Value::from(pattern));
    section.push(table);
    save(&path, &doc)?;
    Ok(Vec::new())
}

/// `memstead workspace revoke-delete <pattern>` — remove a
/// `[[mem_management.delete]]` rule by pattern.
pub fn remove_delete_rule(
    workspace_root: &Path,
    pattern: &str,
) -> Result<Vec<WorkspaceEditWarning>, WorkspaceEditError> {
    let (path, mut doc) = load(workspace_root)?;
    let section = ensure_array_of_tables(&mut doc, "mem_management", "delete");
    let idx = match find_pattern_index(section, pattern) {
        Some(i) => i,
        None => {
            return Ok(vec![WorkspaceEditWarning::RuleNotFoundNoop {
                section: "mem_management.delete",
                pattern: pattern.to_string(),
            }]);
        }
    };
    section.remove(idx);
    save(&path, &doc)?;
    Ok(Vec::new())
}

/// `memstead workspace grant-cross-link <from> <to>` — add `to` to the
/// allowlist for `from` in `[cross_mem_links]`. `to == "*"` sets the
/// wildcard shape; named targets accumulate into a list.
pub fn grant_cross_link(
    workspace_root: &Path,
    from: &str,
    to: &CrossLinkTarget,
    known_mems: &[String],
) -> Result<Vec<WorkspaceEditWarning>, WorkspaceEditError> {
    // Diligence (matching the sibling `revoke_cross_link`'s warn-on-
    // anomaly behaviour): warn — never block — on a self-grant or an
    // unregistered named target. The grant still persists so the
    // forward-reference workflow (grant before the target mem exists)
    // stays open. The `*` wildcard is a legitimate non-mem token and
    // is not validated against the registered set.
    let mut warnings: Vec<WorkspaceEditWarning> = Vec::new();
    if let CrossLinkTarget::Named(name) = to {
        if name == from {
            warnings.push(WorkspaceEditWarning::CrossLinkSelfGrantNoop {
                mem: from.to_string(),
            });
        } else if !known_mems.iter().any(|v| v == name) {
            warnings.push(WorkspaceEditWarning::CrossLinkTargetUnregistered { to: name.clone() });
        }
    }

    let (path, mut doc) = load(workspace_root)?;
    let table = ensure_table(&mut doc, "cross_mem_links");
    match (table.get(from), to) {
        (None, CrossLinkTarget::Wildcard) => {
            table.insert(from, Item::Value(Value::from("*")));
        }
        (None, CrossLinkTarget::Named(name)) => {
            let mut arr = Array::new();
            arr.push(name.as_str());
            table.insert(from, Item::Value(Value::Array(arr)));
        }
        (Some(Item::Value(Value::String(s))), CrossLinkTarget::Wildcard) if s.value() == "*" => {
            warnings.push(WorkspaceEditWarning::GrantAlreadyPresent {
                from: from.to_string(),
                to: "*".to_string(),
            });
            return Ok(warnings);
        }
        (Some(Item::Value(Value::String(_))), _) => {
            return Err(WorkspaceEditError::CrossLinkConflict {
                from: from.to_string(),
                message: "wildcard `*` already set — revoke `*` before granting a named target"
                    .to_string(),
            });
        }
        (Some(Item::Value(Value::Array(_))), CrossLinkTarget::Wildcard) => {
            return Err(WorkspaceEditError::CrossLinkConflict {
                from: from.to_string(),
                message: "specific allowlist already set — revoke every entry before granting `*`"
                    .to_string(),
            });
        }
        (Some(Item::Value(Value::Array(arr))), CrossLinkTarget::Named(name)) => {
            if array_contains(arr, name) {
                warnings.push(WorkspaceEditWarning::GrantAlreadyPresent {
                    from: from.to_string(),
                    to: name.clone(),
                });
                return Ok(warnings);
            }
            let mut arr = arr.clone();
            arr.push(name.as_str());
            table.insert(from, Item::Value(Value::Array(arr)));
        }
        (Some(_), _) => {
            return Err(WorkspaceEditError::CrossLinkConflict {
                from: from.to_string(),
                message: "existing value is neither a string nor an array — fix by hand"
                    .to_string(),
            });
        }
    }
    save(&path, &doc)?;
    Ok(warnings)
}

/// `memstead workspace revoke-cross-link <from> <to>` — remove `to` from
/// the allowlist for `from`. When the underlying list becomes empty,
/// the `<from>` key is dropped entirely; `*` is matched as a literal.
pub fn revoke_cross_link(
    workspace_root: &Path,
    from: &str,
    to: &CrossLinkTarget,
) -> Result<Vec<WorkspaceEditWarning>, WorkspaceEditError> {
    let (path, mut doc) = load(workspace_root)?;
    let table = ensure_table(&mut doc, "cross_mem_links");
    let removed = match (table.get(from), to) {
        (None, _) => false,
        (Some(Item::Value(Value::String(s))), CrossLinkTarget::Wildcard) if s.value() == "*" => {
            table.remove(from);
            true
        }
        (Some(Item::Value(Value::String(_))), CrossLinkTarget::Named(_)) => false,
        (Some(Item::Value(Value::String(_))), CrossLinkTarget::Wildcard) => false,
        (Some(Item::Value(Value::Array(_))), CrossLinkTarget::Wildcard) => false,
        (Some(Item::Value(Value::Array(arr))), CrossLinkTarget::Named(name)) => {
            let mut arr = arr.clone();
            let original_len = arr.len();
            arr.retain(|v| match v {
                Value::String(s) => s.value() != name,
                _ => true,
            });
            if arr.len() == original_len {
                false
            } else if arr.is_empty() {
                table.remove(from);
                true
            } else {
                table.insert(from, Item::Value(Value::Array(arr)));
                true
            }
        }
        (Some(_), _) => false,
    };
    if !removed {
        let target = match to {
            CrossLinkTarget::Wildcard => "*".to_string(),
            CrossLinkTarget::Named(s) => s.clone(),
        };
        return Ok(vec![WorkspaceEditWarning::GrantNotFound {
            from: from.to_string(),
            to: target,
        }]);
    }
    save(&path, &doc)?;
    Ok(Vec::new())
}

/// `memstead workspace set-mutations --require-notes <bool>` — set the
/// `[mutations] require_notes` field. Creates the section on demand.
pub fn set_mutation_require_notes(
    workspace_root: &Path,
    value: bool,
) -> Result<(), WorkspaceEditError> {
    let (path, mut doc) = load(workspace_root)?;
    let table = ensure_table(&mut doc, "mutations");
    table.insert("require_notes", Item::Value(Value::from(value)));
    save(&path, &doc)
}

/// Scrub `.memstead/workspace.toml` of the now-dangling `[cross_mem_links]`
/// grants naming `mem_name` so the workspace no longer references a
/// mem the engine just destructively deleted. The
/// `[[mem_management.create]]` / `[[mem_management.delete]]`
/// allowlist rules are deliberately left intact.
///
/// Two passes, both on the in-memory `DocumentMut` before one final
/// save:
///   1. `[cross_mem_links]` — drop the key `mem_name` if present
///      (the deleted mem as `from`).
///   2. `[cross_mem_links]` — remove `mem_name` from every other
///      key's allowlist `Array`. When an allowlist becomes empty,
///      drop the key entirely (mirrors `revoke_cross_link`).
///
/// What is NOT scrubbed, and why: the `[[mem_management.create]]` /
/// `[[mem_management.delete]]` rules are forward-looking *permissions
/// for a name*, not references to the deleted *instance*. A cross-link
/// grant `test → other` names the gone instance and genuinely dangles,
/// so it is scrubbed; a `[[mem_management.create]]` rule for `other`
/// means "an agent may bring a mem named `other` into existence" and
/// stays true after the delete. Scrubbing it would silently revoke a
/// guarded-config permission as a side effect of an instance op, and
/// force a fresh `allow-create` before the next `mem init other` —
/// exactly the re-attach friction `unregister` avoids by preserving
/// policy. So `delete` is idempotent w.r.t. a later re-create.
///
/// Silent no-op cases (returns `Ok(())` without touching the file):
/// - `.memstead/workspace.toml` missing — pre-init workspaces (tests, ad-hoc
///   consumers) have no policy state to scrub. The caller (delete
///   orchestrator) already wrote nothing, so there's nothing to undo.
/// - `.memstead/workspace.toml` unparseable — surfaces as `InvalidToml`.
///
/// IO failures surface as `WorkspaceEditError::Io`; the caller's outer
/// engine-error type wraps that.
pub fn scrub_policy_for_deleted_mem(
    workspace_root: &Path,
    mem_name: &str,
) -> Result<Vec<ScrubbedEntry>, WorkspaceEditError> {
    let path = workspace_toml_path(workspace_root);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Pre-init or operator-edited away — nothing to scrub.
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(WorkspaceEditError::Io {
                path: path.clone(),
                source,
            });
        }
    };
    let mut doc: DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| WorkspaceEditError::InvalidToml {
                path: path.clone(),
                message: e.to_string(),
            })?;

    let mut scrubbed: Vec<ScrubbedEntry> = Vec::new();

    if let Some(item) = doc.get_mut("cross_mem_links")
        && let Some(table) = item.as_table_mut()
    {
        // The deleted mem's own key — every grant `mem_name
        // → <anything>` is dropped wholesale.
        if let Some(removed) = table.remove(mem_name) {
            let targets = match removed {
                Item::Value(Value::Array(arr)) => arr
                    .iter()
                    .filter_map(|v| match v {
                        Value::String(s) => Some(s.value().to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                Item::Value(Value::String(s)) => vec![s.value().to_string()],
                _ => Vec::new(),
            };
            if targets.is_empty() {
                scrubbed.push(ScrubbedEntry::CrossLink {
                    from: mem_name.to_string(),
                    to: "*".to_string(),
                });
            } else {
                for to in targets {
                    scrubbed.push(ScrubbedEntry::CrossLink {
                        from: mem_name.to_string(),
                        to,
                    });
                }
            }
        }
        // Peer entries referencing the deleted mem as a grant
        // target — drop the entry from the array; collapse empty
        // arrays.
        let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
        for key in keys {
            let drop_key = match table.get(&key) {
                Some(Item::Value(Value::Array(arr))) if array_contains(arr, mem_name) => {
                    let mut arr = arr.clone();
                    arr.retain(|v| match v {
                        Value::String(s) => s.value() != mem_name,
                        _ => true,
                    });
                    scrubbed.push(ScrubbedEntry::CrossLink {
                        from: key.clone(),
                        to: mem_name.to_string(),
                    });
                    if arr.is_empty() {
                        true
                    } else {
                        table.insert(&key, Item::Value(Value::Array(arr)));
                        false
                    }
                }
                _ => false,
            };
            if drop_key {
                table.remove(&key);
            }
        }
    }

    // The `[[mem_management.create]]` / `[[mem_management.delete]]`
    // allowlist rules are deliberately NOT scrubbed — see this
    // function's doc-comment for why deleting an instance must not
    // revoke the forward-looking permission to (re-)create a mem of
    // the same name.

    if !scrubbed.is_empty() {
        save(&path, &doc)?;
    }
    Ok(scrubbed)
}

/// One scrubbed entry returned from [`scrub_policy_for_deleted_mem`].
/// The `memstead_mem_delete` response surfaces these so an agent sees
/// every policy side effect in one round-trip. Only dangling
/// `[cross_mem_links]` grants are scrubbed; the
/// `[[mem_management.*]]` allowlist rules are preserved, so this enum
/// carries the one scrubbed shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrubbedEntry {
    /// `[cross_mem_links]` grant naming the deleted mem on
    /// either side.
    CrossLink {
        /// Source mem — the grant's table key.
        from: String,
        /// Target mem — array element or wildcard `"*"`.
        to: String,
    },
}

// --- internal helpers ---------------------------------------------------

fn ensure_table<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    if !doc.contains_key(name) {
        let mut t = Table::new();
        t.set_implicit(false);
        doc.insert(name, Item::Table(t));
    }
    doc.get_mut(name)
        .unwrap()
        .as_table_mut()
        .expect("ensured table shape")
}

fn ensure_array_of_tables<'a>(
    doc: &'a mut DocumentMut,
    outer: &str,
    inner: &str,
) -> &'a mut ArrayOfTables {
    if !doc.contains_key(outer) {
        let mut t = Table::new();
        t.set_implicit(true);
        doc.insert(outer, Item::Table(t));
    }
    let outer_table = doc
        .get_mut(outer)
        .and_then(|i| i.as_table_mut())
        .expect("mem_management must be a table");
    if !outer_table.contains_key(inner) {
        outer_table.insert(inner, Item::ArrayOfTables(ArrayOfTables::new()));
    }
    outer_table
        .get_mut(inner)
        .and_then(|i| i.as_array_of_tables_mut())
        .expect("ensured array-of-tables shape")
}

fn find_pattern_index(section: &ArrayOfTables, pattern: &str) -> Option<usize> {
    section
        .iter()
        .position(|t| t.get("pattern").and_then(|i| i.as_str()) == Some(pattern))
}

/// Read the `schemas` string list off the rule at `idx` in `section`.
/// Missing or malformed `schemas` reads as an empty list.
fn read_rule_schemas(section: &ArrayOfTables, idx: usize) -> Vec<String> {
    section
        .get(idx)
        .and_then(|t| t.get("schemas"))
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Set-equality over two schema-pin lists (order- and duplicate-
/// insensitive). A rule pinned to `[a, b]` re-added as `[b, a]` is the
/// same allowlist, so it stays a clean no-op rather than a refusal.
fn schema_sets_equal(a: &[String], b: &[String]) -> bool {
    let mut a: Vec<&str> = a.iter().map(String::as_str).collect();
    let mut b: Vec<&str> = b.iter().map(String::as_str).collect();
    a.sort_unstable();
    a.dedup();
    b.sort_unstable();
    b.dedup();
    a == b
}

fn cross_link_value_item(targets: &[CrossLinkTarget]) -> Item {
    if targets
        .iter()
        .any(|t| matches!(t, CrossLinkTarget::Wildcard))
    {
        Item::Value(Value::from("*"))
    } else {
        let mut arr = Array::new();
        for t in targets {
            if let CrossLinkTarget::Named(name) = t {
                arr.push(name.as_str());
            }
        }
        Item::Value(Value::Array(arr))
    }
}

fn array_contains(arr: &Array, needle: &str) -> bool {
    arr.iter().any(|v| match v {
        Value::String(s) => s.value() == needle,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const DEFAULT_BODY: &str =
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n";

    fn seed(body: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        let memstead = tmp.path().join(".memstead");
        fs::create_dir_all(&memstead).unwrap();
        fs::write(memstead.join("workspace.toml"), body).unwrap();
        tmp
    }

    fn read(root: &Path) -> String {
        fs::read_to_string(workspace_toml_path(root)).unwrap()
    }

    /// Registered-mem set for `grant_cross_link` target validation.
    /// Covers every named `to` target the grant tests use, so the
    /// behaviour-focused tests don't trip the `CROSS_LINK_TARGET_
    /// UNREGISTERED` warning. Target-validation behaviour is exercised
    /// by its own dedicated tests.
    fn known() -> Vec<String> {
        ["engine", "plugin", "macos", "specs", "default"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn add_create_rule_appends_by_default() {
        let tmp = seed(DEFAULT_BODY);
        add_create_rule(
            tmp.path(),
            "exec-*",
            &["default@1.0.0".to_string()],
            None,
            None,
        )
        .unwrap();
        let body = read(tmp.path());
        assert!(body.contains("[[mem_management.create]]"), "got:\n{body}");
        assert!(body.contains("pattern = \"exec-*\""), "got:\n{body}");
        assert!(
            body.contains("schemas = [\"default@1.0.0\"]"),
            "got:\n{body}"
        );
    }

    /// Adding a duplicate rule is idempotent — the call returns
    /// `Ok(vec![RuleAlreadyPresent])` rather than an error. Scripts and
    /// agents can retry safely without branching on prior state. The
    /// original file body is preserved (no spurious save).
    #[test]
    fn add_create_rule_duplicate_is_idempotent_with_warning() {
        let tmp = seed(DEFAULT_BODY);
        let first = add_create_rule(
            tmp.path(),
            "exec-*",
            &["default@1.0.0".to_string()],
            None,
            None,
        )
        .unwrap();
        assert!(first.is_empty(), "first add must return no warnings");
        let body_after_first = read(tmp.path());
        let warnings = add_create_rule(
            tmp.path(),
            "exec-*",
            &["default@1.0.0".to_string()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code(), "RULE_ALREADY_PRESENT");
        let body_after_second = read(tmp.path());
        assert_eq!(
            body_after_first, body_after_second,
            "duplicate add must not rewrite the file",
        );
    }

    /// MCP F3 / CLI: re-adding an existing pattern with a *different*
    /// schema set must NOT silently no-op (the deceptive "file unchanged"
    /// echoing a change that did not land). It refuses with a typed error
    /// naming the stored vs requested schemas, and the file is unchanged.
    #[test]
    fn add_create_rule_differing_schemas_refused_file_unchanged() {
        let tmp = seed(DEFAULT_BODY);
        add_create_rule(
            tmp.path(),
            "scratch",
            &["software@0.1.0".to_string()],
            None,
            None,
        )
        .unwrap();
        let body_before = read(tmp.path());

        let err = add_create_rule(
            tmp.path(),
            "scratch",
            &["nonexistent@9.9.9".to_string()],
            None,
            None,
        )
        .expect_err("differing schemas must be refused, not silently no-op'd");
        assert_eq!(err.code(), "RULE_EXISTS_SCHEMAS_DIFFER");
        match &err {
            WorkspaceEditError::RuleExistsSchemasDiffer {
                stored, requested, ..
            } => {
                assert_eq!(stored, &["software@0.1.0".to_string()]);
                assert_eq!(requested, &["nonexistent@9.9.9".to_string()]);
            }
            other => panic!("expected RuleExistsSchemasDiffer, got {other:?}"),
        }
        assert_eq!(
            body_before,
            read(tmp.path()),
            "refused schema change must not rewrite the file (stored schemas stay put)",
        );
    }

    /// Set-equality: the same schemas in a different order is the same
    /// allowlist — a clean idempotent no-op, not a refusal.
    #[test]
    fn add_create_rule_reordered_schemas_is_idempotent_noop() {
        let tmp = seed(DEFAULT_BODY);
        add_create_rule(
            tmp.path(),
            "scratch",
            &["a@1.0.0".to_string(), "b@1.0.0".to_string()],
            None,
            None,
        )
        .unwrap();
        let warnings = add_create_rule(
            tmp.path(),
            "scratch",
            &["b@1.0.0".to_string(), "a@1.0.0".to_string()],
            None,
            None,
        )
        .expect("reordered identical schema set must stay a no-op");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code(), "RULE_ALREADY_PRESENT");
    }

    /// The documented recovery works: revoke the rule, then re-add with
    /// the new schemas — the change lands and the stored pins update.
    #[test]
    fn revoke_then_readd_applies_the_new_schemas() {
        let tmp = seed(DEFAULT_BODY);
        add_create_rule(
            tmp.path(),
            "scratch",
            &["software@0.1.0".to_string()],
            None,
            None,
        )
        .unwrap();
        remove_create_rule(tmp.path(), "scratch").unwrap();
        let warnings = add_create_rule(
            tmp.path(),
            "scratch",
            &["planning@0.1.0".to_string()],
            None,
            None,
        )
        .expect("re-add after revoke must succeed");
        assert!(warnings.is_empty(), "fresh add returns no warnings");
        let body = read(tmp.path());
        assert!(
            body.contains("schemas = [\"planning@0.1.0\"]"),
            "new pins stored; got:\n{body}"
        );
        assert!(
            !body.contains("software@0.1.0"),
            "old pins gone; got:\n{body}"
        );
    }

    #[test]
    fn add_create_rule_before_lifts_priority() {
        let tmp = seed(DEFAULT_BODY);
        add_create_rule(
            tmp.path(),
            "z-*",
            &["default@1.0.0".to_string()],
            None,
            None,
        )
        .unwrap();
        add_create_rule(
            tmp.path(),
            "a-*",
            &["default@1.0.0".to_string()],
            None,
            Some("z-*"),
        )
        .unwrap();
        let body = read(tmp.path());
        let a_idx = body.find("pattern = \"a-*\"").expect("a-* must exist");
        let z_idx = body.find("pattern = \"z-*\"").expect("z-* must exist");
        assert!(
            a_idx < z_idx,
            "--before must place new rule above target; got:\n{body}"
        );
    }

    #[test]
    fn add_create_rule_before_unknown_pattern_errors() {
        let tmp = seed(DEFAULT_BODY);
        let err = add_create_rule(
            tmp.path(),
            "exec-*",
            &["default@1.0.0".to_string()],
            None,
            Some("does-not-exist"),
        )
        .unwrap_err();
        assert_eq!(err.code(), "BEFORE_PATTERN_NOT_FOUND");
    }

    #[test]
    fn add_create_rule_with_named_cross_links() {
        let tmp = seed(DEFAULT_BODY);
        add_create_rule(
            tmp.path(),
            "exec-*",
            &["default@1.0.0".to_string()],
            Some(&[CrossLinkTarget::Named("engine".to_string())]),
            None,
        )
        .unwrap();
        let body = read(tmp.path());
        assert!(
            body.contains("default_cross_links = [\"engine\"]"),
            "got:\n{body}"
        );
    }

    #[test]
    fn add_create_rule_with_wildcard_cross_links() {
        let tmp = seed(DEFAULT_BODY);
        add_create_rule(
            tmp.path(),
            "exec-*",
            &["default@1.0.0".to_string()],
            Some(&[CrossLinkTarget::Wildcard]),
            None,
        )
        .unwrap();
        let body = read(tmp.path());
        assert!(body.contains("default_cross_links = \"*\""), "got:\n{body}");
    }

    #[test]
    fn remove_create_rule_succeeds() {
        let tmp = seed(DEFAULT_BODY);
        add_create_rule(
            tmp.path(),
            "exec-*",
            &["default@1.0.0".to_string()],
            None,
            None,
        )
        .unwrap();
        remove_create_rule(tmp.path(), "exec-*").unwrap();
        let body = read(tmp.path());
        assert!(!body.contains("pattern = \"exec-*\""), "got:\n{body}");
    }

    /// Removing a non-existent rule is idempotent.
    /// Returns `Ok(vec![RuleNotFoundNoop])` rather than refusing.
    #[test]
    fn remove_create_rule_unknown_pattern_is_idempotent_with_warning() {
        let tmp = seed(DEFAULT_BODY);
        let body_before = read(tmp.path());
        let warnings = remove_create_rule(tmp.path(), "ghost").unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code(), "RULE_NOT_FOUND_NOOP");
        let body_after = read(tmp.path());
        assert_eq!(
            body_before, body_after,
            "no-op remove must not touch the file"
        );
    }

    #[test]
    fn add_and_remove_delete_rule() {
        let tmp = seed(DEFAULT_BODY);
        add_delete_rule(tmp.path(), "exec-*").unwrap();
        let body = read(tmp.path());
        assert!(body.contains("[[mem_management.delete]]"), "got:\n{body}");
        assert!(body.contains("pattern = \"exec-*\""), "got:\n{body}");
        remove_delete_rule(tmp.path(), "exec-*").unwrap();
        let body = read(tmp.path());
        assert!(!body.contains("pattern = \"exec-*\""), "got:\n{body}");
    }

    #[test]
    fn grant_cross_link_creates_named_list() {
        let tmp = seed(DEFAULT_BODY);
        grant_cross_link(
            tmp.path(),
            "plugin",
            &CrossLinkTarget::Named("engine".to_string()),
            &known(),
        )
        .unwrap();
        let body = read(tmp.path());
        assert!(body.contains("plugin = [\"engine\"]"), "got:\n{body}");
    }

    #[test]
    fn grant_cross_link_appends_named_target() {
        let tmp = seed(DEFAULT_BODY);
        grant_cross_link(
            tmp.path(),
            "macos",
            &CrossLinkTarget::Named("engine".to_string()),
            &known(),
        )
        .unwrap();
        grant_cross_link(
            tmp.path(),
            "macos",
            &CrossLinkTarget::Named("plugin".to_string()),
            &known(),
        )
        .unwrap();
        let body = read(tmp.path());
        assert!(
            body.contains("macos = [\"engine\", \"plugin\"]"),
            "got:\n{body}"
        );
    }

    #[test]
    fn grant_cross_link_wildcard_sets_string() {
        let tmp = seed(DEFAULT_BODY);
        grant_cross_link(tmp.path(), "specs", &CrossLinkTarget::Wildcard, &known()).unwrap();
        let body = read(tmp.path());
        assert!(body.contains("specs = \"*\""), "got:\n{body}");
    }

    /// Re-granting an existing grant is idempotent.
    /// Returns `Ok(vec![GrantAlreadyPresent])` and leaves the file
    /// unchanged.
    #[test]
    fn grant_cross_link_duplicate_named_is_idempotent_with_warning() {
        let tmp = seed(DEFAULT_BODY);
        grant_cross_link(
            tmp.path(),
            "plugin",
            &CrossLinkTarget::Named("engine".to_string()),
            &known(),
        )
        .unwrap();
        let body_before = read(tmp.path());
        let warnings = grant_cross_link(
            tmp.path(),
            "plugin",
            &CrossLinkTarget::Named("engine".to_string()),
            &known(),
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code(), "GRANT_ALREADY_PRESENT");
        let body_after = read(tmp.path());
        assert_eq!(
            body_before, body_after,
            "duplicate grant must not rewrite the file"
        );
    }

    #[test]
    fn grant_cross_link_named_over_wildcard_conflicts() {
        let tmp = seed(DEFAULT_BODY);
        grant_cross_link(tmp.path(), "plugin", &CrossLinkTarget::Wildcard, &known()).unwrap();
        let err = grant_cross_link(
            tmp.path(),
            "plugin",
            &CrossLinkTarget::Named("engine".to_string()),
            &known(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "CROSS_LINK_CONFLICT");
    }

    /// A named `to` target that isn't a registered mem warns
    /// `CROSS_LINK_TARGET_UNREGISTERED` — but the grant still persists
    /// (the forward-reference workflow stays open).
    #[test]
    fn grant_cross_link_warns_on_unregistered_named_target() {
        let tmp = seed(DEFAULT_BODY);
        let registered = vec!["plugin".to_string()];
        let warnings = grant_cross_link(
            tmp.path(),
            "plugin",
            &CrossLinkTarget::Named("future-mem".to_string()),
            &registered,
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code(), "CROSS_LINK_TARGET_UNREGISTERED");
        // Grant persisted despite the warning.
        assert!(
            read(tmp.path()).contains("plugin = [\"future-mem\"]"),
            "grant must persist for the forward-reference workflow: {}",
            read(tmp.path())
        );
    }

    /// A self-grant (`from == to`) warns `CROSS_LINK_SELF_GRANT_NOOP`
    /// and still persists.
    #[test]
    fn grant_cross_link_warns_on_self_grant() {
        let tmp = seed(DEFAULT_BODY);
        let warnings = grant_cross_link(
            tmp.path(),
            "plugin",
            &CrossLinkTarget::Named("plugin".to_string()),
            &known(),
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code(), "CROSS_LINK_SELF_GRANT_NOOP");
        assert!(read(tmp.path()).contains("plugin = [\"plugin\"]"));
    }

    /// The `*` wildcard is a legitimate non-mem token — it is NOT
    /// validated against the registered set, so granting `*` against an
    /// empty registry warns nothing.
    #[test]
    fn grant_cross_link_wildcard_not_target_validated() {
        let tmp = seed(DEFAULT_BODY);
        let warnings =
            grant_cross_link(tmp.path(), "plugin", &CrossLinkTarget::Wildcard, &[]).unwrap();
        assert!(
            warnings.is_empty(),
            "wildcard target must not be validated against the router: {warnings:?}"
        );
    }

    /// A registered named target grants with no warning (the normal
    /// path is unchanged).
    #[test]
    fn grant_cross_link_registered_target_no_warning() {
        let tmp = seed(DEFAULT_BODY);
        let registered = vec!["engine".to_string()];
        let warnings = grant_cross_link(
            tmp.path(),
            "plugin",
            &CrossLinkTarget::Named("engine".to_string()),
            &registered,
        )
        .unwrap();
        assert!(
            warnings.is_empty(),
            "registered target must warn nothing: {warnings:?}"
        );
    }

    #[test]
    fn revoke_cross_link_removes_named_target() {
        let tmp = seed(DEFAULT_BODY);
        grant_cross_link(
            tmp.path(),
            "macos",
            &CrossLinkTarget::Named("engine".to_string()),
            &known(),
        )
        .unwrap();
        grant_cross_link(
            tmp.path(),
            "macos",
            &CrossLinkTarget::Named("plugin".to_string()),
            &known(),
        )
        .unwrap();
        revoke_cross_link(
            tmp.path(),
            "macos",
            &CrossLinkTarget::Named("engine".to_string()),
        )
        .unwrap();
        let body = read(tmp.path());
        // toml_edit preserves the original array's inner whitespace
        // (e.g. `[ "plugin"]` if the original was `["engine", "plugin"]`).
        // Assert on the key + remaining target + the dropped target.
        assert!(body.contains("macos = ["), "got:\n{body}");
        assert!(body.contains("\"plugin\""), "got:\n{body}");
        assert!(
            !body.contains("\"engine\""),
            "engine target must be removed, got:\n{body}"
        );
    }

    #[test]
    fn revoke_cross_link_empties_key() {
        let tmp = seed(DEFAULT_BODY);
        grant_cross_link(
            tmp.path(),
            "macos",
            &CrossLinkTarget::Named("engine".to_string()),
            &known(),
        )
        .unwrap();
        revoke_cross_link(
            tmp.path(),
            "macos",
            &CrossLinkTarget::Named("engine".to_string()),
        )
        .unwrap();
        let body = read(tmp.path());
        assert!(
            !body.contains("macos"),
            "empty allowlist must drop the key, got:\n{body}"
        );
    }

    #[test]
    fn revoke_cross_link_wildcard() {
        let tmp = seed(DEFAULT_BODY);
        grant_cross_link(tmp.path(), "specs", &CrossLinkTarget::Wildcard, &known()).unwrap();
        revoke_cross_link(tmp.path(), "specs", &CrossLinkTarget::Wildcard).unwrap();
        let body = read(tmp.path());
        assert!(!body.contains("specs"), "got:\n{body}");
    }

    /// Revoking an absent grant is idempotent.
    /// Returns `Ok(vec![GrantNotFound])` and leaves the file
    /// unchanged.
    #[test]
    fn revoke_cross_link_not_granted_is_idempotent_with_warning() {
        let tmp = seed(DEFAULT_BODY);
        let body_before = read(tmp.path());
        let warnings = revoke_cross_link(
            tmp.path(),
            "macos",
            &CrossLinkTarget::Named("engine".to_string()),
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code(), "GRANT_NOT_FOUND");
        let body_after = read(tmp.path());
        assert_eq!(
            body_before, body_after,
            "no-op revoke must not touch the file"
        );
    }

    #[test]
    fn set_mutation_require_notes_creates_section() {
        let tmp = seed(DEFAULT_BODY);
        set_mutation_require_notes(tmp.path(), true).unwrap();
        let body = read(tmp.path());
        assert!(body.contains("[mutations]"), "got:\n{body}");
        assert!(body.contains("require_notes = true"), "got:\n{body}");
    }

    #[test]
    fn set_mutation_require_notes_toggles() {
        let tmp = seed(DEFAULT_BODY);
        set_mutation_require_notes(tmp.path(), true).unwrap();
        set_mutation_require_notes(tmp.path(), false).unwrap();
        let body = read(tmp.path());
        assert!(body.contains("require_notes = false"), "got:\n{body}");
    }

    #[test]
    fn missing_workspace_toml_errors_with_typed_code() {
        let tmp = TempDir::new().unwrap();
        let err = add_create_rule(tmp.path(), "exec-*", &[], None, None).unwrap_err();
        assert_eq!(err.code(), "WORKSPACE_NOT_INITIALISED");
    }

    #[test]
    fn comments_outside_edited_sections_survive() {
        // Operator-authored comments encode non-trivial knowledge
        // (forward-reference rationale, pattern-grammar examples,
        // operator-mode bypass semantics). toml_edit must preserve
        // every byte the CLI doesn't intentionally touch.
        let body = "# operator comment 1\n\
format = \"memstead-git-branch-2\"\n\
\n\
# operator comment 2\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
\n\
# section explanation that must survive\n\
[cross_mem_links]\n\
plugin = [\"engine\"]  # inline pin\n";
        let tmp = seed(body);

        add_create_rule(
            tmp.path(),
            "exec-*",
            &["default@1.0.0".to_string()],
            None,
            None,
        )
        .unwrap();

        let new_body = read(tmp.path());
        assert!(new_body.contains("# operator comment 1"));
        assert!(new_body.contains("# operator comment 2"));
        assert!(new_body.contains("# section explanation that must survive"));
        assert!(new_body.contains("# inline pin"));
        assert!(new_body.contains("[[mem_management.create]]"));
    }

    /// A destructive delete scrubs only the dangling `[cross_mem_links]`
    /// grants naming the deleted mem — its own key and every peer's
    /// allowlist value (with empty-list key drop). The
    /// `[[mem_management.create]]` / `[[mem_management.delete]]`
    /// allowlist rules survive unconditionally — even the exact-name
    /// ones — because they are forward-looking permissions for the name,
    /// not references to the gone instance.
    #[test]
    fn scrub_policy_for_deleted_mem_drops_cross_links_but_keeps_allowlist_rules() {
        let body = "format = \"memstead-git-branch-2\"\n\n\
            [cross_mem_links]\n\
            other = [\"test\"]\n\
            test = [\"other\", \"keep\"]\n\
            \n\
            [[mem_management.create]]\n\
            pattern = \"other\"\n\
            schemas = [\"default@1.0.0\"]\n\
            \n\
            [[mem_management.create]]\n\
            pattern = \"*\"\n\
            schemas = [\"default@1.0.0\"]\n\
            \n\
            [[mem_management.delete]]\n\
            pattern = \"other\"\n\
            \n\
            [[mem_management.delete]]\n\
            pattern = \"team/*\"\n";
        let tmp = seed(body);
        let scrubbed = scrub_policy_for_deleted_mem(tmp.path(), "other").unwrap();
        // Only cross-link grants are reported as scrubbed — never a
        // `mem_management.*` rule. Both the deleted mem's own key
        // (`other → test`) and the peer value (`test → other`) are
        // reported.
        assert!(
            scrubbed
                .iter()
                .all(|e| matches!(e, ScrubbedEntry::CrossLink { .. })),
            "scrub must report only cross-link grants, got: {scrubbed:?}"
        );
        assert!(
            scrubbed.contains(&ScrubbedEntry::CrossLink {
                from: "other".to_string(),
                to: "test".to_string(),
            }),
            "deleted mem's own grant must be reported scrubbed, got: {scrubbed:?}"
        );
        assert!(
            scrubbed.contains(&ScrubbedEntry::CrossLink {
                from: "test".to_string(),
                to: "other".to_string(),
            }),
            "peer grant naming the deleted mem must be reported scrubbed, got: {scrubbed:?}"
        );
        let after = read(tmp.path());
        // `other` key removed entirely.
        assert!(
            !after.contains("\nother = ["),
            "`other` key must be scrubbed from cross_mem_links — got:\n{after}"
        );
        // `other` value removed from `test`'s allowlist; `keep` survives.
        assert!(after.contains("\"keep\""), "non-target values must survive");
        // The exact-name `pattern = "other"` rules in BOTH
        // `[[mem_management.create]]` and `.delete]]` survive — the
        // forward-looking permission for the name `other` is preserved.
        assert_eq!(
            after.matches("pattern = \"other\"").count(),
            2,
            "exact-name mem_management.{{create,delete}} rules for `other` must survive — got:\n{after}"
        );
        assert!(
            after.contains("pattern = \"*\""),
            "wildcard `*` rule must survive"
        );
        assert!(
            after.contains("pattern = \"team/*\""),
            "glob `team/*` rule must survive"
        );
    }

    /// Acceptance complement: a refused (or pre-init) workspace.toml
    /// shouldn't crash the scrub. The function is best-effort — a
    /// missing file is a no-op and surfaces no error.
    #[test]
    fn scrub_policy_for_deleted_mem_missing_file_is_noop() {
        let tmp = TempDir::new().unwrap();
        // No `.memstead/workspace.toml` seeded.
        let outcome = scrub_policy_for_deleted_mem(tmp.path(), "other");
        assert!(outcome.is_ok(), "missing workspace.toml must not error");
    }

    /// Acceptance complement: a successful delete that doesn't touch
    /// any policy entry leaves the file byte-identical (no save).
    #[test]
    fn scrub_policy_for_deleted_mem_no_match_leaves_file_unchanged() {
        let body = "format = \"memstead-git-branch-2\"\n\n\
            [cross_mem_links]\n\
            test = [\"keep\"]\n\
            \n\
            [[mem_management.create]]\n\
            pattern = \"*\"\n\
            schemas = [\"default@1.0.0\"]\n";
        let tmp = seed(body);
        let before = read(tmp.path());
        scrub_policy_for_deleted_mem(tmp.path(), "ghost").unwrap();
        let after = read(tmp.path());
        assert_eq!(before, after, "unrelated delete must not rewrite the file");
    }

    /// Acceptance complement: when the only entry in an allowlist
    /// names the deleted mem, the underlying key is dropped — same
    /// shape as `revoke_cross_link`.
    #[test]
    fn scrub_policy_for_deleted_mem_drops_emptied_allowlist_key() {
        let body = "format = \"memstead-git-branch-2\"\n\n\
            [cross_mem_links]\n\
            test = [\"other\"]\n";
        let tmp = seed(body);
        scrub_policy_for_deleted_mem(tmp.path(), "other").unwrap();
        let after = read(tmp.path());
        assert!(
            !after.contains("\ntest = ["),
            "key whose allowlist drained to empty must be dropped — got:\n{after}"
        );
    }
}
