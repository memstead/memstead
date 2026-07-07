#![cfg(feature = "mem-repo")]
//! Locks the shape of the agent-facing MCP surface.
//!
//! The surface is the `EXPECTED_TOOLS` list below — read-only:
//! `memstead_entity`, `memstead_health`, `memstead_overview`,
//! `memstead_schema`, `memstead_search`; mutation: `memstead_create`,
//! `memstead_delete`, `memstead_relate`, `memstead_rename`,
//! `memstead_update`; admin: `memstead_changes_since`, `memstead_diff`,
//! `memstead_reload`; mem lifecycle: `memstead_mem_create`,
//! `memstead_mem_delete`, `memstead_mem_set_schema`,
//! `memstead_mem_set_version`; workspace policy:
//! `memstead_workspace_allow_create` / `_allow_delete` /
//! `_grant_cross_link` / `_revoke_create` / `_revoke_cross_link` /
//! `_revoke_delete`. The asserted count is `EXPECTED_TOOLS.len()` —
//! `tool_count_matches_expected_set` pins it against the live router.
//!
//! Several former tools are now folded in and must not re-appear:
//! `memstead_list` → `memstead_search` (omit `text` for structural filters);
//! `memstead_path` → `memstead_search related_to=<id> depth=N`;
//! `memstead_schema_list` + `memstead_schema_info` collapsed and re-emerged
//! as a single `memstead_schema(name=...)` reader; `memstead_update_community`,
//! `memstead_batch_update`, `memstead_export`, `memstead_stats`,
//! `memstead_relations`, `memstead_context`, `memstead_type_info` are gone.
//!
//! Mem lifecycle is not workspace configuration, but both are on the
//! MCP surface. The lifecycle family (`memstead_mem_create` /
//! `memstead_mem_delete` / `memstead_mem_set_schema` /
//! `memstead_mem_set_version`) creates/removes/reconfigures a whole
//! mem at runtime; workspace-policy mutation (allowlists and
//! cross-mem link policy in `.memstead/workspace.toml`) is the
//! `memstead_workspace_*` family. The macOS app edits the same policy
//! in-process via its `WorkspaceService`. Every MCP tool must carry the
//! `memstead_` prefix — an un-namespaced `workspace_*` tool (or any
//! other non-`memstead_` tool) must fail this test.
//!
//! Drives the generated `McpServer::tool_router()` directly rather than
//! spawning a server over stdio — the router's tool list is the contract.

use memstead_mcp::server::McpServer;

/// The complete, canonical tool surface. Any change here is a public-API
/// change and must be made deliberately.
const EXPECTED_TOOLS: &[&str] = &[
    // Read-only graph + introspection (5)
    "memstead_entity",
    "memstead_health",
    "memstead_overview",
    "memstead_schema",
    "memstead_search",
    // Mutation (5)
    "memstead_create",
    "memstead_delete",
    "memstead_relate",
    "memstead_rename",
    "memstead_update",
    // Admin (3)
    "memstead_changes_since",
    "memstead_diff",
    "memstead_reload",
    // Mem lifecycle (4)
    "memstead_mem_create",
    "memstead_mem_delete",
    "memstead_mem_set_schema",
    "memstead_mem_set_version",
    // Workspace-policy mutations (6).
    // Closes [MCP F7] by exposing the cross-mem-link grant +
    // revoke surface and the lifecycle allowlist editor — an
    // MCP-driven agent can now complete the full dynamic mem
    // lifecycle without dropping to CLI.
    "memstead_workspace_allow_create",
    "memstead_workspace_allow_delete",
    "memstead_workspace_grant_cross_link",
    "memstead_workspace_revoke_create",
    "memstead_workspace_revoke_cross_link",
    "memstead_workspace_revoke_delete",
];

fn current_tool_names() -> Vec<String> {
    McpServer::tool_router()
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect()
}

#[test]
fn tool_surface_matches_expected_set() {
    let mut names = current_tool_names();
    names.sort();

    let mut expected: Vec<String> = EXPECTED_TOOLS.iter().map(|s| s.to_string()).collect();
    expected.sort();

    assert_eq!(
        names, expected,
        "\nTool surface drifted.\nGot:      {names:?}\nExpected: {expected:?}\n"
    );
}

#[test]
fn every_tool_uses_memstead_prefix() {
    let tools = McpServer::tool_router().list_all();
    for tool in &tools {
        assert!(
            tool.name.starts_with("memstead_"),
            "Tool '{}' lacks the required memstead_ prefix — every MCP tool, workspace-policy tools included, must be namespaced under memstead_",
            tool.name
        );
    }
}

#[test]
fn tool_count_matches_expected_set() {
    let count = McpServer::tool_router().list_all().len();
    let expected = EXPECTED_TOOLS.len();
    assert_eq!(
        count, expected,
        "Tool count drift — expected {expected}, got {count}. Update `EXPECTED_TOOLS` if a new tool intentionally landed."
    );
    // AGENTS.md MCP policy: stay well under Anthropic's 30-50 tool
    // degradation threshold. The cap below is informational — a hard
    // failure here means the surface has grown past where it should.
    assert!(
        count <= 30,
        "Tool surface at {count} — review AGENTS.md MCP policy before adding more (Anthropic's degradation threshold is 30-50). Consolidate or remove a tool first."
    );
}

/// Layering pin. The MCP server (`memstead-mcp`) must not depend on the
/// CLI crate (`memstead-cli`). The layering rule: CLI, MCP, and
/// UniFFI are sibling surfaces over the engine — so MCP tools that need
/// shared logic (e.g. the `workspace_config_edit` writers) reach it
/// through `memstead-engine`, never back through the CLI. Inspecting the
/// Cargo.toml is the canonical source of truth.
#[test]
fn memstead_mcp_does_not_depend_on_memstead_cli() {
    let cargo_toml_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let body = std::fs::read_to_string(&cargo_toml_path).expect("Cargo.toml must be readable");
    assert!(
        !body.contains("memstead-cli") && !body.contains("memstead_cli"),
        "memstead-mcp must not depend on memstead-cli — the layering forbids it. \
         If an MCP tool needs CLI-side helpers, lift them into memstead-engine \
         instead. Cargo.toml contents:\n{body}",
    );
}

/// Explicit guard for removed tools — named so a re-introduction fails
/// with an obvious per-tool message, not just "drift" on the set diff.
#[test]
fn mcp_does_not_expose_batch_update_or_export() {
    let names = current_tool_names();
    for removed in ["memstead_batch_update", "memstead_export"] {
        assert!(
            !names.iter().any(|n| n == removed),
            "{removed} must not be re-exposed — agents use memstead_update in a loop for batches; \
             export is human-triggered via memstead-cli export or the macOS app."
        );
    }
}

/// `memstead_stats` / `memstead_relations` / `memstead_context` are folded into
/// `memstead_entity` and `memstead_overview`; `memstead_type_info` is folded into
/// `memstead_overview.schemas[]`. They must stay gone even if someone
/// re-adds one by copy-paste.
#[test]
fn mcp_does_not_expose_folded_stats_tools() {
    let names = current_tool_names();
    for removed in [
        "memstead_stats",
        "memstead_relations",
        "memstead_context",
        "memstead_type_info",
    ] {
        assert!(
            !names.iter().any(|n| n == removed),
            "{removed} was folded into a sibling tool — do not re-expose."
        );
    }
}

/// `memstead_list` is folded into `memstead_search` (omit `text` for pure
/// structural/metadata filtering). Also guards against re-introducing
/// the phantom `memstead_entities` tool that never existed but was once
/// referenced from skills.
#[test]
fn mcp_does_not_expose_list_or_phantom_entities() {
    let names = current_tool_names();
    for removed in ["memstead_list", "memstead_entities"] {
        assert!(
            !names.iter().any(|n| n == removed),
            "{removed} must not be re-exposed — use memstead_search (omit `text` for filter-only queries)."
        );
    }
}

/// `memstead_path` (niche algorithmic query) is removed — use
/// `memstead_search related_to=<id> depth=N` for neighborhood exploration.
///
/// `memstead_reload` is back:
/// the original removal rationale ("contradicts the never-edit-.md-files-
/// directly project policy") was stale — the real pressure is from
/// regelkonforme MCP-mediated mutations from a sibling engine instance
/// (forked Claude-Code subagents, macOS app + chat subprocess, parallel
/// terminals on the same workspace). The engine also closes the
/// silent-overwrite gap on the write path via the same drift-check
/// primitive that backs `memstead_reload`. Do not re-remove with the
/// original rationale — multi-engine coexistence is a real workload now.
#[test]
fn mcp_does_not_expose_path() {
    let names = current_tool_names();
    {
        let removed = "memstead_path";
        assert!(
            !names.iter().any(|n| n == removed),
            "{removed} must not be re-exposed."
        );
    }
}

/// The two legacy schema-introspection tools are gone. Schema discovery
/// is now a two-tool pair: `memstead_overview` lists schemas as
/// `{ref, description}` only, and `memstead_schema(name=...)` reads one
/// schema's full per-type body.
#[test]
fn mcp_does_not_expose_schema_list_or_schema_info() {
    let names = current_tool_names();
    for removed in ["memstead_schema_list", "memstead_schema_info"] {
        assert!(
            !names.iter().any(|n| n == removed),
            "{removed} must not be re-exposed — use memstead_overview to list and memstead_schema(name=...) to read."
        );
    }
}

/// `memstead_search` takes the structured `query` shape; graph expansion
/// is via `expand_via` / `expand_depth`. The MCP JSON-schema for
/// `memstead_search` is the agent-facing contract; a drift here silently
/// changes the tool's callable shape.
#[test]
fn memstead_search_schema_exposes_query_and_expand_fields() {
    let tools = McpServer::tool_router().list_all();
    let search = tools
        .iter()
        .find(|t| t.name == "memstead_search")
        .expect("memstead_search must exist");
    let schema = serde_json::to_string(&search.input_schema)
        .expect("memstead_search input_schema must serialize to JSON");

    for field in ["\"query\"", "\"expand_via\"", "\"expand_depth\""] {
        assert!(
            schema.contains(field),
            "memstead_search schema missing {field}: {schema}"
        );
    }
    // Re-introducing the legacy flat `text` param would silently revive
    // the substring-semantics regression that the structured `query`
    // shape eliminated.
    assert!(
        !schema.contains("\"text\""),
        "memstead_search schema must not expose `text`: {schema}"
    );
}

/// Plan 03, Part A: the `dry_run` param docs tell the truth about what a
/// dry_run on an INVALID entity does — it refuses with the same typed
/// envelope a real call returns, NOT a warnings-list preview. Pins the
/// corrected wording against regression to the pre-refactor overpromise
/// ("plus any warnings (e.g. missing required sections)"), which claimed a
/// preview the engine never delivers (validation refuses before the dry-run
/// branch). Behaviour itself is pinned by
/// `create_entity_dry_run_returns_same_refusal_envelope_as_real_call`.
#[test]
fn dry_run_docs_describe_refusal_not_a_warnings_preview() {
    let create = schema_for("memstead_create");
    assert!(
        create.contains("typed envelope") || create.contains("typed refusal"),
        "create dry_run doc must say an invalid entity refuses with a typed envelope: {create}"
    );
    assert!(
        !create.contains("e.g. missing required sections"),
        "create dry_run doc must drop the misleading 'warnings (e.g. missing required sections)' overpromise: {create}"
    );
    let update = schema_for("memstead_update");
    assert!(
        update.contains("typed envelope") || update.contains("typed refusal"),
        "update dry_run doc must say validation still refuses under dry_run: {update}"
    );
}

/// Plan 02, Part B: the overview surface documents that community
/// detection is workspace-global — `mem=` scopes which clusters are
/// *reported*, not detection, and a sparse / disconnected subgraph may
/// form no cluster at all. Pins the docs so the expectation-gap fix
/// (the report's "mem= looks like it scopes communities") does not
/// silently regress on either the param docs or the tool description.
#[test]
fn overview_documents_workspace_global_community_scope() {
    // `mem` / `rebuild` param docs live in the input schema.
    let schema = schema_for("memstead_overview");
    assert!(
        schema.contains("workspace-global"),
        "overview param docs must state detection is workspace-global: {schema}"
    );
    assert!(
        schema.contains("catch-all"),
        "overview param docs must warn that sparse/disconnected subgraphs collapse into a catch-all (may form no distinct cluster): {schema}"
    );
    // The tool description carries the same honesty.
    let tools = McpServer::tool_router().list_all();
    let desc = tools
        .iter()
        .find(|t| t.name == "memstead_overview")
        .and_then(|t| t.description.as_ref().map(|d| d.to_string()))
        .expect("memstead_overview must have a description");
    assert!(
        desc.contains("workspace-global"),
        "overview tool description must state detection is workspace-global: {desc}"
    );
}

/// Returns the JSON-schema of one tool's parameters, serialized as a string
/// for substring assertions. Schemas embed properties as `"<name>": { ... }`
/// objects, so `contains("\"<name>\"")` is a reliable presence check (no
/// false positives from value substrings — the MCP wire shape never uses
/// the field name as a value).
fn schema_for(tool_name: &str) -> String {
    let tools = McpServer::tool_router().list_all();
    let tool = tools
        .iter()
        .find(|t| t.name == tool_name)
        .unwrap_or_else(|| panic!("{tool_name} must exist"));
    serde_json::to_string(&tool.input_schema)
        .unwrap_or_else(|e| panic!("{tool_name} input_schema must serialize: {e}"))
}

/// `memstead_mem_create` exposes a
/// `recovery` parameter on the wire shape with three accepted
/// enum values (`reattach`, `force_overwrite`, `hard_cleanup_first`)
/// matching `RecoveryAction::as_wire_str()`. Pin the schema so a
/// rename / drop on either side trips the test, and the
/// snake_case tokens stay stable.
#[test]
fn memstead_mem_create_schema_exposes_recovery_enum() {
    let schema = schema_for("memstead_mem_create");
    assert!(
        schema.contains("\"recovery\""),
        "memstead_mem_create schema must expose `recovery` param. Schema: {schema}"
    );
    for variant in ["reattach", "force_overwrite", "hard_cleanup_first"] {
        assert!(
            schema.contains(&format!("\"{variant}\"")),
            "memstead_mem_create.recovery schema must expose variant `{variant}`. Schema: {schema}"
        );
    }
}

/// The orphaned plural `fields` parameter on `memstead_search` is removed
/// (the engine never honoured it; field restriction lives on
/// `Query.field`, per-query, single-value). Re-introducing it would
/// silently re-create dead-param drift.
#[test]
fn memstead_search_schema_has_no_fields_param() {
    let schema = schema_for("memstead_search");
    // The legacy plural — the field at the SearchParams level.
    assert!(
        !schema.contains("\"fields\""),
        "memstead_search schema must not expose plural `fields` — use `query.field` (singular). \
         Schema: {schema}"
    );
    // Sanity: `query.field` (singular, on Query) is still there.
    assert!(
        schema.contains("\"field\""),
        "memstead_search schema must still expose `Query.field`: {schema}"
    );
}

/// `memstead_delete` exposes no `dry_run` parameter. The contract is
/// `expected_hash` for safe deletes, not preview-before-delete.
#[test]
fn memstead_delete_schema_has_no_dry_run_param() {
    let schema = schema_for("memstead_delete");
    assert!(
        !schema.contains("\"dry_run\""),
        "memstead_delete schema must not expose `dry_run` — use `expected_hash` for safety. Schema: {schema}"
    );
}

/// `memstead_delete` requires `expected_hash` (mirrors `memstead_update` /
/// `memstead_rename`). A destructive op without an optimistic-lock param
/// is a footgun; re-removing the field would silently reintroduce it.
#[test]
fn memstead_delete_schema_requires_expected_hash() {
    let schema = schema_for("memstead_delete");
    assert!(
        schema.contains("\"expected_hash\""),
        "memstead_delete schema must expose `expected_hash`. Schema: {schema}"
    );
    // Serde derives `#[serde(default)]`-less required fields into JSON-schema's
    // `"required": ["id", "expected_hash"]` array; the substring check is the
    // schema's own required-list serialization. Keeps the assertion decoupled
    // from specific schemars internals.
    assert!(
        schema.contains("\"required\"") && schema.contains("\"expected_hash\""),
        "memstead_delete schema must list `expected_hash` as required. Schema: {schema}"
    );
}

/// #10: relationship type is case-insensitive on input and stored canonically
/// as UPPER_SNAKE_CASE. The JSON-Schema `pattern` accepts the broader
/// alphabet so lowercase/mixed-case inputs are admitted (and echoed back in
/// canonical form); `entity/id.rs::validate_rel_type` coerces to the
/// canonical shape at runtime.
#[test]
fn memstead_relate_schema_constrains_rel_type_pattern() {
    for tool in ["memstead_relate", "memstead_create"] {
        let schema = schema_for(tool);
        assert!(
            schema.contains(r#""pattern":"^[A-Za-z][A-Za-z_]*$""#),
            "{tool} schema must carry the case-insensitive alphabetic pattern on `type`. Schema: {schema}"
        );
    }
}

// --- Annotation hints + description-quality invariants -------------------
// --------------------------------------------------------------------------

/// Expected annotation-hint triple for each tool. `None` means the hint
/// isn't set (serialized as absent). This table is the canonical contract —
/// any drift in `server.rs` fails here with an obvious per-tool message.
///
/// Hint semantics (MCP spec):
/// - `read_only_hint = true`: tool does not mutate its environment
/// - `destructive_hint = true`: updates may be destructive (meaningful only
///   when `read_only_hint == false`)
/// - `idempotent_hint = true`: repeated calls with the same args have no
///   additional effect (meaningful only when `read_only_hint == false`)
/// - `open_world_hint = false`: tool interacts only with a closed domain
///   (this graph), not the open internet/world
struct HintTriple {
    read_only: Option<bool>,
    destructive: Option<bool>,
    idempotent: Option<bool>,
    open_world: Option<bool>,
}

fn expected_hints(tool_name: &str) -> HintTriple {
    match tool_name {
        // Read-only graph + introspection — every read tool sets all four
        // hints explicitly. `idempotent_hint = true` is meaningful even for
        // read tools: repeat calls with the same args return equivalent
        // output (modulo in-flight mutations from other callers), so a
        // client that caches or retries transparently is safe.
        "memstead_entity"
        | "memstead_search"
        | "memstead_overview"
        | "memstead_schema"
        | "memstead_health"
        | "memstead_changes_since"
        | "memstead_diff" => HintTriple {
            read_only: Some(true),
            destructive: Some(false),
            idempotent: Some(true),
            open_world: Some(false),
        },
        // Additive mutations — `memstead_create`/`memstead_update`/`memstead_rename`
        // modify existing content but are recoverable via compensating
        // ops (delete the new entity, update back, rename back). Only
        // `memstead_delete` carries the true destructive hint.
        // `idempotent = false` across the board because a partial-failure
        // retry is not safe (duplicate-title collisions, pre-existing
        // state drift, renamed-out-of-existence scenarios).
        "memstead_create" | "memstead_update" | "memstead_rename" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(false),
            open_world: Some(false),
        },
        // `memstead_delete` — the only genuinely destructive tool on the
        // surface. File + edges removed, not recoverable without a git
        // revert; agent must opt in via an explicit call.
        "memstead_delete" => HintTriple {
            read_only: Some(false),
            destructive: Some(true),
            idempotent: Some(false),
            open_world: Some(false),
        },
        // `memstead_relate` is genuinely idempotent — duplicate-add and
        // remove-nonexistent are typed-warning no-ops.
        "memstead_relate" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(true),
            open_world: Some(false),
        },
        // `memstead_reload` is a state-refresh op — repeating it converges
        // the in-memory snapshot toward the on-disk HEAD. Idempotent
        // by construction (a second call against an unchanged HEAD
        // yields a no-op report). Not destructive (no data loss; the
        // store is rebuilt from disk truth). Not read-only because the
        // engine's in-memory state changes — the persistent on-disk
        // graph does not.
        "memstead_reload" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(true),
            open_world: Some(false),
        },
        // `memstead_mem_create` is a write op but not flagged `destructive`
        // (no existing data is rewritten — a seed commit in a fresh
        // gitdir is an additive op from the workspace's perspective).
        // `idempotent = false` because a second call with the same name
        // hits `MEM_NAME_COLLISION`, not a no-op.
        "memstead_mem_create" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(false),
            open_world: Some(false),
        },
        // `memstead_mem_delete` — unregisters (and optionally rmdirs) a
        // mem. Destructive with `delete_files: true`; still
        // destructive when `false` because the router-side effect is
        // immediate and not automatically reversible without a skill
        // re-running the explicit registration.
        "memstead_mem_delete" => HintTriple {
            read_only: Some(false),
            destructive: Some(true),
            idempotent: Some(false),
            open_world: Some(false),
        },
        // `memstead_mem_set_version` — bumps a mem's `version` field
        // and persists through the backend. Mutation (read_only=false)
        // but not destructive — the prior value is overwritten in
        // place. `idempotent=false`: calling it twice with the same
        // version is technically a no-op on disk, but the response
        // still ships `{old, new}` and the engine writes the config
        // bytes either way.
        "memstead_mem_set_schema" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(false),
            open_world: Some(false),
        },
        "memstead_mem_set_version" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(false),
            open_world: Some(false),
        },
        // Workspace-policy mutations.
        // All idempotent (`Err` → `Ok(Warning)` flip) and
        // non-destructive (the rule lists / grant tables grow or
        // shrink, but underlying mem data is never touched). The
        // `revoke` half flips `destructive=true` because removing a
        // grant or rule changes downstream authorization (e.g.,
        // future `memstead_mem_create` may refuse).
        "memstead_workspace_grant_cross_link" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(true),
            open_world: Some(false),
        },
        "memstead_workspace_revoke_cross_link" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(true),
            open_world: Some(false),
        },
        "memstead_workspace_allow_create" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(true),
            open_world: Some(false),
        },
        "memstead_workspace_revoke_create" => HintTriple {
            read_only: Some(false),
            destructive: Some(true),
            idempotent: Some(true),
            open_world: Some(false),
        },
        "memstead_workspace_allow_delete" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(true),
            open_world: Some(false),
        },
        "memstead_workspace_revoke_delete" => HintTriple {
            read_only: Some(false),
            destructive: Some(true),
            idempotent: Some(true),
            open_world: Some(false),
        },
        _ => panic!("unexpected tool in hint table: {tool_name}"),
    }
}

#[test]
fn every_tool_has_expected_annotation_hints() {
    let tools = McpServer::tool_router().list_all();
    for tool in &tools {
        let expected = expected_hints(&tool.name);
        let ann = tool
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("{} must set annotation hints", tool.name));

        assert_eq!(
            ann.read_only_hint, expected.read_only,
            "{}: read_only_hint drifted",
            tool.name
        );
        assert_eq!(
            ann.destructive_hint, expected.destructive,
            "{}: destructive_hint drifted",
            tool.name
        );
        assert_eq!(
            ann.idempotent_hint, expected.idempotent,
            "{}: idempotent_hint drifted",
            tool.name
        );
        assert_eq!(
            ann.open_world_hint, expected.open_world,
            "{}: open_world_hint drifted",
            tool.name
        );
    }
}

/// Helper — returns (tool_name, description) pairs for every tool that
/// declares a description. Every Memstead tool MUST have one.
fn descriptions() -> Vec<(String, String)> {
    McpServer::tool_router()
        .list_all()
        .iter()
        .map(|t| {
            let desc = t
                .description
                .as_deref()
                .unwrap_or_else(|| panic!("{} must set a description", t.name))
                .to_string();
            (t.name.to_string(), desc)
        })
        .collect()
}

/// Description must lead with an active verb (or an active-verbal phrase
/// like "Per-mem"). Curated allowlist, not an exhaustive dictionary —
/// new entries go here deliberately as the surface evolves. Rejects the
/// two most common filler openers ("This tool…", "Allows you to…").
#[test]
fn descriptions_start_with_verb() {
    // Curated — extend deliberately. "Per-mem" is permanent for
    // `memstead_changes_since`; "Search" is permanent for `memstead_search`.
    const ALLOWED_LEADS: &[&str] = &[
        "Read",
        "Find",
        "Search",
        "Create",
        "Modify",
        "Remove",
        "Rename",
        "Connect",
        "Return",
        "Start",
        "Per-mem",
        "List",
        "Check",
        "Unregister",
        "Reload",
        "Update",
        // The six `memstead_workspace_*` tools lead with operation
        // verbs.
        "Grant",
        "Revoke",
        "Append",
    ];
    const BANNED_LEADS: &[&str] = &["This", "Allows", "A", "An", "The"];

    let mut violations = Vec::new();
    for (name, desc) in descriptions() {
        let first = desc.split_whitespace().next().unwrap_or("");
        let first_word = first.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-');

        if BANNED_LEADS.contains(&first_word) {
            violations.push(format!(
                "{name}: description starts with banned filler '{first_word}'"
            ));
            continue;
        }
        if !ALLOWED_LEADS.contains(&first_word) {
            violations.push(format!(
                "{name}: description starts with '{first_word}' — not in curated verb allowlist {ALLOWED_LEADS:?}"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "description-lead violations:\n  {}",
        violations.join("\n  ")
    );
}

/// No TODO/FIXME/XXX/tbd markers leaking into the agent-facing contract.
#[test]
fn descriptions_have_no_todo_markers() {
    const FORBIDDEN: &[&str] = &["TODO", "FIXME", "XXX", "tbd", "TBD"];

    let mut violations = Vec::new();
    for (name, desc) in descriptions() {
        for marker in FORBIDDEN {
            if desc.contains(marker) {
                violations.push(format!(
                    "{name}: description contains forbidden marker '{marker}'"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "TODO-marker violations:\n  {}",
        violations.join("\n  ")
    );
}

/// Description length must sit in a usable band: thick enough to be a
/// standalone brief (≥ 30 words), thin enough to fit in a tool-list render
/// without dominating it (≤ 220 words). This is a usability knob, not a
/// hard limit — raise further if a future rewrite legitimately needs
/// more; tighten if descriptions grow bloated.
#[test]
fn descriptions_length_bounds() {
    const MIN_WORDS: usize = 30;
    // The `OUTER_REPO_NOT_IGNORING_MEM_REPO` surface description on
    // `memstead_health` pushes its word count to 228. The
    // `memstead_schema` precondition line on memstead_create /
    // memstead_update / memstead_relate bumps the cap to 260. The
    // `conformance` / `integrity` include keys and the `findings` shape
    // on `memstead_health` (281 words after trimming) — a genuinely new
    // response surface — move the ceiling to 290; further growth should
    // be answered with a trim rather than a ceiling move.
    const MAX_WORDS: usize = 290;

    let mut violations = Vec::new();
    for (name, desc) in descriptions() {
        let words = desc.split_whitespace().count();
        if words < MIN_WORDS {
            violations.push(format!("{name}: {words} words < {MIN_WORDS} (too thin)"));
        }
        if words > MAX_WORDS {
            violations.push(format!("{name}: {words} words > {MAX_WORDS} (too long)"));
        }
    }
    assert!(
        violations.is_empty(),
        "description-length violations:\n  {}",
        violations.join("\n  ")
    );
}

/// Every description must fit the primary client's truncation window.
/// Claude Code cuts tool descriptions at 2,048 characters — an over-limit
/// description reaches the main consumer chopped mid-sentence. This is a
/// hard client-facing ceiling, not a style knob: teaching content that
/// doesn't fit moves to the server `instructions`, the docs, or the
/// `memstead_schema` lite/full detail path — it is never left to be
/// silently truncated. Measured in bytes (stricter than chars), against
/// the built router output. A deliberately over-limit description
/// requires a documented justification here AND a per-tool allowlist
/// entry — today that list is empty.
#[test]
fn descriptions_fit_primary_client_truncation() {
    const MAX_BYTES: usize = 2048;

    let mut violations = Vec::new();
    for (name, desc) in descriptions() {
        let bytes = desc.len();
        if bytes > MAX_BYTES {
            violations.push(format!(
                "{name}: {bytes} bytes > {MAX_BYTES} (truncated in Claude Code)"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "description-truncation violations:\n  {}",
        violations.join("\n  ")
    );
}

/// Backtick-quoted identifiers in a description must resolve to:
/// 1. a parameter on the tool's input schema, OR
/// 2. a documented response-shape field (per-tool allowlist below), OR
/// 3. a generic term (prose/value, per-tool-agnostic allowlist below).
///
/// This is the forcing function that would have caught today's
/// `search.fields` drift (the param was deleted but the description still
/// mentioned it). Extract only "simple-looking" tokens — anything with
/// braces, brackets, spaces, equals, or 40-char hex strings is skipped
/// (those are example JSON blobs or SHAs, not identifier references).
#[test]
fn descriptions_reference_only_existing_params() {
    let mut violations = Vec::new();
    for (name, desc) in descriptions() {
        let schema = schema_for(&name);
        for token in extract_backtick_tokens(&desc) {
            if is_allowed_reference(&name, &token, &schema) {
                continue;
            }
            violations.push(format!(
                "{name}: backtick reference `{token}` is neither an input param nor a documented response/generic term"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "backtick-reference violations (description drifted from implementation):\n  {}",
        violations.join("\n  ")
    );
}

/// Extract content between single-backtick pairs. Filters out anything
/// that clearly isn't an identifier reference (JSON blobs, SHAs, ranges,
/// long freeform strings). Keeps dotted paths (`query.field`), plain
/// identifiers (`mem`), subscripted paths (`mems[]`), and slashed
/// alternatives (`writable_mems`/`read_mems` — kept as one token
/// because that's how the description writes it; we split on `/` inside
/// `is_allowed_reference`).
fn extract_backtick_tokens(desc: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = desc.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'`' {
                j += 1;
            }
            if j >= bytes.len() {
                break;
            }
            let raw = &desc[start..j];
            i = j + 1;
            // Skip obvious non-identifier content: JSON objects/arrays,
            // anything containing whitespace, quote chars, or `=`/`:` (value
            // assignments like `rebuild: true` — the `rebuild` half is
            // already documented by the param schema, so the full token
            // `rebuild: true` doesn't need a separate assertion).
            let skip = raw.is_empty()
                || raw.contains('{')
                || raw.contains('[')
                || raw.contains(' ')
                || raw.contains('\n')
                || raw.contains('"')
                || raw.contains('=')
                || raw.contains(':');
            if skip {
                continue;
            }
            // Skip git SHA hex strings (40 hex chars) — these are literal
            // values in examples, not identifier references.
            if raw.len() == 40 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            out.push(raw.to_string());
        } else {
            i += 1;
        }
    }
    out
}

/// A reference is allowed iff every segment resolves. For a dotted path
/// `parent.child` the head (`parent`) must be a param — this mirrors the
/// shape of a structured input (`Query.field` under `SearchParams.query`).
/// For a slashed pair `a/b` both halves must resolve. Plain identifiers
/// just need to be in the schema OR one of the allowlists.
fn is_allowed_reference(tool_name: &str, token: &str, schema: &str) -> bool {
    // Trailing-slash directory references (e.g. `mem-repo/`) — strip
    // the slash and resolve the bare name. Without this the slashed
    // branch below would split into `["mem-repo", ""]` and reject the
    // empty half.
    if token.ends_with('/') && !token.is_empty() {
        let trimmed = &token[..token.len() - 1];
        if !trimmed.contains('/') {
            return is_allowed_reference(tool_name, trimmed, schema);
        }
    }
    // Slashed alternative — both sides must resolve independently.
    if token.contains('/') {
        return token
            .split('/')
            .all(|part| is_allowed_reference(tool_name, part, schema));
    }
    // Dotted path — check the head only (children are struct fields we
    // don't enumerate; tightening this is S4's job).
    let head = token.split('.').next().unwrap_or(token);
    let normalised = head.trim_end_matches("[]");

    if schema.contains(&format!("\"{normalised}\"")) {
        return true;
    }
    if response_shape_refs(tool_name).contains(&normalised) {
        return true;
    }
    if GENERIC_REFS.contains(&normalised) {
        return true;
    }
    // For dotted tokens where the head did NOT resolve as a param, also
    // allow if the whole dotted form is listed in response-shape refs.
    if token.contains('.') && response_shape_refs(tool_name).contains(&token) {
        return true;
    }
    false
}

/// Terms that may appear in prose or as literal values — not params, not
/// response fields. Keep this list short; prefer extending a tool-level
/// allowlist when a reference is actually structural.
const GENERIC_REFS: &[&str] = &[
    "true",
    "false",
    "null",
    "markdown",
    "JSON",
    "chunk",
    "mem",
    "sections",
    "structured_content",
];

/// Per-tool response-shape fields referenced in descriptions. A reference
/// that's neither a param nor here is treated as drift. The lists are
/// deliberately small — only add when the description legitimately needs
/// to name a wire-shape field.
fn response_shape_refs(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "memstead_entity" => &[
            "_hash",
            "_chunk",
            "_truncated",
            "_tokens_unfiltered_body",
            "_tokens",
            "_total_chunks",
            "_stub_kind",
            "relationships",
            // Hash-after-relate clarification — memstead_entity's docstring
            // names the relate response's `content_hash` field and points
            // at the `expected_hash` parameter on follow-up mutations so
            // agents know the relate response carries the new valid hash.
            "memstead_relate",
            "_hash",
            "expected_hash",
            // Structured envelope alongside the markdown text channel.
            "structured_content",
            "sections",
            "id",
            "mem",
            "type",
            "level",
            "stability",
            "created_date",
            "last_modified",
            // `metadata` is the single home for frontmatter keys; the
            // description names the map and dotted reads
            // (`metadata.level`, …).
            "metadata",
            // Data-origin trust label on the envelope + its two values.
            "origin",
            "first-party",
            "third-party",
        ],
        "memstead_search" => &[
            // Response envelope fields
            "facets",
            "matched_terms",
            "score_breakdown",
            "expansion",
            // Per-hit data-origin trust label + its two values.
            "origin",
            "first-party",
            "third-party",
            "heading_path",
            "by_subsection",
            "by_type",
            "by_mem",
            "by_level",
            "by_status",
            "by_confidence",
            "by_expansion",
            // Dotted forms from the input `query` struct — allowed as
            // full tokens so `query.any`, `query.not`, etc. resolve even
            // when schemars doesn't surface the sub-field at the top level.
            "query.any",
            "query.not",
            "query.phrase",
            "query.field",
            // Warning codes referenced literally in the description for
            // the search filter family.
            "STUB_FILTER_EXCLUDES_ALL",
            "UNKNOWN_FILTER_KEY",
            "FIELD_NOT_FILTERABLE",
            // #52 enum-value filter warning + #54 neighbourhood cap.
            "INVALID_ENUM_VALUE",
            "enum_values",
            "details.allowed",
            "NEIGHBOURHOOD_CAPPED",
            // Applied-with-type-narrowing carries its own code, distinct
            // from the truly-unknown-key code, so a consumer branches on
            // `code`.
            "FILTER_TYPE_SCOPED",
            "RANGE_FILTER_TYPE_SCOPED",
            "RANGE_FILTER_KEY_MALFORMED",
            "UNKNOWN_RANGE_FILTER_FIELD",
            "FIELD_NOT_RANGE_FILTERABLE",
            "SEARCH_MEM_INDEX_UNAVAILABLE",
            // Token-budget guard: an overflowing page is trimmed with a
            // `SEARCH_RESULTS_TRUNCATED` warning carrying `kept`/`budget`.
            "SEARCH_RESULTS_TRUNCATED",
            "kept",
            "budget",
            // Range-filter param + key-shape mnemonics named in the
            // description so agents know how to construct the keys.
            "range_filters",
            "min_<field>",
            "max_<field>",
            "<field>_before",
            "<field>_after",
            // Warning-envelope shape — every search warning ships
            // `code`, `details`, `message`; `details.mem` /
            // `details.reason` are named on the `SEARCH_MEM_INDEX_UNAVAILABLE`
            // recovery prose.
            "code",
            "details.mem",
            "details.reason",
            // Structured envelope top-level fields surfaced in the
            // description so agents know to branch on `structured_content`.
            "structured_content",
            "SearchResultEnvelope",
            "_total",
            "_returned",
            "_offset",
            "_total_tokens",
            "hits",
            "warnings",
            // Per-hit shape fields the structured envelope ships.
            "score",
            "snippet",
            "sections",
        ],
        "memstead_overview" => &[
            "mems",
            "schemas",
            "overview_mode",
            "_overview_mode",
            "budget",
            "total_entities",
            "hints",
            "community_bridges",
            "dangling_links",
            "estimated_tokens",
            // `include` allowed-keys — named literally in the description
            "community_members",
            "mem_distribution",
            // response-shape fields referenced in prose
            "key",
            // Warning envelope `code` field — errors and warnings ride on
            // `structured_content` with a stable `code`, and the description
            // names it.
            "code",
            // Sibling-tool reference to the new schema-body reader.
            "memstead_schema",
            "ref",
            "description",
            // Cross-tool references in the trailing usage line.
            "memstead_create",
            "memstead_update",
            "memstead_relate",
            // Workspace-policy surface — frontmatter slot + the
            // policy fields named in the description.
            "_policy",
            "require_notes",
            "cross_mem_links",
        ],
        "memstead_schema" => &[
            // Response-shape fields shipped by build_schema_payload.
            // Full and lite ship the heavy arrays under distinct keys;
            // the description names all four so consumers decode by key
            // presence.
            "ref",
            "types",
            "types_summary",
            "relationships_summary",
            "description",
            "when_to_use",
            "relationship_mode",
            "relationships",
            "used_by",
            "default_writing_guidance",
            "alias_target_rel_type",
            // Trust origin: the wire field + its two values.
            "origin",
            "first-party",
            "third-party",
            "system_context",
            "writing_guidance",
            "write_rules",
            "community.resolution",
            "community.seed",
            // Field shapes named in prose.
            "enum",
            "default_weight",
            "default",
            "required",
            // Sibling-tool references named in the workflow imperative.
            "memstead_create",
            "memstead_update",
            "memstead_relate",
            "memstead_overview",
            // Per-mem schema pin reference embedded in the imperative.
            "mem.schema_ref",
            // Recovery-payload error codes named literally.
            "UNKNOWN_SECTION",
            "UNKNOWN_METADATA_FIELD",
            "INVALID_ENUM_VALUE",
            "REQUIRED_FIELD_UNSET",
            "INVALID_REL_TYPE",
            "ENTITY_NOT_FOUND",
            // Validator-refusal code named when describing the
            // alias-synthesis opt-out posture in the response prose.
            "WIKILINK_WITHOUT_RELATION",
            // Item E codes — `mem`-shortcut input validation.
            "INVALID_INPUT",
            "UNKNOWN_MEM",
            "details.id",
            "details.suggestions",
            "details",
            "details.known_mems",
        ],
        "memstead_create" => &[
            // Schema-discovery pointer named in the pre-fetch imperative.
            "memstead_schema",
            "warnings",
            "commit_sha",
            "id",
            "file_path",
            "_hash",
            "incoming",
            "incoming_count",
            // Warning codes referenced literally in the description.
            "MISSING_REQUIRED_SECTION",
            "UNDECLARED_RELATIONSHIP_OPEN",
            "NOTE_MISSING",
            "INLINE_WIKI_LINK_AUTO_STUBBED",
            // Required-metadata-field warning surfaced on create when the
            // schema does not auto-fill an unsupplied required field.
            "MISSING_REQUIRED_FIELD",
            // required_outgoing warning.
            "MISSING_REQUIRED_OUTGOING",
            "details.entity_id",
            "details.entity_type",
            "details.missing",
            "required_outgoing",
            "memstead_relate",
            // Typed error codes + envelope fields.
            "UNKNOWN_SECTION",
            "UNKNOWN_METADATA_FIELD",
            "INVALID_ENUM_VALUE",
            "REQUIRED_FIELD_UNSET",
            "INVALID_REL_TYPE",
            "details.declared",
            "details.allowed",
            "details.field_description",
            "details.enum_values",
            "details.type_write_rules",
            "details.stubs",
            "suggestion",
            // Title-slug refusal (create slug-refusal docs): INVALID_TITLE
            // names the refusal, proposed_slug its recovery field.
            "INVALID_TITLE",
            "proposed_slug",
            // Schema-payload field references. `write_rules` ships per
            // MISSING_REQUIRED_SECTION warning (section-axis); type-axis
            // guidance moved to the response's top-level `type_guidance`
            // map (F9). `type_write_rules` is still cited for the error
            // path (INVALID_ENUM_VALUE / REQUIRED_FIELD_UNSET). `decision`
            // is the example type the description names.
            "write_rules",
            "type_write_rules",
            "type_guidance",
            "decision",
            // Agent-authored provenance field — optional on every mutation
            // and shared across the surface.
            "note",
        ],
        "memstead_update" => &[
            // Schema-discovery pointer named in the pre-fetch imperative.
            "memstead_schema",
            // Recovery-payload home named in the fix-from-details pointer.
            "details",
            "prospective_hash",
            "_hash",
            "commit_sha",
            // Error-envelope code + details field referenced literally.
            "HASH_MISMATCH",
            "details.current",
            // Typed error codes + envelope fields.
            "UNKNOWN_SECTION",
            "UNKNOWN_METADATA_FIELD",
            "INVALID_ENUM_VALUE",
            "REQUIRED_FIELD_UNSET",
            "details.declared",
            "details.allowed",
            "details.field_description",
            "details.enum_values",
            "details.type_write_rules",
            "details.stubs",
            "suggestion",
            // Bug 4 (engine-bugs-from-planning-session.md): inline-wiki-link
            // auto-stub warning surfaces alongside the existing typed warnings.
            "INLINE_WIKI_LINK_AUTO_STUBBED",
            // required_outgoing warning.
            "MISSING_REQUIRED_OUTGOING",
            "required_outgoing",
            "memstead_relate",
            // Bytes-identical no-op short-circuit — empty commit_sha,
            // unchanged content_hash, UPDATE_NOOP warning. Anchors the
            // `expected_hash` caching contract probe campaigns expose.
            "UPDATE_NOOP",
            // Orphan-stub GC response field: removing a body wiki-link
            // that was a stub target's last referrer GC's the stub and
            // lists it here (shared shape with relate / delete).
            "orphan_stubs_removed",
            // Read-only field list: error code + the engine-stamped
            // metadata fields named alongside mem/id/type.
            "READ_ONLY_FIELD",
            "created_date",
            "last_modified",
            // Shared note/require_notes surface.
            "NOTE_MISSING",
            "note",
        ],
        "memstead_delete" => &[
            "relations_removed",
            "commit_sha",
            "warnings",
            // Error-envelope code + details field referenced literally.
            "HASH_MISMATCH",
            "details.current",
            // Refuse-on-write-mem-referrers contract.
            "HAS_INCOMING_REFS",
            "details.referrers",
            "memstead_relate",
            "memstead_update",
            // Residual-stub demotion path (only-ReadOnly referrers).
            "RESIDUAL_STUB_FOR_READONLY_REFERRERS",
            // memstead_entity frontmatter field referenced to describe the stub-
            // delete contract (empty `_hash` on stubs).
            "_hash",
            // Stub-GC response field added alongside the stub-delete contract.
            "orphan_stubs_removed",
            // Shared note/require_notes surface.
            "note",
        ],
        "memstead_rename" => &[
            "old_id",
            "new_id",
            "commit_sha",
            "warnings",
            // Warning code referenced literally in the description.
            "TITLE_NORMALIZED_TO_SLUG_NOOP",
            // Error-envelope code + details field referenced literally.
            "HASH_MISMATCH",
            "details.current",
            // Post-rename response now carries `content_hash` mirroring
            // `memstead_relate`'s contract so agents can chain the next
            // hash-protected op without a fresh memstead_entity read.
            "_hash",
            "expected_hash",
            "memstead_relate",
            "memstead_health",
            "memstead_changes_since",
            // Atomic referrer-rewrite contract (
            // the delete/rename reference-coherence contract). Rename now
            // walks Write-Mem referrers in one per-mem commit;
            // cross-mem peers are policy-gated; sibling-writer drift on a
            // peer surfaces a partial-failure envelope; the in-memory
            // residual-stub demotion path applies when the only surviving
            // referrers live in ReadOnly mounts.
            "relationships",
            "cross_mem_links",
            "RENAME_BLOCKED_BY_CROSS_MEM_POLICY",
            "details.from_mem",
            "details.blocked_referrers",
            "RENAME_PARTIAL_FAILURE",
            "details.committed_mems",
            "details.failed_mem",
            "details.failure_cause",
            "logical_operation_id",
            "RESIDUAL_STUB_FOR_READONLY_REFERRERS",
            // Shared note/require_notes surface.
            "note",
        ],
        "memstead_relate" => &[
            // Schema-discovery pointer named in the pre-fetch imperative.
            "memstead_schema",
            "warnings",
            "commit_sha",
            // Warning codes referenced literally in the description.
            "DUPLICATE_RELATIONSHIP",
            "NO_SUCH_RELATIONSHIP",
            // Structured error code for acyclic-typed cycle rejection + its
            // details payload fields.
            "RELATIONSHIP_CYCLE",
            "details.rel_type",
            "details.from",
            "details.to",
            "details.existing_path",
            "details.path_truncated",
            // INVALID_REL_TYPE recovery payload: allowed[] + nearest-match
            // suggestion ship inside the error envelope.
            "INVALID_REL_TYPE",
            "details.allowed",
            "suggestion",
            "memstead_overview",
            // Edge shape on RelationshipDef: INVALID_REL_SHAPE ships
            // recovery payloads. `memstead_health` is named as the
            // migration surface that exposes pre-constraint shape
            // violations so an agent can run `remove=true` cleanup.
            "source_types",
            "target_types",
            "INVALID_REL_SHAPE",
            "details.rel_type",
            "details.from_type",
            "details.to_type",
            "details.allowed_source_types",
            "details.allowed_target_types",
            "memstead_health",
            // Item 04 sub-case 1: relate-target id-grammar gate.
            // Malformed targets return INVALID_ENTITY_ID with
            // `details.id` + `details.reason`; the gate prevents an
            // auto-stub from being created at the bad id.
            "INVALID_ENTITY_ID",
            "details.id",
            "details.reason",
            // Relate-remove refused because source body still wiki-links target.
            "RELATION_HAS_BODY_LINKS",
            "details.body_links",
            // memstead_entity response field referenced as the post-relate invariant.
            "_hash",
            // Post-relate response now carries `content_hash`; the description
            // points at the downstream mutation tools that consume it via
            // `expected_hash`.
            "_hash",
            "expected_hash",
            "memstead_update",
            "memstead_rename",
            "memstead_delete",
            // Stub-GC response field — stubs whose last incoming edge was
            // dropped by this relate(remove) are GC'd in the same op.
            "orphan_stubs_removed",
            // Cross-mem relate is policy-gated.
            "cross_mem_links",
            "default_cross_links",
            "CROSS_MEM_LINK_NOT_ALLOWED",
            "details.from_mem",
            "details.to_mem",
            "CROSS_MEM_TARGET_NOT_FOUND",
            "details.target_id",
            "details.target_mem",
            // Cross-mem relate to an uncreated target mem — auto-stub
            // still lands; warning surfaces so typos vs. forward
            // references are distinguishable.
            "CROSS_MEM_TARGET_MEM_UNCREATED",
            // Cross-mem edge to a different schema gated on the
            // source schema's `cross_mem_relationships:` section.
            "CROSS_MEM_EDGE_NOT_DECLARED",
            "source_schema",
            "target_schema",
            "rel_type",
            "from_id",
            "to_id",
            "details.source_schema",
            "details.target_schema",
            "details.rel_type",
            "details.from_id",
            "details.to_id",
            "cross_mem_relationships",
            // Shared note/require_notes surface.
            "note",
        ],
        "memstead_health" => &[
            "writable_mems",
            "default_writable_mem",
            "read_mems",
            "orphans",
            "stubs",
            "most_connected",
            "missing_fields",
            "stale",
            "warnings",
            "community_count",
            "mem_schemas",
            "dangling_links",
            "from",
            "target_id",
            "target_path",
            "section",
            "total",
            "incoming",
            "outgoing",
            "typed_total",
            // Compact wildcard form the description uses for the three
            // typed_* counters.
            "typed_*",
            "typed_incoming",
            "typed_outgoing",
            "orphans_by_schema",
            "communities_by_schema",
            "tags",
            "tag_distribution",
            "tag_distribution_folded",
            "untagged_entities",
            // Warning codes referenced literally in the description.
            "UNKNOWN_INCLUDE_KEY",
            "LIMIT_CLAMPED",
            "details",
            // Integrity-linter surface: the `conformance` / `integrity` include
            // keys, the `findings` response array, its field names,
            // and the two consistency-axis codes.
            "conformance",
            "integrity",
            "findings",
            "axis",
            "code",
            "detail",
            "DANGLING_LINK",
            "ORPHAN_STUB",
            "SCHEMA_NOT_FOUND",
            // Load-time drift warning emitted by `push_entities_into_store`
            // at init/reload/attach.
            "SUSPICIOUS_NESTED_PREFIX",
            "details.from",
            "details.resolved_id",
            "details.candidate_target",
            "details.section",
            // Load-time parse warning — the parser emits when a markdown
            // file declared the same `## Heading` more than once for a
            // schema-declared section key.
            "DUPLICATE_SECTION_HEADING",
            "memstead_update",
            // Workspace-policy surface emitted under `include_config: true`
            // (mem-lifecycle-tools Sessions 1 + 5). Per-mem detail
            // array + origin enum land on the response shape the
            // description advertises. Lifecycle policy itself moved to
            // `memstead_overview` (mem-lifecycle-policy plan) — the two
            // related identifiers stay on the description so agents
            // following the cross-reference still parse cleanly.
            "mems",
            "origin",
            "explicit",
            "runtime_created",
            "memstead_overview",
            "mem_management.create",
            "mem_management.delete",
            // `[mutations]`,
            // `[plugin.*]`, and per-mem `vcs: { gitdir, worktree }` all
            // surface under `include_config: true` so the Stop hook can
            // resolve gitdirs and plugins can read their opaque config
            // sub-tables in one round-trip.
            "mutations",
            "require_notes",
            "plugin",
            "vcs",
            "gitdir",
            "worktree",
            "head",
            // Per-mem
            // `write_guidance` (opaque string map) and `extra` (unknown
            // top-level config keys) now ride on the `mems` detail
            // entries under `include_config: true`. F6 renamed the
            // wire-facing key from camelCase `writeGuidance` to
            // snake_case `write_guidance` for parity with the rest of
            // the surface; the on-disk JSON key (`.memstead/config.json`)
            // stays `writeGuidance`.
            "write_guidance",
            "extra",
            // Outer-repo gitignore guard. The
            // description names the warning code, the directory, the
            // outer-repo `.gitignore`, and the structured fields on
            // the warning envelope.
            "OUTER_REPO_NOT_IGNORING_MEM_REPO",
            "mem-repo",
            ".gitignore",
            "details.outer_repo_root",
            "details.workspace_root",
            // Multi-engine coherence (engine-multi-engine-coherence.md):
            // MEM_RELOADED auto-reload warning fires on any read
            // response when a sibling writer advanced the on-disk HEAD
            // past the engine's cached snapshot.
            "MEM_RELOADED",
            // The missing_required_outgoing
            // include surfaces a per-entity report list with the same
            // payload shape as the per-write warning, plus a mem
            // qualifier (entities are scanned cross-mem by default).
            "missing_required_outgoing",
            "required_outgoing",
            "entity_type",
            "id",
            "mem",
            "missing",
            "relationships",
            "cardinality",
            "title",
        ],
        "memstead_diff" => &[
            // Response-shape fields the description names.
            "ref_a",
            "ref_b",
            "resolved_a_sha",
            "resolved_b_sha",
            "config",
            "entries",
            "id",
            "title",
            "entity_type",
            "status",
            "content_before",
            "content_after",
            "ripple",
            // Ripple-entry shape — the docstring describes the populated
            // ripple shape.
            "from_id",
            "side",
            // EntityDiff `status` discriminator values surfaced in prose.
            "added",
            "modified",
            "deleted",
            "renamed",
            "invalid_entity",
            // Refusal codes named literally.
            "UNKNOWN_MEM",
            "UNKNOWN_REF",
            "INVALID_INPUT",
            "details.name",
            "details.ref",
            // Ref-handling conventions named in the docstring.
            // Sibling tool reference (alignment claim).
            "memstead_changes_since",
            // Bare-HEAD substitution and the canonical empty-tree
            // sentinel are documented in prose.
            "HEAD",
        ],
        "memstead_changes_since" => &[
            "commit_sha",
            "renamed",
            "from_id",
            "to_id",
            "head",
            "action",
            "added",
            "updated",
            "removed",
            "title",
            "entity_type",
            "warnings",
            // Out-of-range `rename_similarity` refusal envelope
            // (promoted from the prior clamp+warn shape).
            "INVALID_INPUT",
            "details.allowed_range",
            "details.requested",
            // Unknown / malformed `since` SHA returns a typed envelope.
            "INVALID_CURSOR",
            "details.mem",
            "details.since",
            // `include_notes: true` ride-along — `memstead_ref` is the SHA
            // of the workspace `__MEMSTEAD` ref (unified schemas + per-mem
            // configs).
            "memstead_ref",
            "__MEMSTEAD",
        ],
        "memstead_reload" => &[
            // Response-shape fields surfaced by the per-mem `ReloadReport`.
            "reports",
            "head_before",
            "head_after",
            "entities_loaded",
            "changed_entity_ids",
            // Auto-reload-on-read warning the description points at.
            "MEM_RELOADED",
            // Cross-tool reference for diff-list lookup.
            "memstead_changes_since",
            // Membership-fixed-at-boot clause cites the lifecycle tools that
            // *do* mutate the in-memory router atomically (mem-lifecycle-audit
            // Item 02), so an agent reading the warning knows where to go.
            "memstead_mem_create",
            "memstead_mem_delete",
            // Workspace-config-reload pairing (Item 03 of
            // workspace-config-via-cli.md): the workspace-wide form re-reads
            // `.memstead/workspace.toml`. The slashed-token allowlist rule
            // resolves both halves independently, so both segments are
            // listed here. The CLI surface (`memstead workspace allow-create`
            // etc.) contains a space, so it's filtered out at extraction
            // time and doesn't need an allowlist entry.
            ".memstead",
            "workspace.toml",
        ],
        "memstead_mem_create" => &[
            // Response-shape fields.
            "seed_commit_sha",
            "commit_sha",
            "schema_ref",
            // Schema-payload fields — the full schema catalogue ships
            // under `schema`, gated behind `include_schema: true`. The
            // catalogue references remain valid because the description
            // still names the shape when the caller opts in.
            "schema",
            "write_rules",
            "writing_guidance",
            "system_context",
            "when_to_use",
            // Error codes named literally in the description.
            "MEM_PATH_NOT_ALLOWED",
            "MEM_SCHEMA_NOT_ALLOWED",
            "MEM_NAME_COLLISION",
            "CONFIG_ERROR",
            // The description names
            // the storage-residue refusal envelope, the
            // reattach-after-unregister warning, the `__MEMSTEAD`
            // registry ref the probe inspects, and the
            // `unregistered_at` tombstone field on the residual
            // config.
            "MEM_STORAGE_RESIDUE_DETECTED",
            "MEM_REATTACHED_AFTER_UNREGISTER",
            "__MEMSTEAD",
            "unregistered_at",
            // Error-envelope `details` field references — both
            // envelopes (path + schema) carry these.
            "details.source",
            "details.missing_targets",
            "details.candidate",
            "details.patterns",
            "details.reason",
            "details.matched_pattern",
            "details.requested_schema",
            "details.allowed_schemas",
            // Cross-tool references the description points agents at.
            "memstead_health",
            "memstead_changes_since",
            "memstead_overview",
            // Config-discovery tokens embedded in the description.
            "outside_workspace",
            "no_allowlist_configured",
            "no_match",
            // Composed-candidate vocabulary (lifecycle-policy plan).
            "pattern",
            // Workspace-config tokens referenced verbatim.
            "mem_management.create",
            "schemas",
            // Cross-link policy tokens (workspace-cross-link-policy plan).
            "cross_mem_links",
            "default_cross_links",
            // `.memstead/workspace.toml` is named literally in the description;
            // the slashed-token check resolves each half against this list.
            ".memstead",
            "workspace.toml",
        ],
        "memstead_mem_delete" => &[
            // Response-shape fields.
            "deleted_from_router",
            "files_deleted",
            // Scrubbed-entry audit field surfaces the policy
            // side-effects in one round-trip.
            "allowlist_entries_removed",
            "table",
            "pattern",
            "from",
            "to",
            // Allowlist tables named verbatim — `mem_management.*`
            // is two tables; the slashed-token check resolves each
            // half independently.
            "mem_management.create",
            "mem_management.delete",
            "mem_management",
            "create",
            "delete",
            // Error codes named literally in the description.
            "UNKNOWN_MEM",
            "MEM_PATH_NOT_ALLOWED",
            "MEM_REFERENCED_BY_POLICY",
            "MEM_HAS_INCOMING_REFS",
            // `.memstead/workspace.toml` is named literally in the
            // MEM_REFERENCED_BY_POLICY recovery guidance — point
            // operators at the cross-link grant they have to revoke.
            // The slashed-token check resolves each half against
            // this list.
            ".memstead",
            "workspace.toml",
            // Workspace-policy token referenced verbatim in the
            // policy-grant description.
            "cross_mem_links",
            // Disk-cleanup warning emitted when `delete_files=true`
            // leaves a backend-visible artifact behind — either the
            // folder rmdir failed or the git-branch ref-edit
            // transaction failed.
            "MEM_FILES_NOT_DELETED",
            // Error-envelope `details` field references.
            "details.referring_mems",
            "details.referrers",
            "details.candidate",
            "details.patterns",
            // `details` fields named in the MEM_FILES_NOT_DELETED
            // warning's payload.
            "details.reason",
            "details.path",
            "details.error",
            // Reason discriminator literals carried in the warning's
            // `details.reason`.
            "rmdir_failed",
            "backend_prune_failed",
            // Cross-tool references — `memstead_relate` / `memstead_update`
            // appear in the `MEM_HAS_INCOMING_REFS` recovery guidance
            // (remove the offending edges before retrying).
            "memstead_health",
            "memstead_overview",
            "memstead_relate",
            "memstead_update",
            // Config-discovery tokens embedded in the description.
            "no_allowlist_configured",
            "no_match",
            // Workspace-config tokens referenced verbatim.
            "mem_management.delete",
        ],
        // `memstead_mem_set_schema` — the integrity-driven schema-migration
        // trigger. Response discriminator values, the findings shape,
        // and the cross-referenced tools/params named in the
        // description.
        "memstead_mem_set_schema" => &[
            "outcome",
            "noop",
            "switched",
            "migration_started",
            "migration_pending",
            "findings",
            "schema_pin",
            "migration_target",
            "relations_unset",
            "memstead_schema",
            "memstead_update",
            "memstead_mem_set_version",
            "UNKNOWN_MEM",
            "SCHEMA_NOT_FOUND",
            "INVALID_INPUT",
        ],
        "memstead_mem_set_version" => &[
            // Response-shape fields.
            "mem",
            "old_version",
            "new_version",
            "warnings",
            // Error codes named literally in the description.
            "INVALID_INPUT",
            "UNKNOWN_MEM",
            "READ_ONLY_MOUNT",
            // Warning code emitted on concurrent-drift detection.
            "MEM_RELOADED",
            // Other named codes / types the description cites.
            "MemConfig",
            "write_mem_config",
            // Config-blob layout strings the description names.
            // `.mem` is the sealed-archive extension.
            ".memstead",
            ".mem",
            "config.json",
            "__MEMSTEAD",
            "mems",
            // Cross-tool reference.
            "memstead_export",
            // Allowlist token (description disclaims operator-mode bypass).
            "mem_management",
            // Version-default literal.
            "0.1.0",
        ],
        // Workspace-policy mutation tools.
        "memstead_workspace_grant_cross_link" => &[
            // Response-shape + section-name refs.
            "from",
            "to",
            "warnings",
            "cross_mem_links",
            // Idempotency warning + conflict error codes.
            "GRANT_ALREADY_PRESENT",
            "CROSS_LINK_CONFLICT",
            "WORKSPACE_NOT_INITIALISED",
            "INVALID_TOML",
            "IO_ERROR",
            // F7 workflow cross-tool references.
            "memstead_mem_create",
            "memstead_mem_delete",
            "memstead_relate",
            "memstead_workspace_revoke_cross_link",
            // Workspace config path tokens.
            ".memstead",
            "workspace.toml",
        ],
        "memstead_workspace_revoke_cross_link" => &[
            "from",
            "to",
            "warnings",
            "cross_mem_links",
            "GRANT_NOT_FOUND",
            "MEM_REFERENCED_BY_POLICY",
            "WORKSPACE_NOT_INITIALISED",
            "INVALID_TOML",
            "IO_ERROR",
            "memstead_mem_delete",
            ".memstead",
            "workspace.toml",
        ],
        "memstead_workspace_allow_create" => &[
            "pattern",
            "schemas",
            "before",
            "default_cross_links",
            "warnings",
            // Section names cited in the description.
            "mem_management.create",
            "cross_mem_links",
            // Idempotency warning + related error codes.
            "RULE_ALREADY_PRESENT",
            "BEFORE_PATTERN_NOT_FOUND",
            "WORKSPACE_NOT_INITIALISED",
            "MEM_PATH_NOT_ALLOWED",
            // Schema-differ refusal code + its structured recovery payload.
            "RULE_EXISTS_SCHEMAS_DIFFER",
            "details.stored_schemas",
            "details.requested_schemas",
            "details.recovery",
            // Cross-tool refs.
            "memstead_mem_create",
            "memstead_workspace_grant_cross_link",
            "memstead_overview",
            "memstead_workspace_revoke_create",
            // Rule-derived cross-link grant is surfaced under this
            // workspace-policy posture key.
            "cross_mem_links_from_rules",
            // `.memstead/workspace.toml` slashed token.
            ".memstead",
            "workspace.toml",
        ],
        "memstead_workspace_revoke_create" => &[
            "pattern",
            "warnings",
            "RULE_NOT_FOUND_NOOP",
            "WORKSPACE_NOT_INITIALISED",
            "INVALID_TOML",
            "IO_ERROR",
            "memstead_workspace_allow_create",
            ".memstead",
            "workspace.toml",
        ],
        "memstead_workspace_allow_delete" => &[
            "pattern",
            "warnings",
            "mem_management.delete",
            "RULE_ALREADY_PRESENT",
            "WORKSPACE_NOT_INITIALISED",
            "MEM_PATH_NOT_ALLOWED",
            "memstead_mem_delete",
            "memstead_workspace_allow_create",
            ".memstead",
            "workspace.toml",
        ],
        "memstead_workspace_revoke_delete" => &[
            "pattern",
            "warnings",
            "RULE_NOT_FOUND_NOOP",
            "WORKSPACE_NOT_INITIALISED",
            "INVALID_TOML",
            "IO_ERROR",
            "memstead_workspace_allow_delete",
            ".memstead",
            "workspace.toml",
        ],
        _ => &[],
    }
}

// --- Drift-guard tests + load-bearing substring invariants ---------------
//
// The two drift-guard tests below walk the full WarningHint variant / error
// code set rather than a hand-maintained substring list. When a future
// change adds a new code, its owner must update a tool description before
// these tests pass — a contract that forces "description touch" on every
// code addition.
// --------------------------------------------------------------------------

/// Structured error codes that `engine_err_with_suggestions` may emit on
/// the MCP wire. Exhaustive with the `match` in that function — every
/// `EngineError` variant produces a `{code, message, details}` envelope
/// on `structured_content`. Adding an `EngineError` variant without
/// extending this list fails `every_error_code_appears_in_a_description`,
/// forcing a description touch so agents can cross-reference the `code`
/// back to a calling tool.
const STRUCTURED_ERROR_CODES: &[&str] = &[
    // Lookup
    "ENTITY_NOT_FOUND",
    "ENTITY_ALREADY_EXISTS",
    "UNKNOWN_MEM",
    // Optimistic locking / structural
    "HASH_MISMATCH",
    "RELATIONSHIP_CYCLE",
    // Schema vocabulary violations
    "UNKNOWN_SECTION",
    "UNKNOWN_METADATA_FIELD",
    "UNKNOWN_ENTITY_TYPE",
    "INVALID_ENUM_VALUE",
    "INVALID_REL_TYPE",
    "INVALID_REL_SHAPE",
    // Update-path rules
    "READ_ONLY_FIELD",
    "REQUIRED_FIELD_UNSET",
    "SET_AND_UNSET_CONFLICT",
    "CONFLICTING_SECTION_MODES",
    "SECTION_NOT_UPDATABLE",
    "PATCH_OLD_NOT_FOUND",
    "PATCH_SECTION_EMPTY",
    // Mem invariants
    "CROSS_MEM_LINK_NOT_ALLOWED",
    "CROSS_MEM_TARGET_NOT_FOUND",
    "MEM_NOT_WRITABLE",
    "MEM_NAME_COLLISION",
    "MEM_PATH_NOT_ALLOWED",
    "MEM_SCHEMA_NOT_ALLOWED",
    "MEM_BRANCH_MISSING",
    "MEM_REFERENCED_BY_POLICY",
    // Refuse-on-write-mem-referrers (replaces force flag).
    "HAS_INCOMING_REFS",
    // Stub guards
    "STUB_NOT_UPDATABLE",
    "STUB_NOT_RENAMABLE",
    "STUB_CANNOT_RELATE",
    // Relate-target id-grammar guard
    "INVALID_ENTITY_ID",
    // Relate-remove refused because source body still wiki-links target
    "RELATION_HAS_BODY_LINKS",
    // Per-edge description posture.
    // Both used to fall through to the wildcard `_ => INTERNAL`; now
    // ship typed envelopes with structured details.
    "DESCRIPTION_NOT_PERMITTED",
    "MISSING_REQUIRED_DESCRIPTION",
    // Strict wiki-link/relation invariant — typed envelope.
    "WIKILINK_WITHOUT_RELATION",
    // Rename-policy/partial-failure variants — typed envelopes.
    "RENAME_BLOCKED_BY_CROSS_MEM_POLICY",
    "RENAME_PARTIAL_FAILURE",
    // Schema resolution
    "SCHEMA_NOT_FOUND",
    "SCHEMA_RESOLVER_INIT_FAILED",
    // Fallback / boundary
    "PARSE_ERROR",
    "MEM_ERROR",
    "INVALID_INPUT",
    "VCS_ERROR",
    "INTERNAL_IO_ERROR",
    "CONFIG_ERROR",
    "EXPORT_ERROR",
    "WORKSPACE_SCHEMAS_ERROR",
    // MCP filter (workspace-level `[mcp].disabled_tools`)
    "TOOL_DISABLED",
    // memstead_changes_since cursor resolution
    "INVALID_CURSOR",
];

/// Joins every tool description and the server-level `instructions` into a
/// single haystack. The drift guards assert "this code is named *somewhere*
/// on the agent-facing surface" — the load-bearing substring tests below
/// lock which exact tool must carry each clause.
fn all_description_text() -> String {
    let mut acc = String::new();
    for (_, desc) in descriptions() {
        acc.push_str(&desc);
        acc.push('\n');
    }
    acc.push_str(server_instructions_text());
    acc
}

/// Server-level `instructions` preamble. Re-stated literally so the test
/// doesn't need a running `McpServer` — drift between this copy and the
/// `#[tool_handler(instructions = …)]` literal is caught by
/// `server_instructions_copy_matches_live` below.
const SERVER_INSTRUCTIONS_COPY: &str = "Memstead: schema-agnostic graph engine for typed, interconnected markdown entities. Each mem is a typed model of a chosen subject — its modal flavour follows from its schema (knowledge / planning / inquiry / spec / hybrid). Each mem pins one schema; types and relationships are vocabulary-controlled. Granularity: a mem is the packaged unit — a whole typed model, designed for 1,000-5,000 entities; an entity is never called a mem (a mem is not one 'memory'/fact). Cold-start: call memstead_overview first for the schema catalogue (`{ref, description}` per schema), mem inventory, and communities (token-budgeted; drill via include/hints). Schema-discovery contract: each writable mem pins one schema (visible on overview's `## Mems` entries). Before any memstead_create / memstead_update / memstead_relate against mem X, call memstead_schema(name=<X.schema_ref>) once per session. The default reply is the lite structural skeleton — entity-type names with section keys and metadata-field shapes, relationship names with endpoint constraints, plus every legality flag (required sections/fields, alias_target_rel_type, manual-authoring posture, acyclic) — enough to plan a legal write. Pass verbosity: full for the prose layer (per-section write_rules, writing_guidance, system_context, when_to_use) before substantial authoring against an unfamiliar schema. Cache for the session — schema is workspace-stable. Schema-conformance errors carry recovery payloads as a fallback (UNKNOWN_SECTION, UNKNOWN_METADATA_FIELD, INVALID_ENUM_VALUE, REQUIRED_FIELD_UNSET, INVALID_REL_TYPE, INVALID_REL_SHAPE, MISSING_REQUIRED_SECTION) — fix from `details` rather than re-fetching the schema after every error. Edge model is alias: body wiki-links `[[X]]` are foreign-key references to entries in the auto-managed `## Relationships` section. Schemas with `alias_target_rel_type` auto-emit relations of that rel-type (e.g. REFERENCES) from each body wiki-link via the alias-synthesis pass; explicit author of the named rel-type refuses with RELATION_MANUAL_AUTHORING_FORBIDDEN. Schemas without the pointer refuse unbacked body wiki-links with WIKILINK_WITHOUT_RELATION. Removing a relation while body wiki-links to its target remain refuses only when no other relation to that target survives (RELATION_HAS_BODY_LINKS — set-membership semantics). Shared mutation contract: every mutation accepts an optional note (≤280 chars) landing in the commit body as provenance; when [mutations].require_notes=true a missing note refuses with NOTE_MISSING. Real writes return commit_sha (per-mem git; gitdir via memstead_health include_config=true) — use it as the since cursor for memstead_changes_since polling. Schema-conformance recovery payloads carry the fix material in place: details.declared / details.allowed with nearest-match suggestion, details.field_description, details.enum_values, and the type's details.type_write_rules. After a successful memstead_relate the touched entity's on-disk _hash advances; the relate response's _hash is the next valid expected_hash (no re-read needed) — no-op relates (duplicate add, remove-nonexistent) echo the unchanged _hash, which stays valid. Common workflows: search entities by content/structure (memstead_search — omit query for pure metadata filter); read one (memstead_entity — `_hash` is the optimistic-locking token for mutations); read one schema (memstead_schema); create/update/relate/rename/delete entities (memstead_create, memstead_update, memstead_relate, memstead_rename, memstead_delete); manage workspace mems including planning phases (memstead_mem_create, memstead_mem_delete); inspect drift and per-mem config (memstead_health); poll commit deltas for incremental sync (memstead_changes_since). Errors and warnings ship as { code, message, details } on structured_content; branch on the stable UPPER_SNAKE_CASE code. The text channel mirrors the same code inline as `ERROR [<CODE>]: <message>` so consumers that only read `result.content[0].text` still recover the code with a one-line regex. Never edit `.md` spec files directly — always go through Memstead tools. Error codes: ENTITY_NOT_FOUND, ENTITY_ALREADY_EXISTS, UNKNOWN_MEM, HASH_MISMATCH, RELATIONSHIP_CYCLE, UNKNOWN_SECTION, UNKNOWN_METADATA_FIELD, UNKNOWN_ENTITY_TYPE, INVALID_ENUM_VALUE, INVALID_REL_TYPE, INVALID_REL_SHAPE, READ_ONLY_FIELD, REQUIRED_FIELD_UNSET, SET_AND_UNSET_CONFLICT, CONFLICTING_SECTION_MODES, SECTION_NOT_UPDATABLE, PATCH_OLD_NOT_FOUND, PATCH_SECTION_EMPTY, CROSS_MEM_LINK_NOT_ALLOWED, CROSS_MEM_TARGET_NOT_FOUND, CROSS_MEM_EDGE_NOT_DECLARED, MEM_NOT_WRITABLE, MEM_NAME_COLLISION, MEM_PATH_NOT_ALLOWED, INVALID_MEM_NAME, MEM_SCHEMA_NOT_ALLOWED, MEM_BRANCH_MISSING, MEM_REFERENCED_BY_POLICY, HAS_INCOMING_REFS, STUB_NOT_UPDATABLE, STUB_NOT_RENAMABLE, STUB_CANNOT_RELATE, INVALID_ENTITY_ID, WIKILINK_WITHOUT_RELATION, RELATION_HAS_BODY_LINKS, MISSING_REQUIRED_DESCRIPTION, DESCRIPTION_NOT_PERMITTED, RELATION_MANUAL_AUTHORING_FORBIDDEN, SCHEMA_NOT_FOUND, SCHEMA_RESOLVER_INIT_FAILED, PARSE_ERROR, MEM_ERROR, INVALID_INPUT, VCS_ERROR, INTERNAL_IO_ERROR, CONFIG_ERROR, EXPORT_ERROR, WORKSPACE_SCHEMAS_ERROR, SCHEMA_CACHE_COLLISION, TOOL_DISABLED, INVALID_CURSOR. Health warnings: OUTER_REPO_NOT_IGNORING_MEM_REPO (workspace embedded in an outer git checkout that does not ignore mem-repo/), SUSPICIOUS_NESTED_PREFIX (nested-prefix drift — fix via memstead_update), DUPLICATE_SECTION_HEADING (a section key whose ## Heading appeared twice; first body kept). Drift warning on any tool: MEM_RELOADED (a sibling engine committed to this mem-repo; the engine auto-reloaded — response content is fresh but cached expected_hash values are stale; re-derive before the next mutation). Relate warnings: AUTO_STUB_CREATED. Delete warning: RESIDUAL_STUB_FOR_READONLY_REFERRERS. Boot warnings: PARSED_RELATION_INVALID, AMBIGUOUS_DESCRIPTION_DELIMITER, MISSING_REQUIRED_DESCRIPTION, DESCRIPTION_NOT_PERMITTED. Mutation warning: MISSING_REQUIRED_OUTGOING.";

fn server_instructions_text() -> &'static str {
    SERVER_INSTRUCTIONS_COPY
}

/// Enumeration drift guard: every `WarningHint` variant's `code()` must
/// appear in at least one tool description (or in the server-level
/// `instructions`). Triggered by `WarningHint::all_samples()` — adding a
/// variant without extending a description fails the test.
#[test]
fn every_warning_code_appears_in_a_description() {
    let haystack = all_description_text();
    let mut missing = Vec::new();
    for w in &memstead_git_branch::ops::WarningHint::all_samples() {
        let code = w.code();
        if !haystack.contains(code) {
            missing.push(code);
        }
    }
    assert!(
        missing.is_empty(),
        "WarningHint code(s) not referenced by any tool description: {missing:?}. \
         Update the relevant tool's description, or verify the variant is \
         still used."
    );
}

/// Extract the warning codes the server `instructions` advertise — the
/// UPPER_SNAKE tokens that follow a `... warning(s):` label, up to the
/// next non-code character.
fn advertised_warning_codes(text: &str) -> Vec<String> {
    let mut codes = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find("warning") {
        rest = &rest[pos + "warning".len()..];
        let after_plural = rest.strip_prefix('s').unwrap_or(rest);
        let Some(after_colon) = after_plural.strip_prefix(':') else {
            continue;
        };
        let run: String = after_colon
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || *c == '_' || *c == ',' || *c == ' ')
            .collect();
        for tok in run.split(',') {
            let t = tok.trim();
            if !t.is_empty() && t.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                codes.push(t.to_string());
            }
        }
    }
    codes
}

/// Reverse drift guard: every warning code the `instructions` advertise
/// must map to an emittable `WarningHint` variant. The inverse of
/// `every_warning_code_appears_in_a_description` — this one catches an
/// advertised-but-never-emitted code (e.g. the retired
/// `CARDINALITY_VIOLATION`), which that test could not see because it
/// only iterates emittable variants.
#[test]
fn every_advertised_warning_code_has_an_emitting_path() {
    let emittable: std::collections::HashSet<&'static str> =
        memstead_git_branch::ops::WarningHint::all_samples()
            .iter()
            .map(|w| w.code())
            .collect();
    let advertised = advertised_warning_codes(server_instructions_text());
    assert!(
        !advertised.is_empty(),
        "parser found no advertised warning codes — the roster format may have changed"
    );
    let orphans: Vec<_> = advertised
        .iter()
        .filter(|c| !emittable.contains(c.as_str()))
        .collect();
    assert!(
        orphans.is_empty(),
        "advertised warning code(s) with no emitting WarningHint variant: {orphans:?}"
    );
}

/// Enumeration drift guard: every structured MCP error code emitted through
/// `engine_err_with_suggestions` must appear in at least one tool
/// description. Extend `STRUCTURED_ERROR_CODES` as future items add more.
/// A miss means an agent may see a `code` on the wire it can't
/// cross-reference back to a calling tool.
#[test]
fn every_error_code_appears_in_a_description() {
    let haystack = all_description_text();
    let mut missing = Vec::new();
    for code in STRUCTURED_ERROR_CODES {
        if !haystack.contains(code) {
            missing.push(*code);
        }
    }
    assert!(
        missing.is_empty(),
        "Structured error code(s) not referenced by any tool description: {missing:?}"
    );
}

/// Every mutation that mentions `commit_sha` must qualify it with the
/// per-mem git storage location and point agents at the canonical
/// discovery path (`memstead_health { include_config: true }`) so they don't
/// try `git log <sha>` at the project root. Misinterpretation of the SHA
/// origin was the #1 source of agent confusion before this lock.
///
/// The discovery qualifier is `memstead_health` with `include_config` — the
/// gitdir location is per-mem configurable (see vcs-config.md Phase 1)
/// and the `memstead_health.vcs` subobject is the LLM-facing discovery
/// surface.
#[test]
fn every_mutation_description_clarifies_commit_sha_origin() {
    const MUTATION_TOOLS: &[&str] = &[
        "memstead_create",
        "memstead_update",
        "memstead_delete",
        "memstead_rename",
        "memstead_relate",
    ];
    let mut violations = Vec::new();
    for (name, desc) in descriptions() {
        if !MUTATION_TOOLS.contains(&name.as_str()) {
            continue;
        }
        if !desc.contains("commit_sha") {
            continue; // tool doesn't mention it — not a violation
        }
        let has_per_mem = desc.contains("per-mem git");
        let has_discovery = desc.contains("memstead_health") && desc.contains("include_config");
        // The shared mutation contract in the server `instructions`
        // carries the per-mem-git qualifier and the gitdir discovery
        // pointer once for every mutation — a description that points
        // there satisfies the invariant without restating it.
        let has_contract_pointer = desc.contains("server instructions");
        if !(has_contract_pointer || (has_per_mem && has_discovery)) {
            violations.push(format!(
                "{name}: description mentions `commit_sha` but omits the \
                 per-mem-git qualifier or the \
                 `memstead_health include_config=true` discovery pointer"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "commit_sha origin-drift violations:\n  {}",
        violations.join("\n  ")
    );
}

/// Helper — return the description for one tool. Panics if the tool is
/// absent (indicates the surface itself has drifted, which other tests
/// already catch).
fn description_of(tool_name: &str) -> String {
    descriptions()
        .into_iter()
        .find(|(n, _)| n == tool_name)
        .unwrap_or_else(|| panic!("{tool_name} must exist"))
        .1
}

/// Load-bearing substring invariants — one assertion per load-bearing
/// code name or param reference. These lock the *exact tool* that must
/// carry the clause (the drift guards above only assert "somewhere"
/// presence).

#[test]
fn memstead_update_description_names_hash_mismatch_code() {
    let desc = description_of("memstead_update");
    assert!(
        desc.contains("HASH_MISMATCH"),
        "memstead_update must name the HASH_MISMATCH error code so agents know what to branch on."
    );
}

#[test]
fn memstead_update_description_names_dry_run_recovery() {
    let desc = description_of("memstead_update");
    assert!(
        desc.contains("dry_run"),
        "memstead_update must name `dry_run`."
    );
    assert!(
        desc.to_lowercase().contains("recover"),
        "memstead_update must flag dry_run as the recovery path for stale hashes."
    );
}

#[test]
fn memstead_update_description_mentions_metadata_unset() {
    let desc = description_of("memstead_update");
    assert!(
        desc.contains("metadata_unset"),
        "memstead_update must name `metadata_unset` — the field exists on the wire and \
         agents need to know it."
    );
}

#[test]
fn memstead_update_description_mentions_patch_all() {
    let desc = description_of("memstead_update");
    assert!(
        desc.contains("patch_sections") && desc.contains("all"),
        "memstead_update must document the `all` flag on `patch_sections`."
    );
}

#[test]
fn memstead_relate_description_names_warning_codes() {
    let desc = description_of("memstead_relate");
    for code in ["DUPLICATE_RELATIONSHIP", "NO_SUCH_RELATIONSHIP"] {
        assert!(
            desc.contains(code),
            "memstead_relate must name the {code} warning code."
        );
    }
}

#[test]
fn memstead_relate_description_names_empty_commit_convention() {
    let desc = description_of("memstead_relate");
    assert!(
        desc.contains("commit_sha") && desc.contains("empty"),
        "memstead_relate must document the empty-`commit_sha` no-op convention \
         (duplicate-add / remove-nonexistent)."
    );
}

#[test]
fn memstead_rename_description_names_slug_noop_warning_code() {
    let desc = description_of("memstead_rename");
    assert!(
        desc.contains("TITLE_NORMALIZED_TO_SLUG_NOOP"),
        "memstead_rename must name the TITLE_NORMALIZED_TO_SLUG_NOOP warning code."
    );
}

#[test]
fn memstead_health_description_names_all_include_keys() {
    let desc = description_of("memstead_health");
    for key in memstead_base::ops::health::HEALTH_INCLUDE_KEYS {
        assert!(
            desc.contains(key),
            "memstead_health must name include key `{key}`."
        );
    }
}

#[test]
fn memstead_overview_token_budget_describes_heavy_content_scope() {
    let schema = schema_for("memstead_overview");
    let needle = "\"token_budget\"";
    let idx = schema
        .find(needle)
        .unwrap_or_else(|| panic!("memstead_overview must declare token_budget; got: {schema}"));
    let window_end = (idx + 800).min(schema.len());
    let window = &schema[idx..window_end];
    assert!(
        window.contains("heavy content"),
        "memstead_overview's `token_budget` description must state the heavy-content scope; got window: {window}"
    );
}

#[test]
fn memstead_changes_since_description_names_entity_type() {
    let desc = description_of("memstead_changes_since");
    assert!(
        desc.contains("entity_type"),
        "memstead_changes_since must document the `entity_type` field on events."
    );
}

#[test]
fn memstead_overview_description_names_overview_modes() {
    let desc = description_of("memstead_overview");
    // Actual mode values: complete / reduced / overbudget. The
    // "reduced" mode is the load-bearing signal for an agent — it
    // triggers the `hints[]` follow-up loop.
    assert!(
        desc.contains("reduced"),
        "memstead_overview must name the `reduced` overview_mode — it drives \
         hint-driven follow-up calls."
    );
    assert!(
        desc.contains("overbudget") || desc.contains("complete"),
        "memstead_overview must name at least one non-reduced overview_mode so \
         agents can decode the full lifecycle."
    );
}

/// The server-level `instructions` advertises the unified envelope
/// shape so agents that haven't yet read a tool's description still
/// know the `{ code, message, details }` contract.
#[test]
fn server_instructions_advertise_envelope_shape() {
    let i = server_instructions_text();
    assert!(
        i.contains("code"),
        "server instructions must advertise the envelope's `code` field."
    );
    assert!(
        i.contains("details"),
        "server instructions must advertise the envelope's `details` field."
    );
    assert!(
        i.contains("message"),
        "server instructions must advertise the envelope's `message` field."
    );
}

/// Guard: the `SERVER_INSTRUCTIONS_COPY` constant above must be a verbatim
/// match of the live `#[tool_handler(instructions = …)]` literal in
/// `server.rs`. Without this test a future rewrite could drift the macro
/// literal while leaving the test copy untouched, silently turning the
/// envelope-shape guard into a tautology.
///
/// Reads `server.rs` as source text — cheap and harness-free, no running
/// server required. If the macro-literal accessor ever becomes publicly
/// available via `rmcp` we can replace this with a live-read.
#[test]
fn server_instructions_copy_matches_live() {
    let source = include_str!("../src/server.rs");
    // The macro literal is on one line: `instructions = "…"` inside the
    // tool_handler attribute. Split on the sentinel to pull out the string.
    let marker = "instructions = \"";
    let start = source
        .find(marker)
        .expect("server.rs must contain `instructions = \"…\"` in the tool_handler macro")
        + marker.len();
    let rest = &source[start..];
    let end = rest
        .find('"')
        .expect("tool_handler `instructions` literal must be closed");
    let live = &rest[..end];
    assert_eq!(
        live, SERVER_INSTRUCTIONS_COPY,
        "SERVER_INSTRUCTIONS_COPY drifted from the live tool_handler literal. \
         Update the constant in this test file, or fix the macro literal in \
         server.rs."
    );
}

/// `memstead_overview` is the documented cold-start entry point — the server
/// `instructions` direct agents to call it first. Tagging it with
/// `_meta.anthropic/alwaysLoad = true` opts it out of Claude Code's
/// `ToolSearch` defer set so it is always loaded into the agent's
/// context, removing the cold-start round-trip.
///
/// No other tool currently carries this tag — keeping the always-loaded
/// surface to a single entry point is the design.
#[test]
fn memstead_overview_carries_always_load_meta() {
    let tools = McpServer::tool_router().list_all();
    let overview = tools
        .iter()
        .find(|t| t.name == "memstead_overview")
        .expect("memstead_overview must be registered");
    let meta = overview
        .meta
        .as_ref()
        .expect("memstead_overview must carry a `_meta` map");
    let always_load = meta
        .0
        .get("anthropic/alwaysLoad")
        .expect("memstead_overview must carry `_meta.anthropic/alwaysLoad`");
    assert_eq!(
        always_load.as_bool(),
        Some(true),
        "`anthropic/alwaysLoad` must be the boolean true"
    );

    for t in &tools {
        if t.name == "memstead_overview" {
            continue;
        }
        let has_always_load = t
            .meta
            .as_ref()
            .and_then(|m| m.0.get("anthropic/alwaysLoad"))
            .is_some();
        assert!(
            !has_always_load,
            "{} unexpectedly carries `anthropic/alwaysLoad` — only memstead_overview should",
            t.name
        );
    }
}

/// Ad-hoc measurement printout for the Item-D trim audit. Run with:
///
///     cargo test --features mem-repo -p memstead-mcp --test tool_surface \
///         print_description_sizes -- --nocapture --ignored
///
/// Reports per-tool word/byte sizes plus the server-instructions block
/// so an implementing agent can quantify the cold-start `tools/list`
/// surface and watch the trim's effect. `#[ignore]` keeps it out of the
/// default `cargo nextest` sweep — measurement, not regression.
#[test]
#[ignore]
fn print_description_sizes() {
    let mut tools = descriptions();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    let mut total_bytes = 0usize;
    let mut total_words = 0usize;
    println!("\n{:<22} {:>6} {:>6}", "tool", "words", "bytes");
    println!("{}", "-".repeat(40));
    for (name, desc) in &tools {
        let words = desc.split_whitespace().count();
        let bytes = desc.len();
        total_bytes += bytes;
        total_words += words;
        println!("{:<22} {:>6} {:>6}", name, words, bytes);
    }
    println!("{}", "-".repeat(40));
    println!(
        "{:<22} {:>6} {:>6}",
        "TOOLS_SUBTOTAL", total_words, total_bytes
    );
    let instr = SERVER_INSTRUCTIONS_COPY;
    let instr_words = instr.split_whitespace().count();
    let instr_bytes = instr.len();
    println!(
        "{:<22} {:>6} {:>6}",
        "instructions", instr_words, instr_bytes
    );
    println!(
        "{:<22} {:>6} {:>6}",
        "GRAND_TOTAL",
        total_words + instr_words,
        total_bytes + instr_bytes
    );
}
