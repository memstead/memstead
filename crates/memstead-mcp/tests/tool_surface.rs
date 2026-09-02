#![cfg(feature = "mem-repo")]
//! Locks the shape of the agent-facing MCP surface.
//!
//! The surface is the `EXPECTED_TOOLS` list below — read-only:
//! `memstead_entity`, `memstead_health`, `memstead_overview`,
//! `memstead_schema`, `memstead_search`; mutation: `memstead_create`,
//! `memstead_delete`, `memstead_relate`, `memstead_rename`,
//! `memstead_update`; process: `memstead_check`; admin:
//! `memstead_changes_since`, `memstead_diff`, `memstead_reload`; mem
//! lifecycle: `memstead_mem_create`, `memstead_mem_configure`,
//! `memstead_mem_delete`, `memstead_mem_set_schema`,
//! `memstead_mem_set_version`. The asserted count is
//! `EXPECTED_TOOLS.len()` —
//! `tool_count_matches_expected_set` pins it against the live router.
//!
//! Several former tools are now folded in and must not re-appear:
//! `memstead_list` → `memstead_search` (omit `text` for structural filters);
//! `memstead_path` → `memstead_search related_to=<id> depth=N`;
//! `memstead_schema_list` + `memstead_schema_info` collapsed and re-emerged
//! as a single `memstead_schema(name=...)` reader; `memstead_update_community`,
//! `memstead_batch_update`, `memstead_export`, `memstead_stats`,
//! `memstead_status`, `memstead_relations`, `memstead_context`,
//! `memstead_type_info` are gone (`memstead_status` never minted — D11).
//! So is the whole `memstead_workspace_*` family (allow/revoke create,
//! allow/revoke delete, grant/revoke cross-link): workspace policy is
//! the operator deciding what an agent may do, and the constrained
//! party does not hold the keys to its constraints, so it lives on
//! operator surfaces (the CLI and the operator-authenticated web API)
//! and never here.
//!
//! Mem lifecycle is not workspace configuration. The lifecycle family
//! (`memstead_mem_create` / `memstead_mem_delete` /
//! `memstead_mem_set_schema` / `memstead_mem_set_version`)
//! creates/removes/reconfigures a whole mem at runtime and stays on
//! this surface, gated by the `[mem_management]` allowlists the
//! operator owns. Every MCP tool must carry the `memstead_` prefix —
//! an un-namespaced `workspace_*` tool (or any other non-`memstead_`
//! tool) must fail this test.
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
    // Mutation (6)
    "memstead_create",
    "memstead_delete",
    "memstead_relate",
    "memstead_rename",
    "memstead_retype",
    "memstead_update",
    // Process tier (1) — the bundle's single deliberate tool
    // addition (agent-trust plan 14): the check operation.
    "memstead_check",
    // Admin (3)
    "memstead_changes_since",
    "memstead_diff",
    "memstead_reload",
    // Mem lifecycle (5)
    "memstead_mem_configure",
    "memstead_mem_create",
    "memstead_mem_delete",
    "memstead_mem_set_schema",
    "memstead_mem_set_version",
    // No workspace-policy family: the six `memstead_workspace_*` tools
    // were removed on 2026-08-20. An agent completes the dynamic mem
    // lifecycle only within permissions the operator already granted;
    // widening them is a CLI / operator-web-API act, and a policy-gated
    // refusal names the command to report.
];

fn current_tool_names() -> Vec<String> {
    McpServer::tool_router()
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// The lean (filesystem) server flavour's tool names — the second surface the
/// folded-tools ban must hold on (cross-plan rule c).
fn filesystem_tool_names() -> Vec<String> {
    use memstead_mcp::filesystem_server::FilesystemMcpServer;
    FilesystemMcpServer::tool_router()
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
            "Tool '{}' lacks the required memstead_ prefix — every MCP tool must be namespaced under memstead_",
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
/// CLI crate (`memstead-cli`). The layering rule: CLI and MCP are
/// sibling surfaces over the engine — so MCP tools that need
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

/// Explicit guard for removed/abstained tools — named so a
/// re-introduction fails with an obvious per-tool message, not just
/// "drift" on the set diff.
///
/// The rationale argues against the MCP-only **consumer profile**
/// (`dev/handbook/agent-surfaces.md`, "Consumer profile"), not merely
/// the agent-with-a-shell case. All three of plenum finding 18's axes
/// were weighed: (1) *boot cost* — the MCP server stays warm, so the
/// engine-boot cost that motivates CLI batching does not apply;
/// (2) *agent-context cost* — mass create/update payloads are
/// file-scale, and their report-all responses would flood an agent
/// context, which is why mass ingest is deliberately off the profile;
/// (3) *atomicity* — atomic multi-relate belongs on
/// `memstead_relate`'s list form, never a second tool, and atomic
/// mass-create stays CLI-only because its payloads are file-scale.
/// Export is distribution, off the profile by contract.
#[test]
fn mcp_does_not_expose_batch_update_or_export() {
    let names = current_tool_names();
    for removed in [
        "memstead_batch_update",
        "memstead_batch_create",
        "memstead_batch_relate",
        "memstead_export",
    ] {
        assert!(
            !names.iter().any(|n| n == removed),
            "{removed} must not be re-exposed — the MCP consumer profile \
             (dev/handbook/agent-surfaces.md) deliberately excludes distribution and \
             mass ingest: batch payloads are file-scale and their responses would flood \
             an agent context; atomic multi-relate belongs on memstead_relate's list \
             form, never a second tool; export is human/CLI-triggered distribution."
        );
    }
}

/// `memstead_stats` / `memstead_relations` / `memstead_context` are folded into
/// `memstead_entity` and `memstead_overview`; `memstead_type_info` is folded into
/// `memstead_overview.schemas[]`. They must stay gone even if someone
/// re-adds one by copy-paste.
///
/// `memstead_status` is banned here too (D11): the CLI `stats`→`status`
/// rename mints **no** new MCP tool — health remains the single agent
/// dashboard (a second status tool is the response-shape sprawl the
/// tool-surface policy exists to prevent; the asymmetry is recorded in
/// `agent-surfaces.md`). The ban holds on **both** server flavours (cross-plan
/// rule c) — the full `McpServer` and the lean `FilesystemMcpServer`.
#[test]
fn mcp_does_not_expose_folded_stats_tools() {
    let full = current_tool_names();
    let lean = filesystem_tool_names();
    for removed in [
        "memstead_stats",
        "memstead_status",
        "memstead_relations",
        "memstead_context",
        "memstead_type_info",
    ] {
        assert!(
            !full.iter().any(|n| n == removed),
            "{removed} was folded into a sibling tool — do not re-expose (full server)."
        );
        assert!(
            !lean.iter().any(|n| n == removed),
            "{removed} was folded into a sibling tool — do not re-expose (lean/filesystem server)."
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
/// (forked Claude-Code subagents, parallel
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
        "memstead_create" | "memstead_update" | "memstead_rename" | "memstead_retype"
        | "memstead_check" => HintTriple {
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
        // `memstead_mem_configure` — sets curation fields (title /
        // description / subject) through the same setters the CLI
        // verbs use. Mutation but not destructive; idempotent — the
        // same call twice lands the same state.
        "memstead_mem_configure" => HintTriple {
            read_only: Some(false),
            destructive: Some(false),
            idempotent: Some(true),
            open_world: Some(false),
        },
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

/// Helper — returns (surface, tool_name, description) triples for every
/// tool on BOTH server flavours: the full `McpServer` ("full") and the
/// lean `FilesystemMcpServer` ("lean"). Every Memstead tool MUST declare
/// a description, and both flavours' descriptions go through the same
/// lints — an agent gets the same contract quality regardless of build.
fn descriptions() -> Vec<(&'static str, String, String)> {
    use memstead_mcp::filesystem_server::FilesystemMcpServer;

    let mut out = Vec::new();
    for (surface, tools) in [
        ("full", McpServer::tool_router().list_all()),
        ("lean", FilesystemMcpServer::tool_router().list_all()),
    ] {
        for t in &tools {
            let desc = t
                .description
                .as_deref()
                .unwrap_or_else(|| panic!("{surface}/{} must set a description", t.name))
                .to_string();
            out.push((surface, t.name.to_string(), desc));
        }
    }
    out
}

/// Like `schema_for`, but resolves against the named surface's router so
/// lean tools lint against the lean wire shape, not the full one.
fn schema_for_surface(surface: &str, tool_name: &str) -> String {
    use memstead_mcp::filesystem_server::FilesystemMcpServer;

    let tools = match surface {
        "full" => McpServer::tool_router().list_all(),
        "lean" => FilesystemMcpServer::tool_router().list_all(),
        other => panic!("unknown surface {other}"),
    };
    let tool = tools
        .iter()
        .find(|t| t.name == tool_name)
        .unwrap_or_else(|| panic!("{surface}/{tool_name} must exist"));
    serde_json::to_string(&tool.input_schema)
        .unwrap_or_else(|e| panic!("{surface}/{tool_name} input_schema must serialize: {e}"))
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
        "Record",
        "Unregister",
        "Reload",
        "Update",
    ];
    const BANNED_LEADS: &[&str] = &["This", "Allows", "A", "An", "The"];

    let mut violations = Vec::new();
    for (surface, name, desc) in descriptions() {
        let first = desc.split_whitespace().next().unwrap_or("");
        let first_word = first.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-');

        if BANNED_LEADS.contains(&first_word) {
            violations.push(format!(
                "{surface}/{name}: description starts with banned filler '{first_word}'"
            ));
            continue;
        }
        if !ALLOWED_LEADS.contains(&first_word) {
            violations.push(format!(
                "{surface}/{name}: description starts with '{first_word}' — not in curated verb allowlist {ALLOWED_LEADS:?}"
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
    for (surface, name, desc) in descriptions() {
        for marker in FORBIDDEN {
            if desc.contains(marker) {
                violations.push(format!(
                    "{surface}/{name}: description contains forbidden marker '{marker}'"
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
    for (surface, name, desc) in descriptions() {
        let words = desc.split_whitespace().count();
        if words < MIN_WORDS {
            violations.push(format!(
                "{surface}/{name}: {words} words < {MIN_WORDS} (too thin)"
            ));
        }
        if words > MAX_WORDS {
            violations.push(format!(
                "{surface}/{name}: {words} words > {MAX_WORDS} (too long)"
            ));
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
    for (surface, name, desc) in descriptions() {
        let bytes = desc.len();
        if bytes > MAX_BYTES {
            violations.push(format!(
                "{surface}/{name}: {bytes} bytes > {MAX_BYTES} (truncated in Claude Code)"
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
    for (surface, name, desc) in descriptions() {
        let schema = schema_for_surface(surface, &name);
        for token in extract_backtick_tokens(&desc) {
            if is_allowed_reference(&name, &token, &schema) {
                continue;
            }
            violations.push(format!(
                "{surface}/{name}: backtick reference `{token}` is neither an input param nor a documented response/generic term"
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
            // Skip file-path literals (`.memstead/changes.jsonl`) — a
            // slashed token whose final segment carries an extension is a
            // path, not an identifier or slashed alternative.
            if raw.contains('/')
                && raw
                    .rsplit('/')
                    .next()
                    .is_some_and(|last| last.contains('.'))
            {
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
    // Cross-tool references — a `memstead_`-prefixed token naming a live
    // tool on either server flavour is a valid sibling pointer, not drift.
    if normalised.starts_with("memstead_") {
        use memstead_mcp::filesystem_server::FilesystemMcpServer;
        let is_tool = McpServer::tool_router()
            .list_all()
            .iter()
            .chain(FilesystemMcpServer::tool_router().list_all().iter())
            .any(|t| t.name == normalised);
        if is_tool {
            return true;
        }
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
            "entity_type",
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
            // Direction-labelled relationship entries (cold-start 0-8-0,
            // F15): every entry carries `direction`; outgoing entries
            // hold the endpoint under `target`, incoming (opt-in via
            // include_relations) under `from`.
            "direction",
            "target",
            "from",
        ],
        "memstead_search" => &[
            // Expansion metadata: the traversal direction rides beside the
            // edge label so a `both` walk stays interpretable.
            "via_direction",
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
            // The mem-filter refusal (backlog-sweep plan 05): a filter
            // naming no visible mem refuses typed.
            "UNKNOWN_MEM",
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
            // The coverage rule (04/08): stamped by the composer.
            "_verdict_coverage",
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
            // Frontmatter slot: the serving engine's absolute workspace
            // path (CLI `--workspace` targeting for skills that shell out).
            "_workspace_root",
            // Lean-flavour overview: mount roster fields + the
            // frontmatter/error tokens its description names.
            "durable",
            "storage",
            "Origin",
            "first-party",
            "third-party",
            "_entity_count",
            "UNKNOWN_MEM",
            "_mem_schema",
        ],
        "memstead_schema" => &[
            // Response-shape fields shipped by build_schema_payload.
            // Full and lite ship the heavy arrays under distinct keys;
            // the description names all four so consumers decode by key
            // presence.
            // Lean-flavour additions: the canonical pin format literal
            // and the `community` schema block.
            "name@version",
            "community",
            "ref",
            "types",
            // Type-scoped serving (backlog-sweep plan 06a): the
            // unserved-types roster, the visible-degrade stamp/steer,
            // and the unknown-selection refusal code.
            "types_omitted",
            "_schema_mode",
            "_hint",
            "UNKNOWN_ENTITY_TYPE",
            "types_summary",
            "relationships_summary",
            "description",
            "when_to_use",
            "relationship_mode",
            "relationships",
            "used_by",
            "default_writing_guidance",
            "alias_target_rel_type",
            // Per-type outgoing-edge legality blocks (relationship
            // alternatives + cardinality), present at both verbosity
            // levels.
            "required_outgoing",
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
            // Lean-flavour refusal for params this surface doesn't honour.
            "UNSUPPORTED_PARAM",
            "details.params",
            "warnings",
            "write_id",
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
            "CROSS_SCHEMA_LINK_UNDECLARED",
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
            "INVALID_FIELD_VALUE",
            "SECTION_CONTENT_INVALID",
            "REQUIRED_FIELD_UNSET",
            "INVALID_REL_TYPE",
            "details.declared",
            "details.allowed",
            "details.field_description",
            "details.enum_values",
            "details.type_write_rules",
            "details.stubs",
            // Cross-gate pre-announcement block on MISSING_REQUIRED_SECTION
            // (backlog-sweep/09): still-unset required metadata rides the
            // section refusal so one retry clears both gates.
            "details.pre_announced",
            "details.pre_announced.required_field_unset.missing[]",
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
            // Lean-flavour refusal for params this surface doesn't honour.
            "UNSUPPORTED_PARAM",
            "details.params",
            // Recovery-payload home named in the fix-from-details pointer.
            "details",
            "prospective_hash",
            "_hash",
            "write_id",
            // Error-envelope code + details field referenced literally.
            "HASH_MISMATCH",
            "details.current",
            // Typed error codes + envelope fields.
            "UNKNOWN_SECTION",
            "UNKNOWN_METADATA_FIELD",
            "INVALID_ENUM_VALUE",
            "INVALID_FIELD_VALUE",
            "SECTION_CONTENT_INVALID",
            "REQUIRED_FIELD_UNSET",
            "details.declared",
            "details.allowed",
            "details.field_description",
            "details.enum_values",
            "details.type_write_rules",
            "details.stubs",
            // Cross-gate pre-announcement block on MISSING_REQUIRED_SECTION
            // (backlog-sweep/09): still-unset required metadata rides the
            // section refusal so one retry clears both gates.
            "details.pre_announced",
            "details.pre_announced.required_field_unset.missing[]",
            "suggestion",
            // Bug 4 (engine-bugs-from-planning-session.md): inline-wiki-link
            // auto-stub warning surfaces alongside the existing typed warnings.
            "INLINE_WIKI_LINK_AUTO_STUBBED",
            "CROSS_SCHEMA_LINK_UNDECLARED",
            // required_outgoing warning.
            "MISSING_REQUIRED_OUTGOING",
            "required_outgoing",
            "memstead_relate",
            // Bytes-identical no-op short-circuit — empty write_id,
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
            "write_id",
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
        "memstead_retype" => &[
            "old_type",
            "new_type",
            "_hash",
            "prospective_hash",
            "sections_renamed",
            "edges_rechecked",
            "write_id",
            "checks_stale",
            "staleness_note",
            "warnings",
            "details.target_sections",
            "details.target_catch_all",
            "details.proposed_section_map",
            "details.problems",
            "details.current",
            // Codes the report-all envelope carries, named literally.
            "UNKNOWN_SECTION",
            "MISSING_REQUIRED_SECTION",
            "UNKNOWN_METADATA_FIELD",
            "INVALID_ENUM_VALUE",
            "INVALID_FIELD_VALUE",
            "REQUIRED_FIELD_UNSET",
            "MISSING_REQUIRED_OUTGOING",
            "CONSTRAINT_UNSATISFIED",
            "INVALID_REL_SHAPE",
            "RETYPE_REFUSED",
            "RETYPE_REFERRER_UNPROBEABLE",
            "RETYPE_NO_OP",
            "HASH_MISMATCH",
            // Provenance kind the description names.
            "retype",
        ],
        "memstead_rename" => &[
            "old_id",
            "new_id",
            "write_id",
            "warnings",
            // Warning code referenced literally in the description.
            "TITLE_NORMALIZED_TO_SLUG_NOOP",
            // Title-grammar refusal + its recovery field, named by the
            // TITLE_GRAMMAR_RULE sentence the description carries.
            "INVALID_TITLE",
            "proposed_slug",
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
            // Lean-flavour refusal for the dry_run param this surface
            // doesn't honour (same posture as create / update).
            "UNSUPPORTED_PARAM",
            "details.params",
            // List-form envelope: refusal wrapper + per-entry fields.
            "BATCH_REFUSED",
            "details.entries",
            "errors_suppressed",
            "results",
            "action",
            "expected_hash",
            // Auto-stub + description-posture codes cited literally.
            "AUTO_STUB_CREATED",
            "DESCRIPTION_NOT_PERMITTED",
            "MISSING_REQUIRED_DESCRIPTION",
            "warnings",
            "write_id",
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
        "memstead_check" => &[
            // The open kind form and the finding refusal code, named
            // literally.
            "x-<name>",
            "INVALID_CHECK_FINDING",
            // Response field + derived-state vocabulary (agent-trust
            // plan 14).
            "check_state",
            "never_checked",
            "checked_ok",
            "check_failed",
            "check_stale",
            "_hash",
            "mutation_provenance",
            "memstead_entity",
            // Verdict vocabulary + refusal codes.
            "ok",
            "failed",
            "INVALID_VERDICT",
            "ENTITY_NOT_FOUND",
            "READ_ONLY_MOUNT",
            "CHECK_NOT_RECORDED",
            // The kind vocabulary and its refusal (kinded checks): a
            // conformance record is schema-bound and engine-stamped.
            "verification",
            "conformance",
            "INVALID_CHECK_KIND",
            "schema_ref",
        ],
        "memstead_health" => &[
            // The coverage rule (04/08): the axes the verdict answers
            // for, stamped into the payload.
            "verdict_coverage",
            // The folder-mem ledger axis and the warning that makes it
            // necessary (04/04). This surface serves folder mems, so it is
            // the one where both matter.
            "OUT_OF_BAND_EDITS_UNDETECTED",
            // Open-questions axis (include=open_questions) — agent-trust
            // plan 11's composed what-don't-we-know worklist; `more` is
            // its explicit-truncation field.
            "open_questions",
            "more",
            // Derivation-staleness axis (include=stale_derivations) —
            // agent-trust plan 12.
            "stale_derivations",
            "unbaselined",
            // Checks axis (include=checks) — agent-trust plan 14's
            // derived check states + the independence gate.
            "checks",
            "self_checked",
            "confirmed_independent",
            "unconfirmable",
            // Friction-ledger axis (include=friction) — agent-trust
            // plan 08's refusal-ledger summary.
            "friction",
            // Standalone anchor-verification axis (include=anchors).
            "anchors",
            "resolves",
            "drifted",
            "recheck",
            "unresolvable",
            // The entity end of an anchor (consistency-sweep 03/02): a row
            // naming an entity the mem no longer holds, and the statement of
            // why that check could not run when it could not.
            "dangling",
            "entity_end_unreconciled",
            // The measured failure and the absent measurement, split
            // (consistency-sweep 03/05), and the statement every figure
            // travels with.
            "unobserved",
            "population",
            "fully_adjudicated",
            // The conformance axis's observation channel (04/01).
            "body_observations",
            // The folder-mem ledger-vs-files axis (04/04).
            "ledger",
            "memstead verify-anchors",
            "writable_mems",
            "default_writable_mem",
            "read_mems",
            "orphans",
            "stubs",
            "most_connected",
            "missing_fields",
            // Standing declared-constraint violations (include=constraints).
            "constraints",
            "severity",
            "stale",
            "warnings",
            "community_count",
            "mem_schemas",
            "dangling_links",
            "from",
            "target_id",
            "target_path",
            "section",
            // The discriminator on a `dangling_links` entry (04/06): which
            // of the three conditions this one is. `section` used to carry
            // that job implicitly, by being null.
            "kind",
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
            // The mem-scope refusal (backlog-sweep plan 06): the lean
            // flavour's description names the typed refusal for a `mem`
            // filter matching no visible mem.
            "UNKNOWN_MEM",
            "details",
            // Config projection: the `config` include key (catalogue
            // form of `include_config: true`) and the per-issue
            // `missing_fields` codes the description tells agents to
            // branch on.
            "config",
            "issues",
            "MISSING",
            "SECTION_HEADING_MISMATCH",
            // Integrity-linter surface: the `conformance` / `integrity` include
            // keys, the `findings` response array, its field names,
            // and the two consistency-axis codes.
            "conformance",
            "integrity",
            "findings",
            "axis",
            "code",
            "detail",
            "DANGLING_LINK_TARGET_MISSING",
            "DANGLING_LINK_NOT_RELATED",
            "DANGLING_RELATION_TARGET_MISSING",
            "ORPHAN_STUB",
            // An existing cross-mem edge the workspace grant table no longer
            // permits — a state the default-deny write gate would refuse to
            // create today, reported rather than refused at load (04/07).
            "CROSS_MEM_EDGE_UNGRANTED",
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
            // Standing declared-constraint violations (include=constraints).
            "constraints",
            "severity",
            // Aggregate signals (include=signals): the axis key and
            // the below-first-threshold level literal.
            "signals",
            "none",
            // Grounded labelling (include=labelling): the axis key.
            "labelling",
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
            "write_id",
            // The folder-mem cursor: each ledger entry carries `ts`
            // (RFC3339), and the last one read is the next `since`.
            "ts",
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
            // Lean-surface honour-or-refuse posture (backlog-sweep 09b):
            // the refusal code for an unknown mem, the lean notes[]
            // element fields, and the up-front rename_similarity refusal.
            "UNKNOWN_MEM",
            "notes[]",
            "timestamp",
            "kind",
            "entity_id",
            "note",
            "actor",
            "client",
            "sha",
            "subject",
            "UNSUPPORTED_PARAM",
            "details.params",
        ],
        "memstead_reload" => &[
            // Response-shape fields surfaced by the per-mem `ReloadReport`.
            "reports",
            "head_before",
            "head_after",
            "entities_loaded",
            "changed_entity_ids",
            // Full-refresh mode (plan 12): the additive re-scan block on
            // the response and its fields.
            "refresh",
            "schemas_added",
            "schema_removals_skipped",
            "mems_mounted",
            "mems_unmounted",
            "mems_quarantined",
            "failures",
            "elapsed_ms",
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
            "seed_write_id",
            "write_id",
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
        "memstead_mem_configure" => &[
            // A config-only write advances neither the entity head nor the
            // change log, so MEM_RELOADED cannot see it; the config writer
            // detects the intervention itself and reports it here (04/03).
            "CONFIG_WRITE_INTERVENED",
            "details",
            // Response-shape fields.
            "mem",
            "warnings",
            // Subject block fields the description names.
            "scope",
            "method",
            "exclusions",
            // Error / warning codes named literally.
            "INVALID_INPUT",
            "UNKNOWN_MEM",
            "READ_ONLY_MOUNT",
            "MEM_RELOADED",
            // CLI sibling verbs cited for storage parity.
            "mem set-title",
            "set-description",
            "set-subject",
            // Allowlist token (description disclaims the gate).
            "mem_management",
        ],
        "memstead_mem_set_version" => &[
            // A config-only write advances neither the entity head nor the
            // change log, so MEM_RELOADED cannot see it; the config writer
            // detects the intervention itself and reports it here (04/03).
            "CONFIG_WRITE_INTERVENED",
            "details",
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
        _ => &[],
    }
}

/// Every allowlisted response-shape token must exist in the emitting
/// source. Adding a token to `response_shape_refs` used to be
/// indistinguishable from shipping the field: 04/01 advertised
/// `body_observations` on `memstead_health`, regenerated the docs, and
/// this suite stayed green while no server emitted the key. This gate
/// holds each token to existence as a bounded identifier or literal in
/// the source of the crates that compose responses (`memstead-mcp` plus
/// the engine-side crates, and `memstead-cli` for the CLI commands
/// descriptions point at). A token no source mentions is a never-shipped
/// field or a stale allowlist entry, and both are red.
///
/// This is an existence check, not a per-tool emission proof: a key
/// emitted only by a different tool still passes. The stronger gate —
/// exercising every tool and requiring each advertised key in an actual
/// response — needs a response-coverage harness and stays a backlog item.
#[test]
fn every_response_shape_ref_exists_in_emitting_source() {
    fn collect_rs(dir: &std::path::Path, corpus: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, corpus);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                corpus.push_str(&text);
                corpus.push('\n');
            }
        }
    }

    /// The needle appears with non-identifier characters (or the text
    /// boundary) on both sides — so `hash` inside `expected_hash` does
    /// not count, while `"first-party"` inside a string literal does.
    fn appears_bounded(corpus: &str, needle: &str) -> bool {
        let bytes = corpus.as_bytes();
        let is_ident = |b: u8| b == b'_' || b.is_ascii_alphanumeric();
        let mut start = 0;
        while let Some(pos) = corpus[start..].find(needle) {
            let abs = start + pos;
            let end = abs + needle.len();
            let before_ok = abs == 0 || !is_ident(bytes[abs - 1]);
            let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
            if before_ok && after_ok {
                return true;
            }
            start = end;
        }
        false
    }

    let crates_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("memstead-mcp sits under crates/");
    let mut corpus = String::new();
    for krate in [
        "memstead-mcp",
        "memstead-engine",
        "memstead-base",
        "memstead-git-branch",
        "memstead-schema",
        "memstead-cli",
    ] {
        collect_rs(&crates_root.join(krate).join("src"), &mut corpus);
    }
    assert!(
        !corpus.is_empty(),
        "no emitting source collected under {}",
        crates_root.display()
    );

    let mut untraceable = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for tool in McpServer::tool_router().list_all() {
        for token in response_shape_refs(&tool.name) {
            // Normalise the way `is_allowed_reference` reads tokens: a
            // dotted path or a spaced command is checked by its most
            // specific segment; `[]` and `/` decorations are stripped.
            let tail = token
                .split_whitespace()
                .last()
                .unwrap_or(token)
                .split('.')
                .next_back()
                .unwrap_or(token)
                .trim_end_matches("[]")
                .trim_end_matches('/');
            if tail.is_empty() || !seen.insert(tail.to_string()) {
                continue;
            }
            if !appears_bounded(&corpus, tail) {
                untraceable.push(format!("{}: `{token}` (checked as `{tail}`)", tool.name));
            }
        }
    }
    assert!(
        untraceable.is_empty(),
        "response_shape_refs tokens with no trace in any emitting crate's source — \
         either the field never shipped or the allowlist entry is stale:\n  {}",
        untraceable.join("\n  ")
    );
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
/// The relocated prose files must not grow a stray trailing newline.
///
/// Tool descriptions live in `crates/memstead-mcp/descriptions/**` and end
/// with a newline like any text file; `descriptions::text` strips exactly
/// that one. The two instruction halves cannot be trimmed, because `concat!`
/// runs at compile time, so their files deliberately end without a newline.
/// An editor "fixing" that would silently change the served bytes, and both
/// strings are single-line by construction, so a newline anywhere in either
/// is the signal.
#[test]
fn relocated_prose_carries_no_stray_newline() {
    for (surface, text) in [
        ("full", memstead_mcp::server::SERVER_INSTRUCTIONS),
        (
            "filesystem",
            memstead_mcp::filesystem_server::FS_SERVER_INSTRUCTIONS,
        ),
    ] {
        assert!(
            !text.contains('\n'),
            "{surface} server instructions contain a newline — a descriptions/**/server-instructions-*.md file grew a trailing newline, which changes the served bytes"
        );
    }
    for (surface, tools) in [
        ("full", McpServer::tool_router().list_all()),
        (
            "filesystem",
            memstead_mcp::filesystem_server::FilesystemMcpServer::tool_router().list_all(),
        ),
    ] {
        for tool in &tools {
            let d = tool.description.as_deref().unwrap_or_default();
            assert!(
                !d.is_empty(),
                "{surface}/{}: served an empty description — its descriptions/**/*.md file is missing or blank",
                tool.name
            );
            assert_eq!(
                d.trim_end(),
                d,
                "{surface}/{}: description ends in whitespace — its file grew a second trailing newline or trailing spaces",
                tool.name
            );
        }
    }
}

const STRUCTURED_ERROR_CODES: &[&str] = &[
    // Lookup
    "ENTITY_NOT_FOUND",
    "ENTITY_ALREADY_EXISTS",
    "UNKNOWN_MEM",
    // Check kinds
    "INVALID_CHECK_KIND",
    // Optimistic locking / structural
    "HASH_MISMATCH",
    "RELATIONSHIP_CYCLE",
    // Schema vocabulary violations
    "UNKNOWN_SECTION",
    "UNKNOWN_METADATA_FIELD",
    "UNKNOWN_ENTITY_TYPE",
    "INVALID_ENUM_VALUE",
    "INVALID_FIELD_VALUE",
    "SECTION_CONTENT_INVALID",
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
    "MEM_NAME_COLLISION",
    "MEM_PATH_NOT_ALLOWED",
    "MEM_SCHEMA_NOT_ALLOWED",
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
    "INTERNAL_IO_ERROR",
    "CONFIG_ERROR",
    // MCP filter (workspace-level `[mcp].disabled_tools`)
    "TOOL_DISABLED",
    // memstead_changes_since cursor resolution
    "INVALID_CURSOR",
    // E3a anchors: malformed anchors[] element on create/update
    "INVALID_ANCHOR",
];

/// Joins every tool description and the server-level `instructions` into a
/// single haystack. The drift guards assert "this code is named *somewhere*
/// on the agent-facing surface" — the load-bearing substring tests below
/// lock which exact tool must carry each clause.
fn all_description_text() -> String {
    let mut acc = String::new();
    for (_, _, desc) in descriptions() {
        acc.push_str(&desc);
        acc.push('\n');
    }
    acc.push_str(server_instructions_text());
    acc
}

fn server_instructions_text() -> &'static str {
    // The live const the handler serves — no duplicated copy to drift
    // (the historical SERVER_INSTRUCTIONS_COPY + its extraction-based
    // match test were replaced by this direct read as a deliberate
    // act, agent-trust plan 05).
    memstead_mcp::server::SERVER_INSTRUCTIONS
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
/// A mutation description that names `write_id` must not gloss it as
/// git, must not point at a gitdir, and must not invite a caller to use
/// it as a change cursor.
///
/// This test replaces `every_mutation_description_clarifies_commit_sha_origin`,
/// which asserted the OPPOSITE: it required every mention to carry the
/// "per-mem git" qualifier and the `memstead_health include_config=true`
/// gitdir pointer. That qualifier was false on a folder or in-memory mem
/// (the gitdir lookup errors for that storage kind and the health
/// projection omits the field), and the cursor advice was worse than
/// false — the token sorts below every folder-ledger timestamp, so a
/// caller who followed it silently received the whole history instead of
/// a delta. What the old test pinned was the defect, so it is inverted
/// rather than deleted: the banned-phrase list is the shape of the bug.
///
/// The positive half stays in the shared server `instructions`, which
/// state the token's origin per backend and its non-cursor status once
/// for every mutation (the truncation-ceiling rule keeps shared contract
/// out of per-tool descriptions).
#[test]
fn no_mutation_description_glosses_write_id_as_git_or_cursor() {
    const MUTATION_TOOLS: &[&str] = &[
        "memstead_create",
        "memstead_update",
        "memstead_delete",
        "memstead_rename",
        "memstead_retype",
        "memstead_relate",
        // Returns `seed_write_id`, so it carries the same claim and
        // was outside this list until 2026-08-27 — correct only by
        // luck rather than by check.
        "memstead_mem_create",
        // Documents the cursor the token is not.
        "memstead_changes_since",
    ];
    // Structural, not a banned-phrase list. An earlier version banned
    // "commit SHA" outright and would have rejected the CORRECT wording
    // ("a commit SHA on a git-branch mem, a synthetic token on a folder
    // mem"), while a list built for one phrasing lets the next one
    // through — `ops/mod.rs` learned that when "Per-mem commit
    // identifier" walked past a list written for "Per-mem commit SHA".
    // So: a sentence naming the token and calling it a commit must also
    // name WHICH backend produces one, and no sentence naming the token
    // may invite polling with it.
    const CURSOR_INVITES: &[&str] = &[
        "polling",
        "poll via",
        "since cursor",
        "as the `since`",
        "for memstead_changes_since",
    ];
    let mut violations = Vec::new();
    for (surface, name, desc) in descriptions() {
        if !MUTATION_TOOLS.contains(&name.as_str()) {
            continue;
        }
        if !desc.contains("write_id") {
            continue; // tool doesn't mention it — not a violation
        }
        // Judge only the sentences that talk about the token. A
        // description may legitimately mention a gitdir about something
        // else — `memstead_mem_create` bootstraps one — and scanning the
        // whole blob would force an allowlist, which is where the next
        // drift would hide.
        // Each token-sentence is judged on its own: joining them lets a
        // correct sentence excuse a wrong one elsewhere in the same
        // description.
        for sentence in desc.split(". ").filter(|s| s.contains("write_id")) {
            let lower = sentence.to_lowercase();
            if (lower.contains("commit") || lower.contains("sha")) && !lower.contains("git-branch")
            {
                violations.push(format!(
                    "{surface}/{name}: calls `write_id` a commit without naming which \
                     backend produces one — {sentence}"
                ));
            }
            for phrase in CURSOR_INVITES {
                if lower.contains(phrase) {
                    violations.push(format!(
                        "{surface}/{name}: invites polling with `write_id` (\"{phrase}\") — \
                         it is an identity, not a change cursor"
                    ));
                }
            }
            if lower.contains("gitdir") || lower.contains("include_config") {
                violations.push(format!(
                    "{surface}/{name}: points at a gitdir in a sentence about `write_id` — \
                     the lookup errors on a backend without one"
                ));
            }
        }
    }
    // Vacuity guards. The round-six rewrite of this check dropped the
    // two positive assertions the earlier version carried, so it would
    // have gone green in silence if every mutation description simply
    // stopped naming the token — the loop `continue`s on any tool that
    // does not mention it. The contract has to be asserted present, not
    // merely un-violated.
    let instr = server_instructions_text();
    assert!(
        instr.contains("write_id"),
        "server instructions must name the token every mutation returns"
    );
    assert!(
        instr.contains("NOT a change cursor"),
        "server instructions must state that write_id is not a change cursor"
    );
    assert!(
        descriptions()
            .iter()
            .any(|(_, name, desc)| MUTATION_TOOLS.contains(&name.as_str())
                && desc.contains("write_id")),
        "no mutation description names `write_id` — this check has gone vacuous"
    );
    assert!(
        violations.is_empty(),
        "write_id gloss violations:\n  {}",
        violations.join("\n  ")
    );
}

/// The filesystem flavour has no commits — its substrate is the
/// change ledger — yet its descriptions inherited sentences from the
/// mem-repo flavour that assert one happens ("rewritten in one per-mem
/// commit"). The four write_id guards all key on the token and are
/// blind to a bare commit claim, which is how that sentence survived
/// the 2026-08 rename sweep. Structural rule, not a banned phrase: a
/// lean-flavour description sentence naming a commit must either negate
/// it (no / not / never / none) or explicitly speak about the mem-repo
/// flavour as its subject.
#[test]
fn no_filesystem_description_asserts_a_commit_happens() {
    let mut violations = Vec::new();
    let mut sentences_judged = 0usize;
    for (surface, name, desc) in descriptions() {
        if surface != "lean" {
            continue;
        }
        for sentence in desc.split(". ") {
            let lower = sentence.to_lowercase();
            if !lower.contains("commit") {
                continue;
            }
            sentences_judged += 1;
            let negated = [
                "no commit",
                "no per-mem commit",
                "not a commit",
                "there are no commits",
                "no commit history",
                "never commit",
                "no commits",
                "not commits",
            ]
            .iter()
            .any(|n| lower.contains(n));
            let other_flavour = lower.contains("mem-repo");
            if !negated && !other_flavour {
                violations.push(format!(
                    "lean/{name}: asserts a commit on a substrate that has none — {sentence}"
                ));
            }
        }
    }
    assert!(
        sentences_judged > 0,
        "no filesystem description mentions commits at all — this check has gone vacuous"
    );
    assert!(
        violations.is_empty(),
        "commit-claim violations on the commit-less flavour:\n  {}",
        violations.join("\n  ")
    );
}

/// Helper — return the FULL-surface description for one tool. Panics if
/// the tool is absent (indicates the surface itself has drifted, which
/// other tests already catch). The load-bearing substring tests below
/// lock the full server's contract; the lean flavour is covered by the
/// generic lints, not these per-clause pins.
fn description_of(tool_name: &str) -> String {
    descriptions()
        .into_iter()
        .find(|(surface, n, _)| *surface == "full" && n == tool_name)
        .unwrap_or_else(|| panic!("{tool_name} must exist"))
        .2
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

/// The reserved identity triple (`mem`/`id`/`type`) is set-refused but
/// unset-ALLOWED (the sanctioned repair for a historically smuggled
/// key). The tool description must state that asymmetry — a text that
/// documents unset as refused sends agents to delete-and-recreate,
/// destroying provenance for nothing.
#[test]
fn memstead_update_description_states_reserved_unset_asymmetry() {
    let desc = description_of("memstead_update");
    assert!(
        desc.contains("Read-only on SET"),
        "memstead_update must scope the reserved-triple refusal to SET."
    );
    assert!(
        desc.contains("sanctioned repair"),
        "memstead_update must document reserved-key unset as the sanctioned repair."
    );
    assert!(
        !desc.contains("set/unset"),
        "the retired set-and-unset-refused wording must not resurface."
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
        desc.contains("write_id") && desc.contains("empty"),
        "memstead_relate must document the empty-`write_id` no-op convention \
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
    tools.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let mut total_bytes = 0usize;
    let mut total_words = 0usize;
    println!("\n{:<28} {:>6} {:>6}", "tool", "words", "bytes");
    println!("{}", "-".repeat(46));
    for (surface, name, desc) in &tools {
        let words = desc.split_whitespace().count();
        let bytes = desc.len();
        total_bytes += bytes;
        total_words += words;
        println!(
            "{:<28} {:>6} {:>6}",
            format!("{surface}/{name}"),
            words,
            bytes
        );
    }
    println!("{}", "-".repeat(40));
    println!(
        "{:<22} {:>6} {:>6}",
        "TOOLS_SUBTOTAL", total_words, total_bytes
    );
    let instr = server_instructions_text();
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

/// The title-grammar rule in the `memstead_create` / `memstead_rename`
/// descriptions is the validator's own sentence — asserted verbatim
/// against `memstead_base::TITLE_GRAMMAR_RULE`, whose conformance test
/// in `memstead-base` binds the sentence to `validate_and_derive_slug`
/// behaviour. Derived, not transcribed: neither surface can drift from
/// the accept set alone.
#[test]
fn title_taking_descriptions_carry_the_validator_grammar_rule() {
    let mut missing = Vec::new();
    for (surface, name, desc) in descriptions() {
        if !(name == "memstead_create" || name == "memstead_rename") {
            continue;
        }
        if !desc.contains(memstead_base::TITLE_GRAMMAR_RULE) {
            missing.push(format!("{surface}/{name}"));
        }
    }
    assert!(
        missing.is_empty(),
        "descriptions missing the verbatim TITLE_GRAMMAR_RULE sentence: {missing:?}"
    );
}

// ==========================================================================
// Surface honesty (agent-trust plan 05): the instructions tell the whole
// truth about the surface — complete roster, real version, bounded length.
// ==========================================================================

/// Extract every `memstead_*` token from an instruction string. The
/// instructions may mention a tool any number of times; the SET of
/// mentioned tool-shaped tokens must equal the registered set.
fn mentioned_tools(text: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut rest = text;
    while let Some(pos) = rest.find("memstead_") {
        let start = &rest[pos..];
        let len = start
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_'))
            .map(|(i, _)| i)
            .unwrap_or(start.len());
        out.insert(start[..len].to_string());
        rest = &start[len..];
    }
    out
}

fn registered_tools(tools: &[rmcp::model::Tool]) -> std::collections::BTreeSet<String> {
    tools.iter().map(|t| t.name.to_string()).collect()
}

/// Criterion 1 (full flavour): the instruction text can neither lag nor
/// lead the registry — every registered tool is named, and every
/// `memstead_*` token in the text names a registered tool.
#[test]
fn full_instructions_roster_matches_registry_bidirectionally() {
    let registered = registered_tools(&McpServer::tool_router().list_all());
    let mentioned = mentioned_tools(memstead_mcp::server::SERVER_INSTRUCTIONS);
    let absent: Vec<_> = registered.difference(&mentioned).collect();
    assert!(
        absent.is_empty(),
        "registered tools missing from the instructions roster: {absent:?}"
    );
    let phantom: Vec<_> = mentioned.difference(&registered).collect();
    assert!(
        phantom.is_empty(),
        "instructions name tools that are not registered: {phantom:?}"
    );
}

/// Criterion 1 (lean flavour): same bidirectional contract for the
/// filesystem server's instructions and its own tool set.
#[test]
fn lean_instructions_roster_matches_registry_bidirectionally() {
    let registered = registered_tools(
        &memstead_mcp::filesystem_server::FilesystemMcpServer::tool_router().list_all(),
    );
    let mentioned = mentioned_tools(memstead_mcp::filesystem_server::FS_SERVER_INSTRUCTIONS);
    let absent: Vec<_> = registered.difference(&mentioned).collect();
    assert!(
        absent.is_empty(),
        "registered tools missing from the lean instructions roster: {absent:?}"
    );
    let phantom: Vec<_> = mentioned.difference(&registered).collect();
    assert!(
        phantom.is_empty(),
        "lean instructions name tools that are not registered: {phantom:?}"
    );
}

/// Criterion 2: both flavours' instructions carry the crate version and
/// the CLI-companion note naming the batch, export, and distribution
/// families. (Integration tests compile inside the memstead-mcp
/// package, so `CARGO_PKG_VERSION` here IS the crate version.)
#[test]
fn instructions_carry_crate_version_and_cli_companion_note() {
    for (label, text) in [
        ("full", memstead_mcp::server::SERVER_INSTRUCTIONS),
        (
            "lean",
            memstead_mcp::filesystem_server::FS_SERVER_INSTRUCTIONS,
        ),
    ] {
        assert!(
            text.contains(concat!("Engine version: ", env!("CARGO_PKG_VERSION"))),
            "{label}: instructions must carry the crate version"
        );
        for family in [
            "batch-create",
            "batch-update",
            "batch-relate",
            "export",
            "publish",
        ] {
            assert!(
                text.contains(family),
                "{label}: CLI-companion note must name the `{family}` family"
            );
        }
    }
}

/// Criterion 2: the MCP serverInfo version equals the engine's FULL
/// build version (crate semver plus git build sha for dev builds) on
/// both flavours — the historical hardcoded `"0.1.0"` cannot recur,
/// and two dev builds between releases stay distinguishable.
/// Asserted against the LIVE `get_info()` of constructed servers.
/// The full flavour's served instructions keep the compile-time
/// const verbatim as their prefix and append a runtime `Build:`
/// sentence exactly when a build sha exists.
#[test]
fn server_info_version_equals_full_build_version_on_both_flavours() {
    use rmcp::ServerHandler as _;
    let full_version = memstead_base::build_info::full_version();
    let lean_engine = memstead_base::Engine::from_mounts(Vec::new()).unwrap();
    let lean = memstead_mcp::filesystem_server::FilesystemMcpServer::from_engine(
        lean_engine,
        std::path::PathBuf::from("."),
    );
    let info = lean.get_info();
    assert_eq!(info.server_info.version, full_version);
    assert_eq!(
        info.instructions.as_deref(),
        Some(memstead_mcp::filesystem_server::FS_SERVER_INSTRUCTIONS),
    );

    let full_engine = memstead_base::Engine::from_mounts(Vec::new()).unwrap();
    let full = memstead_mcp::server::McpServer::new_with_config(
        full_engine,
        25_000,
        std::collections::HashSet::new(),
        None,
        Default::default(),
        Default::default(),
    );
    let info = full.get_info();
    assert_eq!(info.server_info.version, full_version);
    let served = info.instructions.as_deref().unwrap();
    assert!(
        served.starts_with(memstead_mcp::server::SERVER_INSTRUCTIONS),
        "served instructions keep the const as their verbatim prefix"
    );
    if memstead_base::build_info::BUILD_SHA.is_empty() {
        assert_eq!(served, memstead_mcp::server::SERVER_INSTRUCTIONS);
    } else {
        assert_eq!(
            served,
            format!(
                "{} Build: {full_version}.",
                memstead_mcp::server::SERVER_INSTRUCTIONS
            )
        );
    }
}

/// Criterion 4: instruction length stays within a stated budget — a
/// tripwire against unbounded growth, not a magic number. Budgets set
/// at plan-05 landing: full ~10.1kB current + ~24% headroom; lean
/// ~2.0kB current with a roomier 4kB ceiling (the lean surface is
/// small enough that a doubling is the signal worth tripping on).
/// Raised consciously on 2026-09-02 (12.5kB → 12.8kB) for the
/// `MEM_ROSTER_CHANGED` membership marker, the sibling of
/// `MEM_RELOADED`: a protocol every consumer must know, not decoration.
#[test]
fn instruction_length_stays_within_budget() {
    let full_len = memstead_mcp::server::SERVER_INSTRUCTIONS.len();
    assert!(
        full_len <= 12_800,
        "full instructions grew past the 12.8kB tripwire ({full_len} bytes) — trim \
         (the error-code list is the sanctioned cut) or consciously raise the budget"
    );
    let lean_len = memstead_mcp::filesystem_server::FS_SERVER_INSTRUCTIONS.len();
    assert!(
        lean_len <= 4_000,
        "lean instructions grew past the 4kB tripwire ({lean_len} bytes) — trim or \
         consciously raise the budget"
    );
}
