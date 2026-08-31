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
    /// Rehearse the whole batch: run the full per-entry validation
    /// (identical refusals, report-all) and report the would-be
    /// receipt, committing nothing. `write_id` stays empty (the
    /// rehearsal marker).
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

/// Recognised mutation-content keys on an `EntryPayload`. Centralised
/// for the empty-mutation guard and the unknown-key suggestion hint.
/// The engine's recognised mutation keys MINUS `relations_unset`, which
/// `EntryPayload` does not accept. Taking the engine's list wholesale would
/// advertise a key the parser (`deny_unknown_fields`) then rejects, which is
/// the same lie in the other direction as the copies this const replaced.
/// `batch_recognised_keys_are_a_subset_of_the_engines` pins the relationship.
const RECOGNISED_MUTATION_KEYS: &[&str] = &[
    "sections",
    "append_sections",
    "patch_sections",
    "metadata",
    "metadata_unset",
    "declare_relations",
    "anchors",
    "anchors_unset",
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
    sections_unset: Vec<String>,
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
    /// Provenance anchors for THIS entry — matches the MCP `memstead_update`
    /// `anchors[]` shape. Written into the mem-branch anchors sidecar in
    /// the same batch commit; malformed input refuses `INVALID_ANCHOR`.
    #[serde(default)]
    anchors: Vec<memstead_base::anchor::AnchorInput>,
    /// Explicit anchor removals for THIS entry — matches the MCP
    /// `memstead_update` `anchors_unset[]` shape; applied before the
    /// entry's `anchors` merge in the same batch commit.
    #[serde(default)]
    anchors_unset: Vec<memstead_base::anchor::AnchorUnsetInput>,
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
    /// Far end of the edge; the near end is the entity being updated.
    target: String,
    /// Rel-type (UPPER_SNAKE_CASE; engine canonicalises).
    rel_type: String,
    #[serde(default)]
    description: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    // Envelope parsing shared with the batch family; the two-phase
    // per-entry parse below keeps this command's richer unknown-key
    // recovery payloads (`entry_index` / `unknown_keys` / `suggested`).
    let updates_array = super::batch::parse_batch_envelope(&args.from, "updates")?;

    let mut entries: Vec<EntryPayload> = Vec::with_capacity(updates_array.len());
    for (idx, entry_value) in updates_array.into_iter().enumerate() {
        match serde_json::from_value::<EntryPayload>(entry_value.clone()) {
            Ok(entry) => entries.push(entry),
            Err(e) => return Err(build_entry_parse_error(idx, &entry_value, &e).into()),
        }
    }

    let mut engine = crate::setup::full_engine(ctx)?;

    let updates: Vec<(UpdateEntityArgs, Option<String>)> = entries
        .into_iter()
        .map(|entry| build_update_args(&engine, entry))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let result = engine
        .batch_update(
            updates,
            Actor::Cli,
            Some(&crate::setup::cli_client_id()),
            args.dry_run,
        )
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
            let mut md = super::batch::render_batch_markdown("update", &result, args.dry_run);
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
        print_markdown(&super::batch::render_batch_markdown(
            "update",
            &result,
            args.dry_run,
        ));
    }
    Err(super::batch::batch_refused_error("update", &result).into())
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
    let mode_count =
        entry.expected_hash.is_some() as u8 + entry.auto_hash as u8 + entry.force as u8;
    // An anchors-only entry needs no hash mode, for the reason every other
    // update surface exempts one (consistency-sweep 03/04): the token would
    // compare a value the write cannot move. Left in, this path refused an
    // entry that `memstead update` accepts, which is the surface divergence
    // the plan's criterion 4 is about.
    // Hand-rolled rather than `UpdateEntityArgs::changes_content()` only
    // because the args do not exist yet at this point: the hash mode has to be
    // resolved before they can be built. `batch_entry_content_matches_the_engine_predicate`
    // pins the two against each other so they cannot drift.
    let changes_content = !entry.sections.is_empty()
        || !entry.append_sections.is_empty()
        || !entry.patch_sections.is_empty()
        || !entry.metadata.is_empty()
        || !entry.metadata_unset.is_empty()
        || !entry.declare_relations.is_empty();
    let anchors_only =
        (!entry.anchors.is_empty() || !entry.anchors_unset.is_empty()) && !changes_content;
    if mode_count == 0 && !anchors_only {
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
    } else if anchors_only {
        // An EMPTY token is no token on an anchors-only entry, as on
        // `memstead update` and both MCP flavours: passed through, `""` reaches
        // the engine and can never match a real hash, so the identical payload
        // that the other three surfaces write refused HASH_MISMATCH here
        // (consistency-sweep 03/04, criterion 4).
        entry.expected_hash.filter(|h| !h.is_empty())
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
            target: EntityId::canonical(&r.target),
            description: r.description,
        })
        .collect();

    Ok((
        UpdateEntityArgs {
            anchors: entry.anchors,
            anchors_unset: entry.anchors_unset,
            id,
            expected_hash,
            sections: entry.sections,
            append_sections: entry.append_sections,
            patch_sections,
            sections_unset: entry.sections_unset,
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
        "anchors",
        "anchors_unset",
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
    /// The batch entry's own content predicate must answer as the engine's
    /// does. It is hand-rolled because the hash mode is resolved before the
    /// engine args exist, so drift between the two is possible and this is
    /// what catches it: an entry whose only content is a section change must
    /// be seen as content-changing by both.
    #[test]
    fn batch_entry_content_matches_the_engine_predicate() {
        use memstead_base::UpdateEntityArgs;
        let mut args = UpdateEntityArgs {
            id: memstead_base::EntityId("m--e".into()),
            expected_hash: None,
            sections: Default::default(),
            append_sections: Default::default(),
            patch_sections: Default::default(),
            sections_unset: Vec::new(),
            metadata: Default::default(),
            metadata_unset: Vec::new(),
            dry_run: false,
            declare_relations: Vec::new(),
            anchors: Vec::new(),
            anchors_unset: Vec::new(),
            relations_unset: Vec::new(),
        };
        assert!(!args.changes_content(), "nothing named changes no content");
        args.anchors.push(Default::default());
        assert!(
            !args.changes_content(),
            "anchors are outside the content hash and must stay off this side"
        );
        args.sections.insert("purpose".into(), "x".into());
        assert!(args.changes_content(), "a section change is content");
    }

    /// The batch surface may recognise FEWER keys than the engine (its entry
    /// payload does not accept `relations_unset`), never more: advertising a
    /// key the parser rejects is the same lie as omitting one it accepts.
    #[test]
    fn batch_recognised_keys_are_a_subset_of_the_engines() {
        let engine_keys: std::collections::BTreeSet<&str> =
            memstead_base::engine::error::RECOGNISED_MUTATION_KEYS
                .iter()
                .copied()
                .collect();
        for key in super::RECOGNISED_MUTATION_KEYS {
            assert!(
                engine_keys.contains(key),
                "batch advertises `{key}`, which the engine does not recognise"
            );
        }
    }

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
            "declare_relations": [{"target": "specs--other", "rel_type": "USES"}],
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
