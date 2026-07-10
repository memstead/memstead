pub mod changes;
pub mod context;
pub mod create;

/// `--help` text describing the title→slug pipeline. Shared by
/// `memstead create` and `memstead rename` so an agent reading either
/// command's help can predict what slug a given title will produce
/// (and therefore why the strict gate refuses titles outside the
/// pipeline's accepted character classes).
pub const SLUG_DERIVATION_HELP: &str = "\
Slug derivation:
  The entity slug derives from the title in five steps:
    1. NFC-normalize (combining sequences fold to precomposed form);
    2. Unicode case-fold to lowercase;
    3. rewrite each whitespace character to '-';
    4. drop every character that is not Unicode alphanumeric and not '-';
    5. collapse hyphen runs, trim leading/trailing hyphens.

  The mutation entry refuses titles where step 4 would drop any
  character, or where the pipeline output is empty (whitespace- or
  hyphen-only input). Errors carry a `proposed_slug` recovery hint
  so a sanitised retry is mechanical.

  The title body is stored as-sent (byte-form preserved); slug bytes
  derive from the NFC-normalised form. An NFD-spelled title therefore
  produces an NFC-spelled slug — the two byte forms are semantically
  equivalent and compare equal under NFC normalization.

  Pre-gate entities (created before this stricter rule landed) remain
  readable. The gate runs at mutation entry only — it does not
  retroactively reject entities loaded from disk.";

/// `--help` epilog for `memstead create` and `memstead update`. The CLI
/// stores `--section` / `--append` / `--patch` flag values as bytes
/// verbatim — backslash escapes (`\n`, `\t`, …) are NOT interpreted.
/// Agents reading the help learn the multi-line-authoring escape hatch
/// (`--from <JSON file>`) before they hit the friction.
pub const SECTION_BYTES_VERBATIM_HELP: &str = "\
Section / append / patch flag values:
  `--section KEY=VALUE`, `--append KEY=VALUE`, and `--patch KEY=OLD=>NEW`
  store the right-hand side as bytes verbatim. The CLI does NOT
  interpret backslash escapes — `--section purpose=\"line1\\nline2\"`
  writes the literal two-character sequence `\\n` into the section
  body, not a newline.

  For multi-line section content, use `--from <FILE>` where FILE is a
  JSON payload matching the MCP `memstead_create` / `memstead_update` shape.
  The JSON parser de-escapes `\\n`, `\\t`, etc. before the engine
  sees the value, so a JSON-quoted `\"line1\\nline2\"` round-trips as
  two lines on disk.";

/// Combined `--help` epilog for `memstead create`: the section-bytes-
/// verbatim note followed by the title→slug pipeline description.
/// `clap`'s `after_long_help` takes a single string, so the two
/// epilogs are pre-concatenated here.
pub const CREATE_AFTER_LONG_HELP: &str = concat!(
    "Section / append / patch flag values:\n",
    "  `--section KEY=VALUE`, `--append KEY=VALUE`, and `--patch KEY=OLD=>NEW`\n",
    "  store the right-hand side as bytes verbatim. The CLI does NOT\n",
    "  interpret backslash escapes — `--section purpose=\"line1\\nline2\"`\n",
    "  writes the literal two-character sequence `\\n` into the section\n",
    "  body, not a newline.\n",
    "\n",
    "  For multi-line section content, use `--from <FILE>` where FILE is a\n",
    "  JSON payload matching the MCP `memstead_create` / `memstead_update` shape.\n",
    "  The JSON parser de-escapes `\\n`, `\\t`, etc. before the engine\n",
    "  sees the value, so a JSON-quoted `\"line1\\nline2\"` round-trips as\n",
    "  two lines on disk.\n",
    "\n",
    "Slug derivation:\n",
    "  The entity slug derives from the title in five steps:\n",
    "    1. NFC-normalize (combining sequences fold to precomposed form);\n",
    "    2. Unicode case-fold to lowercase;\n",
    "    3. rewrite each whitespace character to '-';\n",
    "    4. drop every character that is not Unicode alphanumeric and not '-';\n",
    "    5. collapse hyphen runs, trim leading/trailing hyphens.\n",
    "\n",
    "  The mutation entry refuses titles where step 4 would drop any\n",
    "  character, or where the pipeline output is empty (whitespace- or\n",
    "  hyphen-only input). Errors carry a `proposed_slug` recovery hint\n",
    "  so a sanitised retry is mechanical.\n",
    "\n",
    "  The title body is stored as-sent (byte-form preserved); slug bytes\n",
    "  derive from the NFC-normalised form. An NFD-spelled title therefore\n",
    "  produces an NFC-spelled slug — the two byte forms are semantically\n",
    "  equivalent and compare equal under NFC normalization.\n",
    "\n",
    "  Pre-gate entities (created before this stricter rule landed) remain\n",
    "  readable. The gate runs at mutation entry only — it does not\n",
    "  retroactively reject entities loaded from disk.",
);

/// `--help` epilog for `memstead search` and `memstead list`. Names the five
/// frozen named-flag shortcuts and points at `--filter KEY=VALUE` as
/// the path to any other schema-declared `filterable: equality` field.
pub const FILTER_HELP: &str = "\
Filter surface:
  Named-flag shortcuts (frozen — no new ones are added; use --filter
  for any other schema-declared filterable field):
    --type <T>          Filter by entity_type (engine first-class axis).
    --level <L>         Filter by level (e.g. M0, M1).
    --status <S>        Filter by status (e.g. active, closed).
    --edge-type <E>     Filter by edge type (engine first-class axis).

  Generic equality filter:
    --filter KEY=VALUE  Filter by any schema-declared `filterable: equality`
                        field. Repeatable. Examples:
                          --filter tags=auth
                          --filter scope=subsystem
                          --filter confidence=high
                          --filter tags=auth --filter level=M0
  Unknown keys are silently dropped by the engine and surface as a
  warning. Named-flag shortcuts and `--filter` populate the same
  underlying filter map; if both set the same key, `--filter` wins
  (declared last in the iteration order).";

/// Parse a `KEY=VALUE` argument supplied via `--filter`. Returns a
/// typed `CliError` on malformed input so the failure rides the
/// `INVALID_INPUT` envelope rather than crashing the process.
pub fn parse_filter_arg(raw: &str) -> Result<(String, String), crate::CliError> {
    let (key, value) = raw.split_once('=').ok_or_else(|| {
        crate::CliError::new(
            crate::output::ExitKind::Validation,
            "INVALID_INPUT",
            format!("--filter expects KEY=VALUE, got `{raw}`"),
        )
    })?;
    if key.is_empty() {
        return Err(crate::CliError::new(
            crate::output::ExitKind::Validation,
            "INVALID_INPUT",
            format!("--filter key must be non-empty: `{raw}`"),
        ));
    }
    Ok((key.to_string(), value.to_string()))
}

/// Render the per-entity-type guidance block surfaced on
/// `memstead_create`'s text mirror. Emits one "Type-level guidance"
/// section per `entity_type` key in the map. Returns an empty string
/// when the map is empty so callers can concatenate unconditionally.
/// The structured channel ships the same data top-level on
/// `type_guidance`.
pub fn render_type_guidance_block(
    type_guidance: &std::collections::BTreeMap<String, Vec<String>>,
) -> String {
    if type_guidance.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (entity_type, rules) in type_guidance {
        if rules.is_empty() {
            continue;
        }
        out.push_str(&format!("\n>\n> Type-level guidance for `{entity_type}`:",));
        for rule in rules {
            out.push_str(&format!("\n> - {rule}"));
        }
    }
    out
}

/// Merge a `mem_changed` notice array into a `--json` CLI response
/// body. No-op when no reload happened during the operation or the
/// body is not a JSON object. Mirrors the MCP server's
/// `attach_mem_changed` so the two surfaces emit the same key.
/// (Always compiled; on lean the engine never stashes a notice, so
/// `notices` is empty and this no-ops.)
pub fn merge_mem_changed_json(
    body: &mut serde_json::Value,
    notices: &[memstead_base::ops::MemChangedNotice],
) {
    if notices.is_empty() {
        return;
    }
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "mem_changed".to_string(),
            serde_json::to_value(notices).unwrap_or(serde_json::Value::Null),
        );
    }
}

/// Render a human-readable `mem_changed` block for markdown CLI
/// output. Empty when no reload happened. Names `memstead changes-since`
/// (never `memstead diff`) for the follow-up, matching the cross-surface
/// recovery contract. (Always compiled; on lean `notices` is always
/// empty, so this returns the empty string.)
pub fn render_mem_changed_block(notices: &[memstead_base::ops::MemChangedNotice]) -> String {
    use memstead_base::ops::NoticeChanges;
    if notices.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n## Mem changed under you");
    for n in notices {
        out.push_str(&format!(
            "\n\n- `{}` advanced `{}` → `{}`",
            n.mem, n.from_head, n.to_head
        ));
        match &n.changes {
            NoticeChanges::Detailed { entries } => {
                for e in entries {
                    out.push_str(&format!("\n  - {} `{}`", e.action(), e.primary_id()));
                }
            }
            NoticeChanges::Ids { entries } => {
                out.push_str(&format!(
                    "\n  - {} changed ids — `memstead changes-since --since {}` for the full delta",
                    entries.len(),
                    n.from_head
                ));
            }
            NoticeChanges::Counts { self_inform, .. } => {
                out.push_str(&format!("\n  - mass change — {self_inform}"));
            }
        }
    }
    out
}

pub mod admin;
pub mod delete;
pub mod domain;
pub mod entity;
pub mod export;
pub mod health;
pub mod ingest;
pub mod init;
pub mod link;
pub mod list;
pub mod login;
pub mod logout;
pub mod overview;
pub mod pipeline;
pub mod projection;
pub mod publish;
pub mod quickstart;
pub mod relate;
pub mod relations;
pub mod reload;
pub mod rename;
pub mod schema;
pub mod search;
pub mod status;
pub mod type_cmd;
pub mod unpublish;
pub mod update;

// Multi-mem / mem-repo subcommands — compiled into the full
// `memstead` binary (default features); absent from the lean
// `--no-default-features` build, which has no git-branch backend.
#[cfg(feature = "mem-repo")]
pub mod batch_update;
#[cfg(feature = "mem-repo")]
pub mod branch_reset;
#[cfg(feature = "mem-repo")]
pub mod install;
#[cfg(feature = "mem-repo")]
pub mod mem;
#[cfg(feature = "mem-repo")]
pub mod mem_repo;
#[cfg(feature = "mem-repo")]
pub mod recover;
#[cfg(feature = "mem-repo")]
pub mod transport;
#[cfg(feature = "mem-repo")]
pub mod workspace;
