//! `memstead batch-update --from <file.json>` — update many entities in one call.
//!
//! Per-entry hash mode mirrors `memstead update`'s flag set:
//!
//! * `expected_hash: "..."` — strict optimistic lock.
//! * `auto_hash: true` — read the entity's current hash and use it.
//! * `force: true` — skip the hash check entirely.
//!
//! Exactly one of the three must be set per entry. Each entry resolves
//! its hash mode independently — a mixed-mode batch is fine.
//!
//! ```json
//! { "updates": [
//!     { "id": "specs--x", "expected_hash": "...",
//!       "sections": { "identity": "..." } },
//!     { "id": "specs--y", "auto_hash": true,
//!       "append_sections": { "specifies": "more" } },
//!     { "id": "specs--z", "force": true,
//!       "metadata": { "level": "M1" } }
//! ] }
//! ```

use std::path::PathBuf;

use clap::Parser;
use indexmap::IndexMap;
use serde::Deserialize;

use memstead_base::EntityId;
use memstead_base::ops::{PatchArg, RelateArg};
use memstead_base::{UpdateEntityArgs, vcs::Actor};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// JSON file with a top-level `updates: [...]` array.
    #[arg(long = "from", value_name = "FILE")]
    pub from: PathBuf,
}

/// Recognised mutation-content keys on an `EntryPayload`. Centralised
/// for the empty-mutation guard and the unknown-key suggestion hint.
const RECOGNISED_MUTATION_KEYS: &[&str] = &[
    "sections",
    "append_sections",
    "patch_sections",
    "metadata",
    "metadata_unset",
    "declare_relations",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryPayload {
    id: String,
    #[serde(default)]
    expected_hash: Option<String>,
    #[serde(default)]
    auto_hash: bool,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    sections: IndexMap<String, String>,
    #[serde(default)]
    append_sections: IndexMap<String, String>,
    #[serde(default)]
    patch_sections: IndexMap<String, PatchPayload>,
    #[serde(default)]
    metadata: IndexMap<String, String>,
    #[serde(default)]
    metadata_unset: Vec<String>,
    /// Inline relations to declare atomically before
    /// section/metadata mutations — mirrors `memstead_update.declare_relations`
    /// on the MCP surface. The CLI batch payload aligns with the
    /// recognised mutation-key set so the empty-mutation guard and
    /// `EMPTY_UPDATE` envelope cover this shape uniformly.
    #[serde(default)]
    declare_relations: Vec<RelationPayload>,
    /// Agent-authored provenance note for THIS entry's commit — matches
    /// the MCP mutation shape's `note`. Per-entry: distinct notes across
    /// batch entries are expressible. Optional; omit for note-less
    /// entries. (There is no batch-level `--note` flag, so no precedence
    /// question arises.)
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchPayload {
    old: String,
    new: String,
    #[serde(default)]
    all: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationPayload {
    /// Full target entity id.
    to: String,
    /// Rel-type (UPPER_SNAKE_CASE; engine canonicalises).
    #[serde(rename = "type")]
    rel_type: String,
    #[serde(default)]
    description: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let bytes = std::fs::read(&args.from).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "INVALID_INPUT",
            format!("failed to read {}: {e}", args.from.display()),
        )
    })?;

    // Two-phase parse so unknown-key refusals carry per-entry
    // `entry_index` / `unknown_keys` / `suggested` recovery payloads
    // instead of just serde's raw "unknown field" line/column text.
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!("invalid JSON in {}: {e}", args.from.display()),
        )
        .with_details(serde_json::json!({
            "path": args.from.display().to_string(),
            "parser_error": e.to_string(),
        }))
    })?;
    let updates_value = envelope.get("updates").cloned().unwrap_or_else(|| {
        serde_json::Value::Array(Vec::new())
    });
    let updates_array = match &updates_value {
        serde_json::Value::Array(a) => a.clone(),
        _ => {
            return Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                "`updates` must be a JSON array",
            )
            .into());
        }
    };
    // Surface top-level unknown keys too (e.g. `update` typo for `updates`).
    if let serde_json::Value::Object(map) = &envelope {
        let unknown: Vec<String> = map
            .keys()
            .filter(|k| k.as_str() != "updates")
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "unknown top-level key(s) {unknown:?} — only `updates: [...]` is recognised"
                ),
            )
            .with_details(serde_json::json!({
                "unknown_keys": unknown,
                "suggested": "updates",
            }))
            .into());
        }
    }

    if updates_array.is_empty() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "updates[] is empty",
        )
        .into());
    }

    let mut entries: Vec<EntryPayload> = Vec::with_capacity(updates_array.len());
    for (idx, entry_value) in updates_array.into_iter().enumerate() {
        match serde_json::from_value::<EntryPayload>(entry_value.clone()) {
            Ok(entry) => entries.push(entry),
            Err(e) => return Err(build_entry_parse_error(idx, &entry_value, &e).into()),
        }
    }

    let mut engine = crate::setup::pro_engine(&ctx)?;

    let updates: Vec<(UpdateEntityArgs, Option<String>)> = entries
        .into_iter()
        .map(|entry| build_update_args(&engine, entry))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let result = engine
        .batch_update(updates, Actor::Cli, None)
        .map_err(CliError::from_engine_op)?;
    // Reload-before-op runs inside `batch_update` for every mem the
    // batch touches; drain any `mem_changed` notice it stashed.
    let mem_changed = engine.take_mem_changed_notices();

    // A SUCCESSFUL batch renders exactly as before — the structured
    // result on stdout (`--json`) or the per-entry breakdown (human) —
    // and exits 0.
    if result.applied {
        if ctx.json {
            let mut body = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
            crate::commands::merge_mem_changed_json(&mut body, &mem_changed);
            print_json(&body)?;
        } else {
            let mut md = render_batch_markdown(&result);
            md.push_str(&crate::commands::render_mem_changed_block(&mem_changed));
            print_markdown(&md);
        }
        return Ok(());
    }

    // A FAILED batch (atomic refusal — nothing committed) is surfaced as
    // the standard error envelope (CLI F12): it carries a top-level
    // `code` and maps to a non-zero exit code via `ExitKind`, consistent
    // with single `update` and the documented exit-code table. A script
    // branching on `$?` (or `--json | jq -r .code`) now detects the
    // failure without parsing the per-entry envelope. The full result
    // rides on `details`, so no information is lost. In human mode the
    // per-entry breakdown still prints on stdout; the error summary
    // rides stderr. In `--json` mode the single error envelope is the
    // only thing on stdout, so it stays exactly one JSON document.
    if !ctx.json {
        print_markdown(&render_batch_markdown(&result));
    }
    Err(batch_refused_error(&result).into())
}

/// Render the per-entry markdown breakdown for a batch result (success
/// or failure). Each entry shows a status marker, its id/action, and any
/// per-entry error code+message; an applied batch appends its commit SHA.
fn render_batch_markdown(result: &memstead_base::ops::BatchResult) -> String {
    let header = if result.applied {
        format!(
            "# Batch update applied — {} item(s) in one commit",
            result.succeeded
        )
    } else {
        format!(
            "# Batch update REFUSED — {} item(s) failed, nothing committed",
            result.failed
        )
    };
    let mut lines = vec![header, String::new()];
    for entry in &result.results {
        let marker = if entry.error.is_some() {
            "✗"
        } else if entry.action == "not_applied" {
            "·"
        } else {
            "✓"
        };
        let detail = entry
            .error
            .as_ref()
            .map(|e| format!(" — [{}] {}", e.code, e.message))
            .unwrap_or_default();
        lines.push(format!("- {marker} `{}` ({}){}", entry.id, entry.action, detail));
    }
    if result.applied && !result.commit_sha.is_empty() {
        lines.push(String::new());
        lines.push(format!("Commit: `{}`", result.commit_sha));
    }
    lines.join("\n")
}

/// Build the error envelope for a refused (atomic) batch. The top-level
/// `code` is the stable `BATCH_REFUSED` token; the `ExitKind` mirrors the
/// dominant (refusal-tripping) entry's failure so `$?` matches single
/// `update` and the documented table (hash mismatch → 4, missing entity /
/// mem → 3, schema/policy refusal → 5). The full [`BatchResult`] rides
/// on `details` — per-entry codes stay available without re-running.
fn batch_refused_error(result: &memstead_base::ops::BatchResult) -> CliError {
    let dominant = result.results.iter().find(|e| e.error.is_some());
    let (code, failing_id, message) = match dominant {
        Some(entry) => {
            let err = entry.error.as_ref().expect("dominant entry has an error");
            (err.code.as_str(), entry.id.to_string(), err.message.clone())
        }
        None => ("", String::new(), "batch-update refused; nothing committed".to_string()),
    };
    let kind = batch_refused_exit_kind(code);
    let summary = format!(
        "batch-update refused — {} item(s) failed, nothing committed; first failure [{}] on `{}`: {}",
        result.failed, code, failing_id, message,
    );
    CliError::new(kind, "BATCH_REFUSED", summary)
        .with_details(serde_json::to_value(result).unwrap_or(serde_json::Value::Null))
}

/// Map the dominant per-entry failure code to the process exit code,
/// reusing the documented `0/1/3/4/5` taxonomy so a refused batch exits
/// the same way the equivalent single `memstead update` would. Unrecognised
/// codes fall to `Validation` (5) — the bucket for schema/policy refusals,
/// which is what most batch-entry failures are.
fn batch_refused_exit_kind(code: &str) -> ExitKind {
    match code {
        "HASH_MISMATCH" => ExitKind::HashMismatch,
        "ENTITY_NOT_FOUND" | "UNKNOWN_MEM" => ExitKind::NotFound,
        _ => ExitKind::Validation,
    }
}

/// Map a single JSON entry to the engine's [`UpdateEntityArgs`],
/// resolving the per-entry hash mode against the live engine: explicit
/// hash passes through, `auto_hash` reads the entity's current hash
/// and substitutes it, `force` clears the lock by setting
/// `expected_hash: None`. The mutually-exclusive contract mirrors
/// `memstead update`'s clap-level `conflicts_with_all`.
fn build_update_args(
    engine: &memstead_base::Engine,
    entry: EntryPayload,
) -> anyhow::Result<(UpdateEntityArgs, Option<String>)> {
    let mode_count = entry.expected_hash.is_some() as u8
        + entry.auto_hash as u8
        + entry.force as u8;
    if mode_count == 0 {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "entry `{}`: exactly one of `expected_hash`, `auto_hash`, or `force` must be set",
                entry.id
            ),
        )
        .into());
    }
    if mode_count > 1 {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "entry `{}`: `expected_hash`, `auto_hash`, and `force` are mutually exclusive",
                entry.id
            ),
        )
        .into());
    }

    // Per-entry provenance note rides alongside the args to the engine.
    let note = entry.note.clone();
    let id = EntityId::canonical(&entry.id);
    let expected_hash = if entry.force {
        None
    } else if entry.auto_hash {
        // Missing-entity case falls through with `expected_hash: None`
        // so the engine surfaces a typed `ENTITY_NOT_FOUND` for this
        // entry. Under atomic semantics that refuses the whole batch
        // (nothing commits) with this entry named in the result.
        engine.get_entity(&id).map(|e| e.content_hash.clone())
    } else {
        entry.expected_hash
    };

    let patch_sections = entry
        .patch_sections
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                PatchArg {
                    old: v.old,
                    new: v.new,
                    all: v.all,
                },
            )
        })
        .collect();

    let declare_relations = entry
        .declare_relations
        .into_iter()
        .map(|r| RelateArg {
            rel_type: r.rel_type,
            to: EntityId::canonical(&r.to),
            description: r.description,
        })
        .collect();

    Ok((
        UpdateEntityArgs {
            id,
            expected_hash,
            sections: entry.sections,
            append_sections: entry.append_sections,
            patch_sections,
            metadata: entry.metadata,
            metadata_unset: entry.metadata_unset,
            declare_relations,
            dry_run: false,
            relations_unset: Vec::new(),
        },
        note,
    ))
}

/// Build the typed CLI error envelope for a per-entry deserialisation
/// refusal. Walks the original JSON value to pick out keys not in the
/// recognised entry-shape vocabulary so the recovery payload carries
/// `entry_index`, `unknown_keys`, and a nearest-match `suggested`
/// hint pointing at the recognised mutation key whose name is closest
/// to the first unknown one (fuzzy-match shared with `memstead_schema`).
fn build_entry_parse_error(
    idx: usize,
    entry_value: &serde_json::Value,
    parse_err: &serde_json::Error,
) -> CliError {
    let known: std::collections::BTreeSet<&str> = [
        "id",
        "expected_hash",
        "auto_hash",
        "force",
        "sections",
        "append_sections",
        "patch_sections",
        "metadata",
        "metadata_unset",
        "declare_relations",
        "note",
    ]
    .into_iter()
    .collect();
    let mut unknown: Vec<String> = Vec::new();
    if let Some(map) = entry_value.as_object() {
        for k in map.keys() {
            if !known.contains(k.as_str()) {
                unknown.push(k.clone());
            }
        }
    }
    // Nearest-match suggestion for the first unknown key against the
    // recognised mutation-content vocabulary (the keys this plan adds
    // discipline to). Defaults to the literal vocabulary for callers
    // with no fuzzy hit.
    let suggested = unknown
        .first()
        .and_then(|u| nearest_recognised_key(u))
        .map(String::from);
    let message = if unknown.is_empty() {
        format!("entry {idx}: invalid shape — {parse_err}")
    } else {
        let display = unknown.join(", ");
        format!(
            "entry {idx}: unknown field(s) {display} — recognised mutation keys are {:?}",
            RECOGNISED_MUTATION_KEYS
        )
    };
    let mut details = serde_json::json!({
        "entry_index": idx,
        "unknown_keys": unknown,
        "parser_error": parse_err.to_string(),
        "recognised_keys": RECOGNISED_MUTATION_KEYS,
    });
    if let Some(s) = suggested {
        details["suggested"] = serde_json::Value::String(s);
    }
    CliError::new(ExitKind::Validation, "INVALID_INPUT", message).with_details(details)
}

/// Pick the recognised mutation-content key whose name is most
/// similar to `attempted`, by simple substring + prefix scoring. Good
/// enough for `section_replacements` → `sections`, `meta` →
/// `metadata`, `declares` → `declare_relations`. Returns `None` for
/// inputs with no plausible match.
fn nearest_recognised_key(attempted: &str) -> Option<&'static str> {
    let lower = attempted.to_lowercase();
    let mut best: Option<(&'static str, usize)> = None;
    for &key in RECOGNISED_MUTATION_KEYS {
        // Score: full-prefix > substring > shared-stem letters.
        let score = if lower.starts_with(key) || key.starts_with(&lower) {
            100
        } else if lower.contains(key) || key.contains(&lower) {
            80
        } else {
            shared_prefix_len(&lower, key) * 4
        };
        if score > 0 {
            best = match best {
                Some((_, best_score)) if best_score >= score => best,
                _ => Some((key, score)),
            };
        }
    }
    best.map(|(k, _)| k)
}

fn shared_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-entry deserialisation refusal carries `entry_index`,
    /// `unknown_keys`, and a nearest-match `suggested` hint pointing
    /// at the recognised mutation key whose name is closest to the
    /// first unknown one. Probe reproducer: `section_replacements`
    /// → `sections`.
    #[test]
    fn entry_parse_error_names_unknown_keys_and_suggests_nearest() {
        let entry: serde_json::Value = serde_json::json!({
            "id": "specs--target",
            "auto_hash": true,
            "section_replacements": {"identity": "X"},
        });
        let parse_err = serde_json::from_value::<EntryPayload>(entry.clone()).unwrap_err();
        let err = build_entry_parse_error(0, &entry, &parse_err);
        let details = err.details.expect("details payload must be present");
        assert_eq!(details["entry_index"].as_u64(), Some(0));
        let unknown: Vec<String> = details["unknown_keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(unknown, vec!["section_replacements".to_string()]);
        assert_eq!(details["suggested"].as_str(), Some("sections"));
        assert_eq!(err.code, "INVALID_INPUT");
    }

    /// Complement AC: a `meta` → `metadata` fuzzy hit lands.
    #[test]
    fn entry_parse_error_suggests_metadata_for_meta_typo() {
        let entry: serde_json::Value = serde_json::json!({
            "id": "specs--target",
            "auto_hash": true,
            "meta": {"level": "M1"},
        });
        let parse_err = serde_json::from_value::<EntryPayload>(entry.clone()).unwrap_err();
        let err = build_entry_parse_error(7, &entry, &parse_err);
        let details = err.details.expect("details payload");
        assert_eq!(details["entry_index"].as_u64(), Some(7));
        assert_eq!(details["suggested"].as_str(), Some("metadata"));
    }

    /// Complement AC: documented optional fields (`expected_hash`,
    /// `auto_hash`, `force`, mutation maps) all parse cleanly under
    /// `deny_unknown_fields`. Regression check that no documented
    /// field name became an unknown key by accident.
    #[test]
    fn entry_parse_accepts_every_documented_field() {
        let entry: serde_json::Value = serde_json::json!({
            "id": "specs--target",
            "expected_hash": "abc",
            "auto_hash": false,
            "force": false,
            "sections": {"identity": "A"},
            "append_sections": {"purpose": "B"},
            "patch_sections": {"identity": {"old": "X", "new": "Y", "all": true}},
            "metadata": {"level": "M1"},
            "metadata_unset": ["tags"],
            "declare_relations": [{"to": "specs--other", "type": "USES"}],
            "note": "per-entry provenance",
        });
        let parsed = serde_json::from_value::<EntryPayload>(entry).expect("must parse");
        assert_eq!(parsed.id, "specs--target");
        assert_eq!(parsed.sections.len(), 1);
        assert_eq!(parsed.declare_relations.len(), 1);
        assert_eq!(parsed.declare_relations[0].rel_type, "USES");
        // Per-entry note parses (distinct notes per batch entry).
        assert_eq!(parsed.note.as_deref(), Some("per-entry provenance"));
    }
}
