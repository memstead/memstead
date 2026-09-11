//! The MCP server's unit tests, moved out of `server.rs` on 2026-09-11 so
//! the handlers read without thirteen thousand lines of tests beneath
//! them and the engine mem's file-grain anchor on `server.rs` stops
//! drifting on every test edit. A child module of `server`, so `use
//! super::*` reaches the same private items it always did; the file
//! gates itself with the inner `cfg(test)` so the parent carries no test
//! attribute at all.
#![cfg(test)]

use super::*;
use crate::tools::mutation::{PatchInput, RelateOpInput};
use indexmap::IndexMap;
use memstead_base::Query;
use std::fs;
use tempfile::TempDir;

/// Build the workspace directory layout used by every test in this
/// module: a `specs` mem with two seed entities and an
/// auto-seeded `mem-repo/.git/` so the git-branch backend factory
/// has a real gitdir to point at.
fn setup_test_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    // Dir basename must equal the declared `name` per the
    // basename-invariant. Router key matches too for consistency.
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();

    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    fs::write(
            mem_dir.join("entity-a.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\ntags: backend\n---\n# Entity A\n\n## Identity\n\nFirst test entity.\n\n## Purpose\n\nTesting the MCP server.\n\n## Relationships\n\n- **USES**: [[entity-b]]\n",
        )
        .unwrap();

    fs::write(
            mem_dir.join("entity-b.md"),
            "---\ntype: spec\ncreated_date: 2026-02-01\nlast_modified: 2026-04-12\nlevel: M1\ntags: frontend\n---\n# Entity B\n\n## Identity\n\nSecond test entity.\n\n## Purpose\n\nDependency of Entity A.\n",
        )
        .unwrap();

    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());

    tmp
}

/// Build a default `McpServer` over the standard test workspace.
/// Tests that want a customised engine (different settings, different
/// backend variant) call `setup_test_workspace`, build the engine
/// themselves, then `McpServer::new(engine, BUDGET)` directly.
fn setup_test_engine() -> (McpServer, TempDir) {
    let tmp = setup_test_workspace();
    let engine = setup_unified_test_engine(tmp.path());
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);
    (server, tmp)
}

/// Build a unified `memstead_base::Engine` from any disk-shaped mem
/// directories under `workspace_root` — every subdir carrying
/// `.memstead/config.json` becomes a folder-backend `Mount`.
fn setup_unified_test_engine(workspace_root: &std::path::Path) -> memstead_base::Engine {
    use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
    let mut mounts: Vec<(Mount, Box<dyn memstead_base::backend::MemBackend>)> = Vec::new();
    for entry in std::fs::read_dir(workspace_root).unwrap().flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        // Skip the auto-seeded mem-repo gitdir — folder mounts
        // only.
        if p.file_name().and_then(|s| s.to_str()) == Some("mem-repo") {
            continue;
        }
        let cfg = p.join(".memstead").join("config.json");
        if !cfg.is_file() {
            continue;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap().to_string();
        let cfg_bytes = std::fs::read(&cfg).unwrap();
        let cfg_val: serde_json::Value =
            serde_json::from_slice(&cfg_bytes).unwrap_or(serde_json::Value::Null);
        let schema_str = cfg_val
            .get("schema")
            .and_then(|v| v.as_str())
            .unwrap_or("default@1.0.0")
            .to_string();
        let schema = Some(schema_str.parse().unwrap());
        let mount = Mount {
            mem: name,
            schema,
            storage: MountStorage::Folder { path: p.clone() },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend = memstead_base::instantiate_local_backend(&mount).unwrap();
        mounts.push((mount, backend));
    }
    let mut engine = memstead_base::Engine::from_mounts(mounts).unwrap();
    // Install the full backend factory so
    // `mem_management::create_mem` can materialise
    // git-branch backends when its workspace-shape heuristic
    // fires. Test fixtures auto-seed a mem-repo
    // (`auto_seeded_settings`) so the heuristic always picks
    // `MountStorage::GitBranch` for runtime-created mems — the
    // default local factory would reject with
    // `GitBranchBackendUnavailable`.
    engine.set_backend_factory(memstead_git_branch::storage::instantiate_full_backend);
    engine
}

/// Build a unified `memstead_base::Engine` whose mounts use the
/// git-branch backend rather than the folder backend.
///
/// Test fixtures that call `auto_seeded_settings(tmp.path())`
/// produce a `<tmp>/mem-repo/.git/` with a branch
/// `refs/heads/<mem>` per disk mem. Mutations through this
/// engine variant land as real commits on the mem-repo
/// gitdir, producing 40-char hex `write_id` /
/// `seed_write_id` values that lifecycle / commit-body tests
/// assert on.
fn setup_unified_test_engine_git_branch(workspace_root: &std::path::Path) -> memstead_base::Engine {
    use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
    // Canonicalise the gitdir so `engine.gitdir_for(name)` matches
    // full's canonical paths (TempDir on macOS returns a symlink
    // to /private/var/...; full canonicalizes at init).
    let gitdir = workspace_root.join("mem-repo").join(".git");
    let gitdir = gitdir.canonicalize().unwrap_or(gitdir);
    let mut mounts: Vec<(Mount, Box<dyn memstead_base::backend::MemBackend>)> = Vec::new();
    for entry in std::fs::read_dir(workspace_root).unwrap().flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if p.file_name().and_then(|s| s.to_str()) == Some("mem-repo") {
            continue;
        }
        let cfg = p.join(".memstead").join("config.json");
        if !cfg.is_file() {
            continue;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap().to_string();
        let cfg_bytes = std::fs::read(&cfg).unwrap();
        let cfg_val: serde_json::Value =
            serde_json::from_slice(&cfg_bytes).unwrap_or(serde_json::Value::Null);
        let schema_str = cfg_val
            .get("schema")
            .and_then(|v| v.as_str())
            .unwrap_or("default@1.0.0")
            .to_string();
        let schema = Some(schema_str.parse().unwrap());
        let mount = Mount {
            migration_target: None,
            mem: name.clone(),
            schema,
            storage: MountStorage::GitBranch {
                gitdir: gitdir.clone(),
                branch: format!("refs/heads/{name}"),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
        };
        let backend = memstead_git_branch::storage::instantiate_full_backend(&mount).unwrap();
        mounts.push((mount, backend));
    }
    let mut engine = memstead_base::Engine::from_mounts(mounts).unwrap();
    // Install the full backend factory so create_mem's
    // git-branch path can materialise a backend when the
    // workspace-shape heuristic fires.
    engine.set_backend_factory(memstead_git_branch::storage::instantiate_full_backend);
    engine
}

/// Alias for [`setup_test_engine`]. Kept for callsite compatibility;
/// pre-rebuild this materialised an additional full engine alongside
/// the unified one.
fn setup_dual_test_engine() -> (McpServer, TempDir) {
    setup_test_engine()
}

/// Extract text from a CallToolResult.
fn extract_text(result: &CallToolResult) -> String {
    result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

// ------------------------------------------------------------------
// Test-only overview Markdown introspection. Parses the handful of
// facts (frontmatter + headings) out of the Markdown body so
// existing assertions translate 1:1. Intentionally shallow: we do
// not try to round-trip every field, just the ones tests actually
// assert on.
// ------------------------------------------------------------------

/// Thin parsed view over the overview markdown body.
struct ParsedOverview {
    text: String,
    frontmatter: std::collections::HashMap<String, String>,
}

impl ParsedOverview {
    fn from(result: &CallToolResult) -> Self {
        let text = extract_text(result);
        let mut frontmatter = std::collections::HashMap::new();
        // The core's split, never a second reader of the fence.
        if let Some((meta, _)) = memstead_base::frontmatter_parts(&text) {
            for line in meta.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    frontmatter.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        Self { text, frontmatter }
    }

    fn overview_mode(&self) -> &str {
        self.frontmatter
            .get("_overview_mode")
            .map(String::as_str)
            .unwrap_or("")
    }

    fn budget_used(&self) -> u64 {
        self.frontmatter
            .get("_budget_used")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    fn budget_requested(&self) -> u64 {
        self.frontmatter
            .get("_budget_requested")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// Schema refs from the `## Schemas` block — each `### <ref>` heading.
    fn schema_refs(&self) -> Vec<String> {
        self.section_h3("## Schemas", "## ")
            .iter()
            .map(|l| l.trim_start_matches("### ").trim().to_string())
            .collect()
    }

    /// Mem names from the `## Mems` block — each `### <name>` heading.
    fn mem_names(&self) -> Vec<String> {
        self.section_h3("## Mems", "## ")
            .iter()
            .map(|l| l.trim_start_matches("### ").trim().to_string())
            .collect()
    }

    /// Hint keys from the `## Hints` block — each `- `<key>` — ...` line.
    fn hint_keys(&self) -> Vec<String> {
        let block = self.section_lines("## Hints", "## ");
        let mut keys = Vec::new();
        for line in block {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("- `")
                && let Some((key, _)) = rest.split_once('`')
            {
                keys.push(key.to_string());
            }
        }
        keys
    }

    /// Warning codes from the `## Warnings` block — each
    /// `- **CODE** — message` line.
    fn warning_codes(&self) -> Vec<String> {
        let block = self.section_lines("## Warnings", "## ");
        let mut codes = Vec::new();
        for line in block {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("- **")
                && let Some((code, _)) = rest.split_once("**")
            {
                codes.push(code.to_string());
            }
        }
        codes
    }

    /// Community-bridges heading block — each `### from ↔ to (N edges)`.
    fn bridge_headings(&self) -> Vec<String> {
        self.section_h3("## Community Bridges", "## ")
            .iter()
            .map(|s| s.trim_start_matches("### ").to_string())
            .collect()
    }

    fn section_lines(&self, start: &str, end_prefix: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut in_block = false;
        for line in self.text.lines() {
            if line.starts_with(start) {
                in_block = true;
                continue;
            }
            if in_block && line.starts_with(end_prefix) && !line.starts_with("### ") {
                break;
            }
            if in_block {
                out.push(line);
            }
        }
        out
    }

    fn section_h3(&self, start: &str, end_prefix: &str) -> Vec<&str> {
        self.section_lines(start, end_prefix)
            .into_iter()
            .filter(|l| l.starts_with("### "))
            .collect()
    }
}

// ------------------------------------------------------------------
// Regression guards for the read/error contract. The contract for
// `memstead_entity` + `memstead_search` is:
//   entity/search success = Markdown on text + structured envelope
//                            on `structured_content`
//   other read success    = pure Markdown on the text channel
//   error / warning       = `structured_content` with `{code,
//                            message, details}`
// ------------------------------------------------------------------

#[test]
fn entity_and_search_emit_structured_envelope_on_success() {
    let (server, _tmp) = setup_dual_test_engine();

    // memstead_entity — every combination must populate
    // structured_content with the typed Entity envelope.
    for (relations, context) in [(false, false), (true, false), (false, true), (true, true)] {
        if context {
            let _ = server.memstead_overview(Parameters(OverviewParams {
                rebuild: Some(true),
                chunk: None,
                mem: None,
                include: None,
                token_budget: None,
            }));
        }
        let r = server.memstead_entity(Parameters(EntityParams {
            id: "specs--entity-a".to_string(),
            include_relations: Some(relations),
            include_context: Some(context),
            sections: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        let sc = r.structured_content.as_ref().unwrap_or_else(|| {
                panic!(
                    "memstead_entity(relations={relations}, context={context}) must populate structured_content"
                )
            });
        assert!(
            sc.get("_hash").and_then(|v| v.as_str()).is_some(),
            "entity structured_content must carry `_hash`",
        );
        assert!(
            sc.get("sections").and_then(|v| v.as_object()).is_some(),
            "entity structured_content must carry `sections` object",
        );
    }

    // memstead_search — text query and filter-only must each carry
    // the structured `SearchResultEnvelope`.
    for params in [
        SearchParams {
            query: Some(Query {
                any: vec!["Entity".into()],
                ..Default::default()
            }),
            ..search_params_defaults()
        },
        SearchParams {
            query: None,
            entity_type: Some("spec".to_string()),
            ..search_params_defaults()
        },
    ] {
        let r = server.memstead_search(Parameters(params));
        let sc = r
            .structured_content
            .as_ref()
            .expect("memstead_search must populate structured_content");
        assert!(
            sc.get("_total").and_then(|v| v.as_u64()).is_some(),
            "search structured_content must carry `_total`",
        );
        assert!(
            sc.get("hits").and_then(|v| v.as_array()).is_some(),
            "search structured_content must carry `hits[]`",
        );
    }
}

#[test]
fn other_read_tools_emit_no_structured_content_on_success() {
    let (server, _tmp) = setup_dual_test_engine();

    // memstead_overview — default, with include, with mem filter.
    for (include, mem) in [
        (None, None),
        (Some(vec!["community_members".to_string()]), None),
        (None, Some("specs".to_string())),
    ] {
        let r = server.memstead_overview(Parameters(OverviewParams {
            rebuild: Some(true),
            chunk: None,
            mem,
            include,
            token_budget: None,
        }));
        assert!(
            r.structured_content.is_none(),
            "memstead_overview must not emit structured_content on success; got {:?}",
            r.structured_content
        );
    }
}

fn search_params_defaults() -> SearchParams {
    SearchParams {
        query: None,
        direction: None,
        mem: None,
        entity_type: None,
        expand_via: None,
        expand_depth: None,
        related_to: None,
        depth: None,
        edge_type: None,
        limit: None,
        offset: None,
        filters: None,
        range_filters: None,
        stub: None,
        token_budget: None,
    }
}

#[test]
fn error_envelopes_still_emit_structured_content() {
    // Exercises the unified dispatcher's error-envelope contract
    // (HASH_MISMATCH details.current and the
    // STUB_FILTER_EXCLUDES_ALL warning code).
    let (server, _tmp) = setup_dual_test_engine();

    // HASH_MISMATCH via memstead_update with a stale hash.
    let stale = server.memstead_update(Parameters(crate::tools::mutation::UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: "specs--entity-a".to_string(),
        expected_hash: Some(
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        ),
        sections: None,
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: Some(false),

        note: None,

        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(stale.is_error.unwrap_or(false), "stale hash must error");
    let sc = stale
        .structured_content
        .as_ref()
        .expect("HASH_MISMATCH must carry structured_content");
    assert_eq!(sc["code"], "HASH_MISMATCH");
    assert!(
        sc["details"]["current"].is_string(),
        "HASH_MISMATCH must carry details.current; got {sc:?}"
    );

    // STUB_FILTER_EXCLUDES_ALL via memstead_search with stub=true + entity_type.
    let stub_excl = server.memstead_search(Parameters(SearchParams {
        query: None,
        entity_type: Some("spec".to_string()),
        stub: Some(true),
        token_budget: None,
        ..search_params_defaults()
    }));
    // This path surfaces via a warnings entry in the list, not an error
    // envelope — post-removal, warnings live in the `## Filter warnings`
    // Markdown block for search. The test asserts that the stable code
    // is still named somewhere on the wire.
    let body = extract_text(&stub_excl);
    assert!(
        body.contains("STUB_FILTER_EXCLUDES_ALL"),
        "STUB_FILTER_EXCLUDES_ALL code must surface on the wire; got:\n{body}"
    );
}

/// #57: a health call whose rendered report exceeds `token_budget`
/// switches the text channel to chunkable markdown (with chunk-walk
/// frontmatter) while `structured_content` still ships whole.
#[test]
fn health_over_budget_text_is_chunked_markdown() {
    let (server, _tmp) = setup_dual_test_engine();
    // A tiny budget forces the markdown+chunk path even for a small graph.
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: Some(5),
        chunk: None,
        target_schema: None,
    }));
    // structured_content is whole regardless of chunking.
    let sc = result.structured_content.clone().unwrap();
    assert!(
        sc.get("summary").is_some(),
        "structured payload ships whole"
    );
    let text = extract_text(&result);
    assert!(
        text.contains("# Graph health"),
        "text is the markdown report: {text}"
    );
    assert!(
        text.contains("_total_chunks") || text.contains("_chunk"),
        "over-budget text carries chunk-walk frontmatter: {text}"
    );
    // The markdown text is no longer parseable as JSON.
    assert!(serde_json::from_str::<serde_json::Value>(&text).is_err());
}

/// Durability honesty (Plan 02, Part A) refusal complement: a
/// folder-backed (durable on disk) mem reports `durable: true` /
/// `storage: folder` in the `include_config` mem detail — the marker
/// reflects the real backend, so a durable mem never reads ephemeral.
#[test]
fn folder_mem_health_detail_reports_durable() {
    let (server, _tmp) = setup_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: true,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let sc = result.structured_content.clone().unwrap();
    let mems = sc["mems"]
        .as_array()
        .expect("include_config carries the per-mem detail array");
    assert!(!mems.is_empty(), "at least one writable folder mem");
    for v in mems {
        assert_eq!(v["durable"], true, "folder mem must report durable: {v}");
        assert_eq!(v["storage"], "folder", "folder mem storage kind: {v}");
    }
}

/// #57 refusal: a default-budget call keeps the text channel as
/// parseable JSON — byte-identical behavior for the common small call.
#[test]
fn health_default_budget_text_stays_json() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    assert!(
        serde_json::from_str::<serde_json::Value>(&text).is_ok(),
        "default-budget text stays JSON"
    );
}

#[test]
fn health_default_contains_legacy_stats_fields() {
    // `memstead_stats` is folded into `memstead_health`'s default output —
    // stats-era fields sit as top-level siblings of `summary`.
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    // Stats-era fields sit as top-level siblings of `summary`.
    assert!(json["total_nodes"].as_u64().unwrap() >= 2);
    assert!(json["real_nodes"].as_u64().unwrap() >= 2);
    assert!(json["stub_nodes"].is_number());
    assert!(json["total_edges"].as_u64().unwrap() >= 1);
    assert!(json["edge_types"].is_array());
    assert!(json["type_distribution"].is_array());
    assert!(json["writable_mems"].is_array());
    assert!(json["read_mems"].is_array());
    assert!(json["mem_schemas"].is_array());
}

#[test]
fn health_default_contains_existing_summary_fields() {
    // Absorbing stats must not regress the pre-existing `summary` object.
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let summary = json["summary"]
        .as_object()
        .expect("summary object still present");
    assert!(summary["total_entities"].is_number());
    assert!(summary["total_orphans"].is_number());
    assert!(summary["total_stubs"].is_number());
    assert!(summary["total_stale"].is_number());
    assert!(summary["total_missing_fields"].is_number());
    assert!(summary["total_communities"].is_number());
}

#[test]
fn health_never_surfaces_lifecycle_policy_fields() {
    // Lifecycle policy lives on `memstead_overview` — `memstead_health`
    // is drift / diagnostics, not "what can I do here." The two
    // legacy field names must not appear on either the default
    // or the `include_config=true` response.
    let (server, _tmp) = setup_dual_test_engine();
    for include_config in [false, true] {
        let result = server.memstead_health(Parameters(HealthParams {
            include: None,
            limit: None,
            mem: None,
            include_config,
            target_schema: None,
            token_budget: None,
            chunk: None,
        }));
        // #57: typed payload from structured_content (text is markdown).
        let json: serde_json::Value = result.structured_content.clone().unwrap();
        assert!(
            json.get("allowed_create_patterns").is_none(),
            "allowed_create_patterns must never surface on memstead_health (include_config={include_config})"
        );
        assert!(
            json.get("allowed_delete_patterns").is_none(),
            "allowed_delete_patterns must never surface on memstead_health (include_config={include_config})"
        );
    }
}

/// F6: under `memstead_health(mem=B)`,
/// `most_connected` degrees count only source-in-mem edges, matching
/// the same response's `total_edges`/`edge_types`. A cross-mem edge
/// `A→B` is excluded from B's scoped aggregate, so it must also be
/// excluded from B's node degree — pre-fix the per-node degree reused
/// the global adjacency and reported a degree that included an edge the
/// same scoped response did not count.
#[test]
fn health_scoped_most_connected_degrees_exclude_cross_mem_incoming() {
    use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = TempDir::new().unwrap();
    let a_dir = tmp.path().join("specs");
    let b_dir = tmp.path().join("memos");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    let mk_mount = |mem: &str, path: std::path::PathBuf| Mount {
        mem: mem.to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::Folder { path },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let a_writer = memstead_base::storage::FilesystemBackend::new(a_dir.clone());
    let b_writer = memstead_base::storage::FilesystemBackend::new(b_dir.clone());
    let mut engine = memstead_base::Engine::from_mounts(vec![
        (
            mk_mount("specs", a_dir),
            Box::new(a_writer) as Box<dyn memstead_base::backend::MemBackend>,
        ),
        (
            mk_mount("memos", b_dir),
            Box::new(b_writer) as Box<dyn memstead_base::backend::MemBackend>,
        ),
    ])
    .unwrap();
    // Grant specs → memos so the cross-mem relate lands.
    let mut cross_links = std::collections::BTreeMap::new();
    cross_links.insert(
        "specs".to_string(),
        memstead_schema::workspace_config::CrossLinkValue::Wildcard,
    );
    engine.set_settings(memstead_base::WorkspaceSettings {
        cross_mem_links: cross_links,
        ..Default::default()
    });
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    let mk = |mem: &str, title: &str| {
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "the identity".to_string());
        sections.insert("purpose".to_string(), "the purpose".to_string());
        let r = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            mem: Some(mem.to_string()),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
            role: None,
            identity: None,
        }));
        assert!(
            !r.is_error.unwrap_or(false),
            "create {title}: {}",
            extract_text(&r)
        );
    };
    mk("specs", "Source");
    mk("memos", "Target");
    mk("memos", "Hub");

    let relate = |from: &str, to: &str| {
        let r = server.memstead_relate(Parameters(RelateParams {
            relations: vec![RelateOpInput {
                from: from.to_string(),
                to: to.to_string(),
                rel_type: "USES".to_string(),
                remove: None,
                description: None,
            }],
            note: None,
            role: None,
            identity: None,
            dry_run: None,
        }));
        assert!(
            !r.is_error.unwrap_or(false),
            "relate {from}->{to}: {}",
            extract_text(&r)
        );
    };
    // Intra-mem incoming (source in memos → counted) and a
    // cross-mem incoming (source in specs → excluded under mem=memos).
    relate("memos--hub", "memos--target");
    relate("specs--source", "memos--target");

    // Global view: target has both incoming edges (degree 2).
    let global = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["most_connected".to_string()]),
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let gjson: serde_json::Value = serde_json::from_str(&extract_text(&global)).unwrap();
    let g_target = gjson["most_connected"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "memos--target")
        .expect("target in global most_connected");
    assert_eq!(
        g_target["incoming"].as_u64(),
        Some(2),
        "global degree counts both edges"
    );

    // Scoped to memos: the cross-mem incoming from specs is excluded
    // from both the aggregate and the node degree.
    let scoped = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["most_connected".to_string()]),
        limit: None,
        mem: Some("memos".to_string()),
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let sjson: serde_json::Value = serde_json::from_str(&extract_text(&scoped)).unwrap();
    let s_target = sjson["most_connected"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "memos--target")
        .expect("target in scoped most_connected");
    assert_eq!(
        s_target["incoming"].as_u64(),
        Some(1),
        "scoped degree must exclude the cross-mem incoming edge: {s_target}",
    );
    assert_eq!(s_target["outgoing"].as_u64(), Some(0));
    assert_eq!(s_target["total"].as_u64(), Some(1));
    // The scoped aggregate counts only the one source-in-memos edge
    // (hub→target); the cross-mem USES is source-in-specs, excluded —
    // so the node's degree and the aggregate now describe one subgraph.
    assert_eq!(
        sjson["total_edges"].as_u64(),
        Some(1),
        "scoped total_edges counts only source-in-memos edges: {}",
        sjson["total_edges"],
    );
}

/// Scoped `memstead_health` reports a community count that reflects the
/// mem: an empty mem → 0 communities (consistent with its 0
/// entities, no "0 entities / N communities" contradiction), and a
/// non-empty mem → exactly the clusters with ≥1 member in it. The
/// global (unscoped) count is unchanged. Filters the global
/// partition — no per-mem detection.
#[test]
fn health_scoped_community_count_reflects_mem() {
    use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = TempDir::new().unwrap();
    let specs_dir = tmp.path().join("specs");
    let memos_dir = tmp.path().join("memos");
    let scratch_dir = tmp.path().join("scratch");
    for d in [&specs_dir, &memos_dir, &scratch_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mk_mount = |mem: &str, path: std::path::PathBuf| Mount {
        mem: mem.to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::Folder { path },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine = memstead_base::Engine::from_mounts(vec![
        (
            mk_mount("specs", specs_dir.clone()),
            Box::new(memstead_base::storage::FilesystemBackend::new(specs_dir))
                as Box<dyn memstead_base::backend::MemBackend>,
        ),
        (
            mk_mount("memos", memos_dir.clone()),
            Box::new(memstead_base::storage::FilesystemBackend::new(memos_dir))
                as Box<dyn memstead_base::backend::MemBackend>,
        ),
        (
            mk_mount("scratch", scratch_dir.clone()),
            Box::new(memstead_base::storage::FilesystemBackend::new(scratch_dir))
                as Box<dyn memstead_base::backend::MemBackend>,
        ),
    ])
    .unwrap();
    let _ = &mut engine;
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    let mk = |mem: &str, title: &str| {
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "the identity".to_string());
        sections.insert("purpose".to_string(), "the purpose".to_string());
        let r = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            mem: Some(mem.to_string()),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
            role: None,
            identity: None,
        }));
        assert!(
            !r.is_error.unwrap_or(false),
            "create {title}: {}",
            extract_text(&r)
        );
    };
    let relate = |from: &str, to: &str| {
        let r = server.memstead_relate(Parameters(RelateParams {
            relations: vec![RelateOpInput {
                from: from.to_string(),
                to: to.to_string(),
                rel_type: "USES".to_string(),
                remove: None,
                description: None,
            }],
            note: None,
            role: None,
            identity: None,
            dry_run: None,
        }));
        assert!(
            !r.is_error.unwrap_or(false),
            "relate {from}->{to}: {}",
            extract_text(&r)
        );
    };
    // Two disconnected intra-mem edges → two clusters, one wholly
    // in `specs`, one wholly in `memos`. `scratch` stays empty.
    mk("specs", "A1");
    mk("specs", "A2");
    relate("specs--a1", "specs--a2");
    mk("memos", "B1");
    mk("memos", "B2");
    relate("memos--b1", "memos--b2");

    let total_communities = |mem: Option<&str>| -> u64 {
        let r = server.memstead_health(Parameters(HealthParams {
            include: None,
            limit: None,
            mem: mem.map(String::from),
            include_config: false,
            token_budget: None,
            chunk: None,
            target_schema: None,
        }));
        let j: serde_json::Value = serde_json::from_str(&extract_text(&r)).unwrap();
        j["summary"]["total_communities"].as_u64().unwrap()
    };
    let total_entities = |mem: Option<&str>| -> u64 {
        let r = server.memstead_health(Parameters(HealthParams {
            include: None,
            limit: None,
            mem: mem.map(String::from),
            include_config: false,
            token_budget: None,
            chunk: None,
            target_schema: None,
        }));
        let j: serde_json::Value = serde_json::from_str(&extract_text(&r)).unwrap();
        j["summary"]["total_entities"].as_u64().unwrap()
    };

    // Global: both clusters, all four entities.
    assert_eq!(total_communities(None), 2, "global community count");
    assert_eq!(total_entities(None), 4, "global entity count");

    // Empty mem: 0 entities, 0
    // communities (was 0 entities / 2 communities pre-fix).
    assert_eq!(total_entities(Some("scratch")), 0);
    assert_eq!(
        total_communities(Some("scratch")),
        0,
        "empty mem must report 0 communities, not the global count",
    );

    // Non-empty scope: exactly the one cluster whose members live in
    // the mem.
    assert_eq!(total_entities(Some("specs")), 2);
    assert_eq!(
        total_communities(Some("specs")),
        1,
        "specs touches one cluster"
    );
    assert_eq!(total_entities(Some("memos")), 2);
    assert_eq!(
        total_communities(Some("memos")),
        1,
        "memos touches one cluster"
    );
}

/// Scoped `memstead_overview` frontmatter reflects the mem: an empty
/// mem reports `_entity_count: 0` / `_cluster_count: 0` and a
/// "no communities" `## Communities` section — consistent with its
/// `## Mems` roster (was `_entity_count: N` / global clusters
/// pre-fix). A non-empty scope reports the mem's own count and only
/// its clusters. Global (unscoped) overview is unchanged. Mirrors the
/// health fix via the shared `clusters_in_mem` helper.
#[test]
fn overview_scoped_entity_and_community_count_reflect_mem() {
    use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = TempDir::new().unwrap();
    let specs_dir = tmp.path().join("specs");
    let scratch_dir = tmp.path().join("scratch");
    for d in [&specs_dir, &scratch_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mk_mount = |mem: &str, path: std::path::PathBuf| Mount {
        mem: mem.to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::Folder { path },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let engine = memstead_base::Engine::from_mounts(vec![
        (
            mk_mount("specs", specs_dir.clone()),
            Box::new(memstead_base::storage::FilesystemBackend::new(specs_dir))
                as Box<dyn memstead_base::backend::MemBackend>,
        ),
        (
            mk_mount("scratch", scratch_dir.clone()),
            Box::new(memstead_base::storage::FilesystemBackend::new(scratch_dir))
                as Box<dyn memstead_base::backend::MemBackend>,
        ),
    ])
    .unwrap();
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    let mk = |title: &str| {
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "the identity".to_string());
        sections.insert("purpose".to_string(), "the purpose".to_string());
        let r = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            mem: Some("specs".to_string()),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
            role: None,
            identity: None,
        }));
        assert!(
            !r.is_error.unwrap_or(false),
            "create {title}: {}",
            extract_text(&r)
        );
    };
    mk("A1");
    mk("A2");
    let r = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--a1".to_string(),
            to: "specs--a2".to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(!r.is_error.unwrap_or(false), "relate: {}", extract_text(&r));

    let overview = |mem: Option<&str>| -> String {
        extract_text(&server.memstead_overview(Parameters(OverviewParams {
            rebuild: None,
            chunk: None,
            mem: mem.map(String::from),
            include: None,
            token_budget: None,
        })))
    };

    // Global: one cluster, two entities.
    let global = overview(None);
    assert!(
        global.contains("_entity_count: 2"),
        "global entity count: {global}"
    );
    assert!(
        global.contains("_cluster_count: 1"),
        "global cluster count: {global}"
    );

    // Empty mem: reconcilable summary — 0 entities, 0 clusters, no
    // communities listed.
    let empty = overview(Some("scratch"));
    assert!(
        empty.contains("_entity_count: 0"),
        "empty-mem scope must report 0 entities: {empty}",
    );
    assert!(
        empty.contains("_cluster_count: 0"),
        "empty-mem scope must report 0 clusters, not the global count: {empty}",
    );
    assert!(
        empty.contains("_(no communities"),
        "empty-mem scope must render the no-communities section: {empty}",
    );

    // Non-empty scope: the mem's own count and its one cluster.
    let scoped = overview(Some("specs"));
    assert!(
        scoped.contains("_entity_count: 2"),
        "specs scope entity count: {scoped}"
    );
    assert!(
        scoped.contains("_cluster_count: 1"),
        "specs scope cluster count: {scoped}"
    );
}

/// `include=missing_required_outgoing`
/// is a recognised include key (not `UNKNOWN_INCLUDE_KEY`-rejected)
/// and surfaces an array on the response. The default-schema
/// fixture declares no `required_outgoing`, so the array is empty —
/// the test pins the wire-shape contract, not a violator count.
#[test]
fn health_missing_required_outgoing_include_key_is_recognised() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["missing_required_outgoing".to_string()]),
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let arr = json
        .get("missing_required_outgoing")
        .and_then(|v| v.as_array())
        .expect("missing_required_outgoing must surface as array under the include key");
    assert!(
        arr.is_empty(),
        "default schema declares no required_outgoing — array must be empty; got {arr:?}"
    );
    // Must NOT carry an UNKNOWN_INCLUDE_KEY warning for the key.
    let warnings = json
        .get("warnings")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let has_unknown = warnings.iter().any(|w| {
        w.get("code").and_then(|c| c.as_str()) == Some("UNKNOWN_INCLUDE_KEY")
            && w.get("details")
                .and_then(|d| d.get("key"))
                .and_then(|k| k.as_str())
                == Some("missing_required_outgoing")
    });
    assert!(
        !has_unknown,
        "include key must be on the allowlist; got warnings: {warnings:?}"
    );
}

/// `include=["conformance"]` is a
/// recognised key and surfaces a `findings` array in the pinned
/// `{id, axis, code, detail}` shape. The standard fixture's
/// entities conform to the pin, so the array's conformance
/// entries are keyed to whatever genuinely fails — the shape, not
/// a violator count, is the contract here.
/// The `config` include key and the `include_config: true` boolean
/// are aliases: each renders the identical projection, passing both
/// renders it once, and a call with neither carries no config block.
#[test]
fn health_config_include_key_and_boolean_alias_render_identically() {
    let (server, _tmp) = setup_test_engine();
    let call = |include: Option<Vec<String>>, include_config: bool| {
        let r = server.memstead_health(Parameters(HealthParams {
            include,
            limit: None,
            mem: None,
            include_config,
            token_budget: None,
            chunk: None,
            target_schema: None,
        }));
        assert!(!r.is_error.unwrap_or(false), "health must succeed: {r:?}");
        r.structured_content.unwrap()
    };

    let via_alias = call(None, true);
    let via_key = call(Some(vec!["config".to_string()]), false);
    let via_both = call(Some(vec!["config".to_string()]), true);
    for payload in [&via_alias, &via_key, &via_both] {
        for key in ["mems", "mutations", "plugin"] {
            assert!(
                payload.get(key).is_some(),
                "config projection must carry `{key}`: {payload}"
            );
        }
    }
    assert_eq!(
        via_alias, via_key,
        "alias and catalogue key must render the identical projection"
    );
    assert_eq!(via_key, via_both, "passing both renders it once");
    // The catalogue key is known — no UNKNOWN_INCLUDE_KEY warning.
    assert!(
        !via_key
            .get("warnings")
            .and_then(|w| w.as_array())
            .is_some_and(|w| w.iter().any(|e| e["code"] == "UNKNOWN_INCLUDE_KEY")),
        "`config` is a catalogue member, not an unknown key: {via_key}"
    );

    // Refusal complement: without the token there is no config block.
    let plain = call(None, false);
    for key in ["mems", "mutations", "plugin"] {
        assert!(
            plain.get(key).is_none(),
            "no config block without the opt-in: {plain}"
        );
    }
}

/// The `missing_fields` include carries WHICH condition each issue
/// reports: a genuinely absent section and content under a
/// non-deriving heading produce entries with distinct `code`s and
/// their messages, while the legacy `missing` field-name array
/// stays exactly as today for both. Exercised through the engine
/// composer (`compose_health`) over a sealed-violator workspace.
#[test]
fn health_missing_fields_include_carries_issue_codes_additively() {
    use memstead_base::backend::MemBackend;
    use memstead_base::storage::FilesystemBackend;
    use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = tempfile::TempDir::new().unwrap();
    let schemas_dir = tmp.path().join("schemas");
    let pkg = schemas_dir.join("debate");
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    std::fs::write(
            pkg.join("schema.yaml"),
            "name: debate\nversion: 0.1.0\ndescription: fixture\nwhen_to_use: tests\ntypes:\n  - question\nrelationships:\n  mode: strict\n  definitions:\n    - name: PART_OF\n      description: hier\n      default_weight: 3.0\n    - name: _default\n      description: fallback\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n",
        )
        .unwrap();
    std::fs::write(
            pkg.join("types").join("question.yaml"),
            "name: question\ndescription: t\nwhen_to_use: tests\nsections:\n  - key: answers\n    heading: Answers argued\n    required: true\n    search_weight: 10.0\n    write_rules: []\n  - key: notes\n    heading: Notes\n    required: false\n    search_weight: 3.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - answers\n  - notes\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - answers\nhealth_required_fields:\n  - answers\nstaleness_threshold_days: 90\nwrite_rules: []\n",
        )
        .unwrap();
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    // Content present under the schema's non-deriving heading →
    // SECTION_HEADING_MISMATCH, never "missing".
    std::fs::write(
        mem_dir.join("mismatch.md"),
        "---\ntype: question\n---\n# Mismatch\n\n## Answers argued\n\nPresent.\n",
    )
    .unwrap();
    // Genuinely absent section → MISSING, exactly as today.
    std::fs::write(
        mem_dir.join("absent.md"),
        "---\ntype: question\n---\n# Absent\n",
    )
    .unwrap();

    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = Mount {
        mem: "debate-mem".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "debate",
            semver::Version::new(0, 1, 0),
        )),
        storage: MountStorage::Folder { path: mem_dir },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine = memstead_base::Engine::from_mounts_with_schemas_dir(
        vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
        Some(&schemas_dir),
    )
    .unwrap();

    let include = vec!["missing_fields".to_string()];
    let args = memstead_base::ops::health_compose::HealthArgs {
        mem: None,
        include: &include,
        limit: None,
        target_schema: None,
        include_config: false,
        strict: false,
        today: None,
    };
    let config = memstead_base::ops::health_compose::HealthConfig {
        mutations: serde_json::Value::Null,
        plugin: serde_json::Value::Object(Default::default()),
    };
    let payload =
        memstead_projection::health::compose_health(&mut engine, &args, Vec::new(), &config)
            .expect("compose_health succeeds");

    let entries = payload["missing_fields"]
        .as_array()
        .expect("missing_fields include renders");
    let entry_for = |id: &str| {
        entries
            .iter()
            .find(|e| e["id"] == format!("debate-mem--{id}"))
            .unwrap_or_else(|| panic!("entry for {id}: {entries:?}"))
    };

    let mismatch = entry_for("mismatch");
    // Legacy array byte-identical to today's: bare field names.
    assert_eq!(mismatch["missing"], serde_json::json!(["answers"]));
    assert_eq!(mismatch["issues"][0]["code"], "SECTION_HEADING_MISMATCH");
    assert_eq!(mismatch["issues"][0]["field"], "answers");
    assert!(
        mismatch["issues"][0]["message"]
            .as_str()
            .unwrap()
            .contains("is not missing"),
        "the message rides beside the code: {mismatch}"
    );

    let absent = entry_for("absent");
    assert_eq!(absent["missing"], serde_json::json!(["answers"]));
    assert_eq!(absent["issues"][0]["code"], "MISSING");
    assert!(
        absent["issues"][0]["message"]
            .as_str()
            .unwrap()
            .contains("is empty"),
        "the message rides beside the code: {absent}"
    );
}

/// Shared read-envelope contract — the MCP `memstead_health` path and a
/// direct, rmcp-free call to `compose_health` emit identical bytes. Proves the
/// health read-envelope is produced by one transport-neutral builder
/// reachable with no rmcp type in the path, so a future `/api/health` is
/// not a hand-mirrored copy. Several `include` detail sections are active
/// so the comparison exercises the builder beyond the bare summary.
#[test]
fn memstead_health_mcp_path_matches_direct_composer_bytes() {
    let (server, _tmp) = setup_test_engine();
    let include = vec![
        "orphans".to_string(),
        "stubs".to_string(),
        "most_connected".to_string(),
        "missing_fields".to_string(),
        "stale".to_string(),
        "dangling_links".to_string(),
        "tags".to_string(),
    ];

    // MCP path: structured_content of the memstead_health tool.
    let mcp = server.memstead_health(Parameters(HealthParams {
        include: Some(include.clone()),
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    assert!(
        !mcp.is_error.unwrap_or(false),
        "memstead_health must succeed: {mcp:?}"
    );
    let mcp_payload = mcp
        .structured_content
        .clone()
        .expect("memstead_health returns structured_content");

    // Direct rmcp-free path: mirror the handler's pre-call reload, then
    // call the composer with no rmcp type involved. `config` is unread
    // here (include_config = false) but supplied for shape.
    let direct_payload = {
        let unified = server.unified_engine();
        let mut engine = unified.lock().unwrap();
        let drift = engine.reload_if_stale(None);
        let args = memstead_base::ops::health_compose::HealthArgs {
            mem: None,
            include: &include,
            limit: None,
            target_schema: None,
            include_config: false,
            strict: false,
            today: None,
        };
        let config = memstead_base::ops::health_compose::HealthConfig {
            mutations: serde_json::Value::Null,
            plugin: serde_json::Value::Object(Default::default()),
        };
        memstead_projection::health::compose_health(&mut engine, &args, drift, &config)
            .expect("compose_health succeeds")
    };

    assert_eq!(
        mcp_payload, direct_payload,
        "MCP health structured_content must equal the direct composer payload"
    );
    // Byte-level identity, not just structural equality.
    assert_eq!(
        serde_json::to_string(&mcp_payload).unwrap(),
        serde_json::to_string(&direct_payload).unwrap(),
        "serialized bytes must be identical across MCP and direct call"
    );
}

/// Migration wire surface: the migration
/// trigger's stable five-field response, the dual-pin
/// confirmation on `memstead_health`'s `mem_schemas`, and the typed
/// refusals. The standard fixture's `spec` entities are
/// non-conformant against the built-in `planning` schema (no `spec` type), so
/// migrating to it enters dual-pin.
#[test]
fn mem_set_schema_wire_lifecycle_and_health_confirmation() {
    let (server, _tmp) = setup_dual_test_engine();
    let call = |schema: &str| {
        server.memstead_mem_set_schema(Parameters(crate::lifecycle::MemSetSchemaParams {
            mem: "specs".to_string(),
            schema: schema.to_string(),
            note: None,
        }))
    };

    // noop — every field present, agent branches on `outcome`.
    let json: serde_json::Value =
        serde_json::from_str(&extract_text(&call("default@1.0.0"))).unwrap();
    assert_eq!(json["outcome"].as_str(), Some("noop"));
    assert_eq!(json["mem"].as_str(), Some("specs"));
    assert_eq!(json["schema_pin"].as_str(), Some("default@1.0.0"));
    assert!(json["migration_target"].is_null());
    assert_eq!(json["findings"].as_array().map(|a| a.len()), Some(0));

    // Non-integral target → migration starts; findings ride the
    // response in the linter shape.
    let json: serde_json::Value =
        serde_json::from_str(&extract_text(&call("planning@0.1.0"))).unwrap();
    assert_eq!(json["outcome"].as_str(), Some("migration_started"));
    assert_eq!(json["schema_pin"].as_str(), Some("default@1.0.0"));
    assert_eq!(json["migration_target"].as_str(), Some("planning@0.1.0"));
    let findings = json["findings"].as_array().unwrap();
    assert!(!findings.is_empty());
    assert!(
        findings
            .iter()
            .all(|f| f["axis"].as_str() == Some("conformance"))
    );

    // Dual-pin confirmation on memstead_health.
    let health = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: Some("specs".to_string()),
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    // #57: the text channel is now chunked markdown; the typed payload
    // lives in structured_content (always whole).
    let hj: serde_json::Value = health.structured_content.clone().unwrap();
    let entry = hj["mem_schemas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["mem"].as_str() == Some("specs"))
        .expect("specs entry present")
        .clone();
    assert_eq!(entry["schema"].as_str(), Some("default@1.0.0"));
    assert_eq!(entry["migration_target"].as_str(), Some("planning@0.1.0"));

    // Re-issue → pending.
    let json: serde_json::Value =
        serde_json::from_str(&extract_text(&call("planning@0.1.0"))).unwrap();
    assert_eq!(json["outcome"].as_str(), Some("migration_pending"));

    // Typed refusals.
    let missing = call("no-such@9.9.9");
    assert!(extract_text(&missing).contains("SCHEMA_NOT_FOUND"));
    let malformed = call("not-a-ref");
    assert!(extract_text(&malformed).contains("INVALID_INPUT"));
}

#[test]
fn health_conformance_include_surfaces_findings_array() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["conformance".to_string()]),
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let findings = json
        .get("findings")
        .and_then(|v| v.as_array())
        .expect("findings must surface as array under include=conformance");
    for f in findings {
        assert!(f.get("id").is_some(), "finding carries id: {f}");
        assert_eq!(f["axis"].as_str(), Some("conformance"));
        assert!(f.get("code").is_some(), "finding carries code: {f}");
        assert!(f.get("detail").is_some(), "finding carries detail: {f}");
    }
    let warnings = json
        .get("warnings")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        !warnings
            .iter()
            .any(|w| { w.get("code").and_then(|c| c.as_str()) == Some("UNKNOWN_INCLUDE_KEY") }),
        "conformance must be on the allowlist; got {warnings:?}"
    );
}

/// `include=["integrity"]` returns both axes in one `findings`
/// list, and conformance findings reuse the write-time typed
/// codes. Fixture: one entity with an undeclared metadata field
/// (conformance break, `UNKNOWN_METADATA_FIELD`) and a relation to
/// an absent target (load-time stub → consistency finding
/// `UNRESOLVED_STUB`). Two runs are byte-identical (determinism).
#[test]
fn health_integrity_include_returns_both_axes_with_write_time_codes() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            mem_dir.join("drifted.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nzzz_bogus_field: x\n---\n# Drifted\n\n## Identity\n\nCarries an undeclared metadata field.\n\n## Purpose\n\nConformance-break fixture.\n\n## Relationships\n\n- **USES**: [[never-created]]\n",
        )
        .unwrap();
    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let engine = setup_unified_test_engine(tmp.path());
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    let call = || {
        let result = server.memstead_health(Parameters(HealthParams {
            include: Some(vec!["integrity".to_string()]),
            limit: None,
            mem: Some("specs".to_string()),
            include_config: false,
            token_budget: None,
            chunk: None,
            target_schema: None,
        }));
        let text = extract_text(&result);
        serde_json::from_str::<serde_json::Value>(&text).unwrap()
    };
    let json = call();
    let findings = json
        .get("findings")
        .and_then(|v| v.as_array())
        .expect("findings array present")
        .clone();
    let code_of = |axis: &str, code: &str| {
        findings
            .iter()
            .any(|f| f["axis"].as_str() == Some(axis) && f["code"].as_str() == Some(code))
    };
    assert!(
        code_of("conformance", "UNKNOWN_METADATA_FIELD"),
        "undeclared field must lint with the write-time code; got {findings:?}"
    );
    assert!(
        code_of("consistency", "UNRESOLVED_STUB"),
        "stub target must surface on the consistency axis; got {findings:?}"
    );
    // Determinism: a second identical call is byte-identical on
    // the findings list.
    let second = call();
    assert_eq!(
        json.get("findings"),
        second.get("findings"),
        "two runs over unchanged state must produce identical findings"
    );
}

/// `target_schema` redirects the conformance lint: an unresolvable
/// ref refuses with `SCHEMA_NOT_FOUND`; a malformed ref refuses
/// with `INVALID_INPUT`. The valid case (the pin itself, spelled
/// explicitly) succeeds and returns the same findings as the
/// implicit-pin call.
#[test]
fn health_target_schema_resolution_and_refusals() {
    let (server, _tmp) = setup_dual_test_engine();
    let call = |target: Option<&str>| {
        server.memstead_health(Parameters(HealthParams {
            include: Some(vec!["conformance".to_string()]),
            limit: None,
            mem: None,
            include_config: false,
            token_budget: None,
            chunk: None,
            target_schema: target.map(|s| s.to_string()),
        }))
    };
    // Unresolvable → SCHEMA_NOT_FOUND.
    let missing = call(Some("no-such-schema@9.9.9"));
    let text = extract_text(&missing);
    assert!(
        missing.is_error.unwrap_or(false) && text.contains("SCHEMA_NOT_FOUND"),
        "unresolvable target_schema must refuse with SCHEMA_NOT_FOUND; got {text}"
    );
    // Malformed → INVALID_INPUT.
    let malformed = call(Some("default@^1.0"));
    let text = extract_text(&malformed);
    assert!(
        malformed.is_error.unwrap_or(false) && text.contains("INVALID_INPUT"),
        "malformed target_schema must refuse with INVALID_INPUT; got {text}"
    );
    // Explicit pin == implicit pin.
    let explicit = extract_text(&call(Some("default@1.0.0")));
    let implicit = extract_text(&call(None));
    let ej: serde_json::Value = serde_json::from_str(&explicit).unwrap();
    let ij: serde_json::Value = serde_json::from_str(&implicit).unwrap();
    assert_eq!(ej.get("findings"), ij.get("findings"));
}

#[test]
fn overview_surfaces_configured_lifecycle_namespaces() {
    // The `## Lifecycle Namespaces` section is rendered from
    // `engine.settings()`. Build an engine carrying two create
    // rules and one delete rule; the markdown response of
    // `memstead_overview` must carry the section listing each
    // rule's pattern, the actions it gates, and the allowed
    // schemas. Cross-reference: the `## Schemas` section's
    // entries carry a `**Reachable as:**` line naming every
    // pattern that can pin them.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    let settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        memstead_base::WorkspaceSettings {
            mem_create_rules: vec![
                memstead_base::CreateRuleSetting {
                    pattern: "exec-*".to_string(),
                    schemas: vec!["default@1.0.0".to_string()],
                    default_cross_links: None,
                },
                memstead_base::CreateRuleSetting {
                    pattern: "plan-*".to_string(),
                    schemas: vec!["default@1.0.0".to_string()],
                    default_cross_links: None,
                },
            ],
            mem_delete_rules: vec![memstead_base::DeleteRuleSetting {
                pattern: "exec-*".to_string(),
            }],
            ..Default::default()
        },
    );
    let unified_settings = settings.clone();
    let _ = (mem_dir, settings);
    let mut unified = setup_unified_test_engine(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let text = extract_text(&result);
    assert!(
        text.contains("## Lifecycle Namespaces"),
        "overview must carry the lifecycle namespaces section: {text}"
    );
    assert!(
        text.contains("`exec-*`"),
        "exec-* rule must surface: {text}"
    );
    assert!(
        text.contains("`plan-*`"),
        "plan-* rule must surface: {text}"
    );
    assert!(
        text.contains("default@1.0.0"),
        "rule schemas must surface: {text}"
    );
    // Cross-reference: schema entry names the patterns that allow it.
    assert!(
        text.contains("**Reachable as:**"),
        "schema cross-reference must surface: {text}"
    );
}

/// Lifecycle-Namespaces rendering surfaces rule schema pins
/// verbatim — no `(unresolved)` annotation, even for schemas no
/// registered mem currently pins. The annotation was previously
/// fired for any pin not in `engine.schemas()` or
/// `engine.workspace_schemas()`, mis-flagging built-in schemas as
/// non-functional and causing agents to skip working namespaces
/// (mem-lifecycle-audit Item 03). The `(invalid)` annotation
/// still fires for malformed pins — that's a real operator-config
/// bug worth flagging.
#[test]
fn lifecycle_section_does_not_label_resolvable_pins_as_unresolved() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    // `planning@0.1.0` is a built-in schema, but no mem in this
    // fixture pins it. Pre-fix the lifecycle rendering would mark
    // it `(unresolved)`; post-fix the raw pin surfaces verbatim.
    let settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        memstead_base::WorkspaceSettings {
            mem_create_rules: vec![memstead_base::CreateRuleSetting {
                pattern: "planning/plan-*".to_string(),
                schemas: vec!["planning@0.1.0".to_string()],
                default_cross_links: None,
            }],
            mem_delete_rules: vec![memstead_base::DeleteRuleSetting {
                pattern: "planning/plan-*".to_string(),
            }],
            ..Default::default()
        },
    );
    let unified_settings = settings.clone();
    let _ = (mem_dir, settings);
    let mut unified = setup_unified_test_engine(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let text = extract_text(&result);
    assert!(
        text.contains("planning@0.1.0"),
        "lifecycle rendering must list the rule's schema pin: {text}"
    );
    assert!(
        !text.contains("planning@0.1.0 (unresolved)"),
        "(unresolved) must NOT decorate a resolvable built-in schema: {text}"
    );
}

/// Companion: malformed pins still surface the `(invalid)`
/// annotation. The lifecycle rendering preserves this signal so an
/// operator typo (rule pattern with a non-parsing schema entry)
/// stays visible to the agent at overview time.
#[test]
fn lifecycle_section_keeps_invalid_annotation_for_malformed_pins() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    let settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        memstead_base::WorkspaceSettings {
            mem_create_rules: vec![memstead_base::CreateRuleSetting {
                pattern: "exec-*".to_string(),
                schemas: vec!["not a valid pin".to_string()],
                default_cross_links: None,
            }],
            mem_delete_rules: vec![],
            ..Default::default()
        },
    );
    let unified_settings = settings.clone();
    let _ = (mem_dir, settings);
    let mut unified = setup_unified_test_engine(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let text = extract_text(&result);
    assert!(
        text.contains("(invalid)"),
        "(invalid) annotation must survive for malformed pins: {text}"
    );
}

/// A fresh workspace at engine defaults has nothing to say under
/// `## Workspace policy` — the section stays absent and no `_policy`
/// frontmatter slot is emitted. Default-suppress keeps the overview
/// quiet for the common case.
#[test]
fn overview_workspace_policy_silent_on_defaults() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    let settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        memstead_base::WorkspaceSettings::default(),
    );
    let unified_settings = settings.clone();
    let _ = (mem_dir, settings);
    let mut unified = setup_unified_test_engine(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let text = extract_text(&result);
    assert!(
        !text.contains("## Workspace policy"),
        "default workspace must not emit Workspace policy section: {text}"
    );
    assert!(
        !text.contains("_policy:"),
        "default workspace must not stamp a _policy frontmatter slot: {text}"
    );
}

/// `require_notes = true` surfaces as a single-line entry in
/// `## Workspace policy` and as a `_policy` frontmatter slot.
#[test]
fn overview_workspace_policy_surfaces_require_notes() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    let settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        memstead_base::WorkspaceSettings {
            mutations: memstead_base::workspace::MutationsSection {
                require_notes: Some(true),
            },
            ..Default::default()
        },
    );
    let unified_settings = settings.clone();
    let _ = (mem_dir, settings);
    let mut unified = setup_unified_test_engine(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let text = extract_text(&result);
    assert!(
        text.contains("## Workspace policy"),
        "Workspace policy section must appear: {text}"
    );
    assert!(
        text.contains("**require_notes:** true"),
        "require_notes entry must appear in the section body: {text}"
    );
    assert!(
        text.contains("_policy: {require_notes: true}"),
        "_policy frontmatter slot must carry the inline flow mapping: {text}"
    );
}

/// Any schema referenced in a rule's `schemas[]` becomes visible
/// in `## Schemas`, even when no mem pins it. Builds a
/// workspace where the only pinned schema is `default@1.0.0` but
/// a rule lists `tinyschema@0.1.0` (registered via the workspace-
/// level schemas dir but not pinned by any mem). The overview
/// must surface `tinyschema` as a full schema entry with the
/// `**Reachable as:**` cross-reference pointing at the rule.
#[test]
fn overview_surfaces_rule_referenced_unpinned_schema() {
    // `memstead_overview_unified` enumerates rule-referenced
    // schemas; the workspace `schemas_dir` must be wired into
    // the engine so it resolves `tinyschema@0.1.0`. Use
    // Engine::from_mounts_with_schemas_dir to load
    // workspace-level schemas alongside the folder mount.
    let tmp = TempDir::new().unwrap();

    // Stage a workspace-level schemas dir carrying a second
    // schema (mirrors the `MEM_SCHEMA_NOT_ALLOWED` engine test
    // fixture). Minimal manifest: identity + open relationships
    // with the engine-required `_default`.
    let schemas_dir = tmp.path().join("schemas");
    let tinyschema_dir = schemas_dir.join("tinyschema");
    fs::create_dir_all(&tinyschema_dir).unwrap();
    fs::write(
        tinyschema_dir.join("schema.yaml"),
        r#"name: tinyschema
version: 0.1.0
description: test fixture for rule-referenced surfacing
when_to_use: test only
types: []
relationships:
  mode: open
  definitions:
    - name: _default
      description: fallback weight required by the engine
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
    )
    .unwrap();

    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    let settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        memstead_base::WorkspaceSettings {
            // Rule pins `tinyschema@0.1.0` — no mem references
            // this schema.
            mem_create_rules: vec![memstead_base::CreateRuleSetting {
                pattern: "exec-*".to_string(),
                schemas: vec!["tinyschema@0.1.0".to_string()],
                default_cross_links: None,
            }],
            mem_delete_rules: vec![],
            ..Default::default()
        },
    );
    let unified_settings = settings.clone();
    let _ = (mem_dir.clone(), settings);
    // Build unified with the folder mount + workspace schemas_dir
    // so tinyschema is loaded into the unified engine's catalogue.
    let mount = memstead_base::workspace::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::workspace::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::workspace::MountCapability::Write,
        lifecycle: memstead_base::workspace::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let backend = memstead_base::instantiate_local_backend(&mount).unwrap();
    let mut unified = memstead_base::Engine::from_mounts_with_schemas_dir(
        vec![(mount, backend)],
        Some(&schemas_dir),
    )
    .unwrap();
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let text = extract_text(&result);

    // tinyschema appears as a `### tinyschema@0.1.0` heading in
    // the `## Schemas` block — same surface as the mem-pinned
    // `default@1.0.0`, so the agent gets full description + types
    // + relationship vocabulary, not just a literal string in the
    // lifecycle namespaces section.
    assert!(
        text.contains("### tinyschema@0.1.0"),
        "rule-referenced unpinned schema must surface as ## Schemas entry: {text}"
    );
    // Description from the manifest reaches the markdown.
    assert!(
        text.contains("test fixture for rule-referenced surfacing"),
        "schema description must propagate: {text}"
    );
    // Cross-reference points at the rule's pattern.
    // The `**Reachable as:**` line under tinyschema must list `exec-*`.
    let tiny_section = text
        .split("### tinyschema@0.1.0")
        .nth(1)
        .expect("tinyschema section present");
    let tiny_until_next_h3 = tiny_section.split("\n### ").next().unwrap_or(tiny_section);
    assert!(
        tiny_until_next_h3.contains("**Reachable as:**"),
        "tinyschema must carry Reachable-as cross-reference: {tiny_until_next_h3}"
    );
    assert!(
        tiny_until_next_h3.contains("`exec-*`"),
        "tinyschema's Reachable-as must name the rule pattern: {tiny_until_next_h3}"
    );
}

#[test]
fn health_with_include_config_surfaces_writable_mem_origins() {
    // Under `include_config: true`, the response carries a
    // `mems` detail array with `{ name, origin }` per writable mem.
    // A freshly `Engine::init`ed mem from explicit `MemInit` input
    // is `ExplicitToml` → origin slug "explicit". Absent from the
    // response means the detail array wasn't emitted (regression
    // guard).
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: true,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mems = json
        .get("mems")
        .and_then(|v| v.as_array())
        .expect("mems detail array present when include_config is true");
    assert!(!mems.is_empty(), "at least one writable mem expected");
    for entry in mems {
        assert!(entry.get("name").and_then(|n| n.as_str()).is_some());
        assert_eq!(
            entry.get("origin").and_then(|o| o.as_str()),
            Some("explicit"),
            "setup_test_engine mem must carry origin=explicit, entry={entry}",
        );
    }
}

#[test]
fn health_with_include_config_surfaces_per_mem_vcs_subobject() {
    // `include_config: true`
    // adds a `vcs: { gitdir, worktree }` subobject to each writable
    // mem entry. Paths must be absolute and canonical, and must
    // match what `Engine::gitdir_for` / `worktree_for` return when
    // called directly — the MCP payload is the public form of
    // those primitives for tooling that needs the paths.
    //
    // `memstead_health_unified` emits the vcs subobject for
    // git-branch mounts when the workspace root carries the
    // disk-shape mem folder (`worktree_for` resolves the
    // worktree via the disk-shape composition).
    let tmp = setup_test_workspace();
    let mut unified = setup_unified_test_engine_git_branch(tmp.path());
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: true,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mems = json
        .get("mems")
        .and_then(|v| v.as_array())
        .expect("mems detail array present when include_config is true");
    let unified = server.unified_engine().clone();
    let engine = unified.lock().unwrap();
    for entry in mems {
        let name = entry.get("name").and_then(|n| n.as_str()).unwrap();
        let vcs = entry
            .get("vcs")
            .and_then(|v| v.as_object())
            .expect("vcs subobject present on writable-mem entry");
        let gitdir = std::path::PathBuf::from(
            vcs.get("gitdir")
                .and_then(|v| v.as_str())
                .expect("gitdir is a string"),
        );
        let worktree = std::path::PathBuf::from(
            vcs.get("worktree")
                .and_then(|v| v.as_str())
                .expect("worktree is a string"),
        );
        assert!(gitdir.is_absolute(), "gitdir must be absolute: {gitdir:?}");
        assert!(
            worktree.is_absolute(),
            "worktree must be absolute: {worktree:?}"
        );
        assert!(gitdir.exists(), "gitdir must exist on disk: {gitdir:?}");
        assert_eq!(
            gitdir,
            engine.gitdir_for(name).expect("gitdir_for resolves"),
            "MCP-surfaced gitdir must match Engine::gitdir_for",
        );
        assert_eq!(
            worktree,
            engine.worktree_for(name).expect("worktree_for resolves"),
            "MCP-surfaced worktree must match Engine::worktree_for",
        );
    }
}

#[test]
fn health_with_include_config_surfaces_mutations_and_plugin() {
    // `mutations` and `plugin` arrive on the MCP
    // response verbatim from `EffectiveSettings`. Default-empty
    // values (section absent from config) surface as
    // `require_notes: null` and `plugin: {}` — the absence of a
    // value is communicated explicitly rather than via absent key.
    let (server_default, _tmp1) = setup_dual_test_engine();
    let result = server_default.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: true,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["mutations"]["require_notes"], serde_json::Value::Null);
    assert_eq!(json["plugin"], serde_json::json!({}));

    // Now with non-default values threaded through the full-surface
    // constructor. Both must round-trip.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());

    let mutations = crate::config::MutationsSection {
        require_notes: Some(true),
    };
    let mut plugin = HashMap::new();
    // Any opaque sub-table proves the pass-through — the engine
    // never inspects the keys.
    let mut sub_table = toml::Table::new();
    sub_table.insert("enabled".into(), toml::Value::Boolean(true));
    sub_table.insert("mode".into(), toml::Value::String("session_bundle".into()));
    let mut claude_code = toml::Table::new();
    claude_code.insert("custom_setting".into(), toml::Value::Table(sub_table));
    plugin.insert("claude_code".into(), claude_code);

    let server = McpServer::new_with_config(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
        HashSet::new(),
        None,
        mutations,
        plugin,
    );
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: true,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["mutations"]["require_notes"], serde_json::json!(true));
    assert_eq!(
        json["plugin"]["claude_code"]["custom_setting"]["enabled"],
        serde_json::json!(true)
    );
    assert_eq!(
        json["plugin"]["claude_code"]["custom_setting"]["mode"],
        serde_json::json!("session_bundle")
    );
}

#[test]
fn health_without_include_config_omits_mutations_and_plugin() {
    // Absent opt-in → neither `mutations` nor `plugin` appear.
    // Zero-bytes contract for clients that never ask for config.
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json.get("mutations").is_none());
    assert!(json.get("plugin").is_none());
}

#[test]
fn health_without_include_config_omits_mems_detail() {
    // Absent opt-in → the `mems` detail array is not emitted.
    // Clients that never call with `include_config: true` pay zero
    // extra bytes and the default-posture contract is preserved.
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        json.get("mems").is_none(),
        "mems detail array must be absent when include_config is false"
    );
}

#[test]
fn health_after_runtime_create_lists_new_mem_with_origin_runtime_created() {
    // A mem registered via `memstead_mem_create` surfaces with
    // `origin: "runtime_created"` on a subsequent `memstead_health
    // { include_config: true }` call. Pins the provenance path
    // skills rely on to distinguish explicit-init and
    // runtime-created registrations without a trial-create
    // round-trip.
    let tmp = TempDir::new().unwrap();
    let settings = memstead_base::WorkspaceSettings {
        mem_create_rules: vec![
            memstead_base::CreateRuleSetting {
                pattern: "*".to_string(),
                schemas: vec!["default@1.0.0".to_string()],
                default_cross_links: None,
            },
            memstead_base::CreateRuleSetting {
                pattern: "**".to_string(),
                schemas: vec!["default@1.0.0".to_string()],
                default_cross_links: None,
            },
        ],
        mem_delete_rules: vec![],
        ..Default::default()
    };
    memstead_git_branch::test_support::auto_seed_with_settings(tmp.path(), settings.clone());
    let mut unified = setup_unified_test_engine(tmp.path());
    unified.set_settings(settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);
    let target = tmp.path().join("runtime-born");
    let create_result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "runtime-born".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: Some("origin surface test".to_string()),
        recovery: None,
        include_schema: false,
    }));
    assert!(
        create_result.is_error.is_none() || create_result.is_error == Some(false),
        "create must succeed: {:?}",
        create_result,
    );

    let health_result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: true,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    // #57: typed payload from structured_content (text is now markdown).
    let json: serde_json::Value = health_result.structured_content.clone().unwrap();
    let mems = json
        .get("mems")
        .and_then(|v| v.as_array())
        .expect("mems detail array present");
    let entry = mems
        .iter()
        .find(|v| v.get("name").and_then(|n| n.as_str()) == Some("runtime-born"))
        .expect("runtime-born mem must be listed");
    assert_eq!(
        entry.get("origin").and_then(|o| o.as_str()),
        Some("runtime_created"),
        "runtime-created mem must carry origin=runtime_created",
    );
}

#[test]
fn test_memstead_entity_found() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_entity(Parameters(EntityParams {
        id: "specs--entity-a".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    assert!(text.contains("# Entity A"));
    assert!(text.contains("_hash:"));
}

#[test]
fn test_memstead_entity_not_found() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_entity(Parameters(EntityParams {
        id: "specs--nonexistent".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    assert!(text.contains("not found"));
}

/// Data-origin labelling: `memstead_entity` and `memstead_search`
/// stamp `origin` on the served content. An entity/hit from a writable
/// mount is `first-party`; one from a read-only mount (a stand-in for
/// a registry-installed read-mem or an adopted foreign folder/clone)
/// is `third-party` so the consuming agent treats it as quoted,
/// untrusted data.
#[test]
fn entity_and_search_stamp_data_origin_by_mount_capability() {
    use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = TempDir::new().unwrap();
    let writable_dir = tmp.path().join("writable");
    let readonly_dir = tmp.path().join("readonly");
    std::fs::create_dir_all(&writable_dir).unwrap();
    std::fs::create_dir_all(&readonly_dir).unwrap();
    std::fs::write(
        writable_dir.join("note.md"),
        "---\ntype: spec\n---\n# Note\n\n## Identity\n\nA labeltest entity.\n",
    )
    .unwrap();
    std::fs::write(
        readonly_dir.join("ext.md"),
        "---\ntype: spec\n---\n# Ext\n\n## Identity\n\nA labeltest entity.\n",
    )
    .unwrap();

    let mk = |mem: &str, dir: std::path::PathBuf, cap: MountCapability| Mount {
        mem: mem.to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::Folder { path: dir },
        capability: cap,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mounts: Vec<(Mount, Box<dyn memstead_base::backend::MemBackend>)> = vec![
        {
            let m = mk("local", writable_dir, MountCapability::Write);
            let b = memstead_base::instantiate_local_backend(&m).unwrap();
            (m, b)
        },
        {
            let m = mk("external", readonly_dir, MountCapability::ReadOnly);
            let b = memstead_base::instantiate_local_backend(&m).unwrap();
            (m, b)
        },
    ];
    let engine = memstead_base::Engine::from_mounts(mounts).unwrap();
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    // Single-entity reads carry the data-origin label.
    let writable_entity = server.memstead_entity(Parameters(EntityParams {
        id: "local--note".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let sc = writable_entity.structured_content.clone().unwrap();
    assert_eq!(
        sc.get("origin").and_then(|o| o.as_str()),
        Some("first-party"),
        "writable-mount entity is first-party"
    );

    let readonly_entity = server.memstead_entity(Parameters(EntityParams {
        id: "external--ext".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let sc = readonly_entity.structured_content.clone().unwrap();
    assert_eq!(
        sc.get("origin").and_then(|o| o.as_str()),
        Some("third-party"),
        "read-only-mount entity is third-party"
    );

    // Search hits carry per-hit data-origin labels keyed on each
    // hit's source mem.
    let search = server.memstead_search(Parameters(search_params_defaults()));
    let sc = search.structured_content.clone().unwrap();
    let hits = sc.get("hits").and_then(|h| h.as_array()).expect("hits[]");
    let origin_of = |mem: &str| -> Option<String> {
        hits.iter()
            .find(|h| h.get("mem").and_then(|v| v.as_str()) == Some(mem))
            .and_then(|h| h.get("origin").and_then(|o| o.as_str()))
            .map(|s| s.to_string())
    };
    assert_eq!(
        origin_of("local").as_deref(),
        Some("first-party"),
        "writable-mem hit is first-party"
    );
    assert_eq!(
        origin_of("external").as_deref(),
        Some("third-party"),
        "read-only-mem hit is third-party"
    );
}

/// `memstead_overview` marks a read-only (third-party) mem's data
/// origin at the cold-start surface so an agent learns which mems
/// are untrusted before reading their content; a writable (first-party)
/// mem stays unmarked.
#[test]
fn overview_marks_read_only_mem_third_party_origin() {
    use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = TempDir::new().unwrap();
    let writable_dir = tmp.path().join("writable");
    let readonly_dir = tmp.path().join("readonly");
    std::fs::create_dir_all(&writable_dir).unwrap();
    std::fs::create_dir_all(&readonly_dir).unwrap();
    std::fs::write(
        writable_dir.join("note.md"),
        "---\ntype: spec\n---\n# Note\n\n## Identity\n\nLocal.\n",
    )
    .unwrap();
    std::fs::write(
        readonly_dir.join("ext.md"),
        "---\ntype: spec\n---\n# Ext\n\n## Identity\n\nForeign.\n",
    )
    .unwrap();

    let mk = |mem: &str, dir: std::path::PathBuf, cap: MountCapability| Mount {
        mem: mem.to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::Folder { path: dir },
        capability: cap,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mounts: Vec<(Mount, Box<dyn memstead_base::backend::MemBackend>)> = vec![
        {
            let m = mk("local", writable_dir, MountCapability::Write);
            let b = memstead_base::instantiate_local_backend(&m).unwrap();
            (m, b)
        },
        {
            let m = mk("external", readonly_dir, MountCapability::ReadOnly);
            let b = memstead_base::instantiate_local_backend(&m).unwrap();
            (m, b)
        },
    ];
    let engine = memstead_base::Engine::from_mounts(mounts).unwrap();
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    let text = extract_text(&server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    })));

    // The read-only mem carries the third-party origin marker.
    let external_section = text
        .split("### external")
        .nth(1)
        .expect("overview lists the external mem");
    let external_block = external_section.split("### ").next().unwrap();
    assert!(
        external_block.contains("**Origin:** third-party"),
        "read-only mem must be marked third-party in overview; got:\n{external_block}"
    );

    // The writable mem stays unmarked (first-party, common case).
    let local_section = text
        .split("### local")
        .nth(1)
        .expect("overview lists the local mem");
    let local_block = local_section.split("### ").next().unwrap();
    assert!(
        !local_block.contains("**Origin:**"),
        "writable mem must not carry an origin marker; got:\n{local_block}"
    );
}

/// `memstead_mem_create` end-to-end through the unified engine.
#[test]
fn test_memstead_mem_create_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let seed_dir = tmp.path().join("seed");
    let writer = memstead_base::storage::FilesystemBackend::new(seed_dir.clone());
    let mount = memstead_base::Mount {
        mem: "seed".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: seed_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    // Install the full backend factory so create_mem's
    // git-branch path can materialise a writer when the
    // workspace-shape heuristic fires.
    unified.set_backend_factory(memstead_git_branch::storage::instantiate_full_backend);
    // Canonicalise the workspace_root so the outside_workspace
    // check inside create_mem compares canonical paths
    // consistently (macOS resolves `/var/...` → `/private/var/...`).
    let workspace_root = tmp.path().canonicalize().unwrap();
    unified.set_workspace_root(workspace_root.clone());
    unified.set_settings(memstead_base::WorkspaceSettings {
        mem_create_rules: vec![memstead_base::CreateRuleSetting {
            pattern: "*".to_string(),
            schemas: vec!["*".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_mem_create(Parameters(crate::lifecycle::MemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "alpha".to_string(),
        location: workspace_root.join("alpha").to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: Some("seed".to_string()),
        recovery: None,
        // Opt in so this end-to-end test still
        // sees the inlined schema body it asserts on below.
        include_schema: true,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "expected success; got {}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    // Response carries the load-bearing fields.
    assert!(text.contains("\"name\""));
    assert!(text.contains("\"location\""));
    assert!(text.contains("\"schema_ref\""));
    assert!(text.contains("\"seed_write_id\""));
    // The new mem's name surfaces.
    assert!(text.contains("alpha"));
    // Schema priming payload is folded in.
    assert!(text.contains("\"schema\""));

    // Surface parity (Plan 01): the include_schema inline path honours
    // `schema_verbosity: "lite"` — a first-mem create can prime on the
    // cheap skeleton instead of the ~25 KB full body.
    let lite_create = server.memstead_mem_create(Parameters(crate::lifecycle::MemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: Some("lite".to_string()),
        write_guidance: Default::default(),
        name: "beta".to_string(),
        location: workspace_root.join("beta").to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: Some("seed".to_string()),
        recovery: None,
        include_schema: true,
    }));
    assert!(
        !lite_create.is_error.unwrap_or(false),
        "lite create must succeed; got {}",
        extract_text(&lite_create)
    );
    let inlined = lite_create.structured_content.unwrap();
    let schema_body = &inlined["schema"];
    assert!(
        schema_body["types_summary"].is_array(),
        "include_schema=lite inlines the skeleton, got {schema_body}"
    );
    assert!(
        schema_body.get("types").is_none(),
        "lite inline drops the rich types[] array"
    );

    // An unknown schema_verbosity refuses up front (before the mem
    // lands) rather than silently inlining full.
    let bad = server.memstead_mem_create(Parameters(crate::lifecycle::MemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: Some("brief".to_string()),
        write_guidance: Default::default(),
        name: "gamma".to_string(),
        location: workspace_root.join("gamma").to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: Some("seed".to_string()),
        recovery: None,
        include_schema: true,
    }));
    assert!(
        bad.is_error.unwrap_or(false),
        "unknown schema_verbosity refuses"
    );
    assert_eq!(bad.structured_content.unwrap()["code"], "INVALID_INPUT");
}

/// `memstead_mem_delete` end-to-end through the unified engine.
#[test]
fn test_memstead_mem_delete_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let target_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(target_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder {
            path: target_dir.clone(),
        },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    unified.set_settings(memstead_base::WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![memstead_base::DeleteRuleSetting {
            pattern: "*".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_mem_delete(Parameters(crate::lifecycle::MemDeleteParams {
        name: "specs".to_string(),
        note: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "expected success; got {}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    assert!(text.contains("\"deleted_from_router\""));
    assert!(text.contains("\"files_deleted\""));
    assert!(text.contains("\"name\""));
    assert!(text.contains("specs"));
}

/// `memstead_overview` end-to-end through the unified engine.
/// Verifies the response carries the load-bearing markdown
/// sections (`## Schemas`, `## Mems`, `## Communities`,
/// `## Lifecycle Namespaces`).
#[test]
fn test_memstead_overview_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_overview(Parameters(OverviewParams {
        mem: None,
        include: None,
        token_budget: None,
        chunk: None,
        rebuild: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    // Frontmatter present.
    assert!(text.starts_with("---\n"));
    assert!(text.contains("_overview_mode:"));
    assert!(text.contains("_cluster_count:"));
    // Load-bearing markdown sections.
    assert!(text.contains("## Lifecycle Namespaces"));
    assert!(text.contains("## Schemas"));
    assert!(text.contains("## Mems"));
    assert!(text.contains("## Communities"));
    // The fixture mem should surface.
    assert!(text.contains("### specs"));
    // The fixture schema ref should surface.
    assert!(text.contains("default@1.0.0"));
}

/// `memstead_health` default body end-to-end through the unified
/// engine. Verifies the default-shape response carries
/// `writable_mems`, `read_mems`, `mem_schemas`, summary
/// counts, and edge totals.
#[test]
fn test_memstead_health_default_body_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Default body (no include_config) routes through unified.
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    // Default-shape fields present.
    assert!(text.contains("\"writable_mems\""));
    assert!(text.contains("\"read_mems\""));
    assert!(text.contains("\"mem_schemas\""));
    assert!(text.contains("\"summary\""));
    assert!(text.contains("\"total_entities\""));
    assert!(text.contains("\"edge_types\""));
    // The fixture mem should surface.
    assert!(text.contains("\"specs\""));
}

/// `memstead_health` with `include_config: true` end-to-end. The
/// per-mem `mems` detail block surfaces `origin`, the
/// optional `vcs` block, and the parsed
/// `.memstead/config.json` (`write_guidance` + `extra`). F6
/// renamed the wire-facing key from camelCase `writeGuidance`
/// to snake_case `write_guidance` (the on-disk JSON key stays
/// `writeGuidance` — that file format is human-authored and
/// retains its existing shape).
#[test]
fn test_memstead_health_include_config_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");

    // Drop a `.memstead/config.json` so `mem_config_for` surfaces
    // a non-empty `write_guidance` block in the response.
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": { "tone": "formal" }
        }"#;
    std::fs::write(mem_dir.join(".memstead").join("config.json"), config_body).unwrap();

    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: true,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let payload = result
        .structured_content
        .as_ref()
        .expect("structured_content present");
    assert!(
        payload.get("mutations").is_some(),
        "mutations present: {payload}"
    );
    assert!(payload.get("plugin").is_some(), "plugin present: {payload}");
    let mems = payload["mems"].as_array().expect("mems[] array");
    let entry = mems
        .iter()
        .find(|v| v["name"] == "specs")
        .expect("specs entry");
    // Folder mount: vcs.worktree must be present (gitdir is
    // git-branch-only; head may be absent on a fresh mem).
    let vcs = entry["vcs"]
        .as_object()
        .expect("vcs block present for folder mount");
    assert!(
        vcs.contains_key("worktree"),
        "vcs.worktree present: {vcs:?}"
    );
    // Snake_case rename — old camelCase must NOT appear on the wire.
    assert!(
        entry.get("write_guidance").is_some(),
        "write_guidance present"
    );
    assert!(
        entry.get("writeGuidance").is_none(),
        "legacy camelCase writeGuidance must be gone: {entry}",
    );
    assert_eq!(entry["write_guidance"]["tone"], "formal");
    // `extra` is documented as the forward-compat catch-all; the
    // fixture supplies no unknown keys, so it serialises as an
    // empty object.
    assert!(entry.get("extra").is_some(), "extra present: {entry}");
}

/// `memstead_entity` end-to-end through the unified engine —
/// found-path + not-found.
#[test]
fn test_memstead_entity_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Found-path: same shape as test_memstead_entity_found.
    let result = server.memstead_entity(Parameters(EntityParams {
        id: "specs--entity-a".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    assert!(text.contains("# Entity A"));
    assert!(text.contains("_hash:"));

    // Not-found path on the unified branch.
    let missing = server.memstead_entity(Parameters(EntityParams {
        id: "specs--nope".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(missing.is_error.unwrap_or(false));
    assert!(extract_text(&missing).contains("not found"));
}

/// `memstead_schema` end-to-end. The engine's per-mem HashMap
/// shape iterates values for name+version lookup; the not-found
/// envelope ships an empty suggestions list. `used_by` derives
/// from `engine.mounts()`.
#[test]
fn test_memstead_schema_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Found path: bare-name lookup picks the first matching schema.
    let result = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: Some("default".to_string()),
        mem: None,
        types: None,
        token_budget: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    assert!(text.contains("\"ref\""));
    assert!(text.contains("\"used_by\""));
    // The fixture mem "specs" pins default@1.0.0 → it appears in used_by.
    assert!(text.contains("\"specs\""));

    // Intra-mem
    // relationship entries surface `allowed_sources` and
    // `allowed_targets` so agents can pre-filter rel-types for
    // their `(from_type, to_type)` pair without trial-and-error
    // against `INVALID_REL_SHAPE`. The field names mirror the
    // error envelope's `allowed_source_types` /
    // `allowed_target_types` payload (modulo the trimmed
    // `_types` suffix). Empty arrays = "any type admitted"
    // (no pinning).
    assert!(
        text.contains("\"allowed_sources\""),
        "schema response must surface allowed_sources per relationship; got:\n{text}"
    );
    assert!(
        text.contains("\"allowed_targets\""),
        "schema response must surface allowed_targets per relationship; got:\n{text}"
    );

    // Not-found path: text content carries the message. The
    // empty suggestions array lives on structured_content (the
    // envelope payload) — not asserted in the text body.
    let missing = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: Some("no-such-schema".to_string()),
        mem: None,
        types: None,
        token_budget: None,
    }));
    assert!(missing.is_error.unwrap_or(false));
    let text = extract_text(&missing);
    assert!(text.contains("schema not found"));
    // Unified path drops suggest_name → no "Did you mean" suffix.
    assert!(!text.contains("Did you mean"));
}

/// `memstead_schema` honours the `verbosity` toggle: an absent value
/// is exactly the lite structural skeleton (the cold-start default —
/// a fresh session pays the skeleton price), `full` returns the
/// complete prose payload, and an unrecognized value refuses typed
/// rather than silently falling back.
#[test]
fn test_memstead_schema_verbosity_toggle() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let call = |verbosity: Option<&str>| {
        server.memstead_schema(Parameters(SchemaParams {
            verbosity: verbosity.map(|s| s.to_string()),
            name: Some("default".to_string()),
            mem: None,
            types: None,
            token_budget: None,
        }))
    };

    // Lite: structural skeleton under the distinct keys, prose dropped,
    // alias pointer + endpoints retained.
    let lite = call(Some("lite"));
    assert!(!lite.is_error.unwrap_or(false));
    let lite_body = lite.structured_content.clone().unwrap();
    assert!(
        lite_body["types_summary"].is_array(),
        "lite has types_summary"
    );
    assert!(
        lite_body["relationships_summary"].is_array(),
        "lite has relationships_summary"
    );
    assert!(lite_body.get("types").is_none(), "lite omits rich types");
    assert!(
        lite_body.get("description").is_none(),
        "lite drops schema description prose"
    );
    assert_eq!(lite_body["alias_target_rel_type"], "REFERENCES");

    // Absent verbosity == lite == explicit "lite" (byte-identical) —
    // the schema cold-start costs the skeleton price by default.
    let default_body = call(None).structured_content.unwrap();
    assert_eq!(default_body, lite_body, "absent verbosity must equal lite");
    let full_body = call(Some("full")).structured_content.unwrap();
    assert!(full_body["types"].is_array(), "full keeps rich types");
    assert!(full_body["description"].is_string(), "full keeps prose");

    // Lite is smaller than full on the same mem.
    let lite_len = serde_json::to_string(&lite_body).unwrap().len();
    let full_len = serde_json::to_string(&full_body).unwrap().len();
    assert!(lite_len < full_len, "lite ({lite_len}) < full ({full_len})");

    // Unknown value: typed INVALID_INPUT naming the bad value, no
    // silent fallback.
    let bogus = call(Some("brief"));
    assert!(
        bogus.is_error.unwrap_or(false),
        "unknown verbosity must error"
    );
    let env = bogus.structured_content.clone().unwrap();
    assert_eq!(env["code"], "INVALID_INPUT");
    assert_eq!(env["details"]["value"], "brief");
    assert!(
        extract_text(&bogus).contains("brief"),
        "error names the bad value"
    );
}

/// `memstead_schema` resolves built-in schemas even when no mem pins
/// them. Closes the documented discovery contract:
/// `memstead_overview` advertises lifecycle namespaces whose schemas
/// (`default@1.0.0`, `planning@0.1.0`) the workspace does not
/// necessarily pin yet; an agent must still be able to introspect
/// the schema by name before committing to `memstead_mem_create`.
/// The unified engine's catalogue cascade (mem-pinned →
/// workspace → built-ins) walks all three on every call.
#[test]
fn test_memstead_schema_resolves_builtin_when_no_mem_pins_it() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    // Pin a builtin that's NOT `planning@0.1.0` so the planning
    // resolution path must go through the builtin catalogue.
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // `default@1.0.0` is also a builtin; pinned-by-mem resolves
    // first but the canonical-pin path is what agents call.
    let result = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: Some("default@1.0.0".to_string()),
        mem: None,
        types: None,
        token_budget: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    assert!(text.contains("\"ref\""));
    assert!(text.contains("\"default@1.0.0\""));

    // `planning@0.1.0` — no mem pins this; resolution must fall
    // through to the builtin catalogue. Prior to the fix this
    // returned ENTITY_NOT_FOUND.
    let result = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: Some("planning@0.1.0".to_string()),
        mem: None,
        types: None,
        token_budget: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "planning@0.1.0 must resolve via the builtin catalogue: {}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    assert!(text.contains("\"ref\""));
    assert!(text.contains("\"planning@0.1.0\""));
    // No mem pins planning, so used_by[] is empty.
    assert!(text.contains("\"used_by\": []"));

    // Bare-name lookup also routes through the cascade.
    let result = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: Some("planning".to_string()),
        mem: None,
        types: None,
        token_budget: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    assert!(text.contains("\"planning@0.1.0\""));
}

/// `memstead_schema(mem=<name>)` resolves the mem's pinned
/// `schema_ref` from the mount roster — closes the one-hop
/// round-trip an agent would otherwise pay when it cold-starts
/// through `memstead_overview` and wants to write against a specific
/// mem. Same wire shape as the `name`-driven path; the only
/// difference is the lookup key.
#[test]
fn test_memstead_schema_mem_shortcut() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Happy path: mem → pinned schema.
    let result = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: None,
        mem: Some("specs".to_string()),
        types: None,
        token_budget: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "{:?}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    assert!(text.contains("\"default@1.0.0\""));
    assert!(text.contains("\"specs\""));

    // Unknown mem: typed UNKNOWN_MEM with `details.known_mems`.
    let result = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: None,
        mem: Some("not-a-mem".to_string()),
        types: None,
        token_budget: None,
    }));
    assert!(result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    assert!(text.contains("UNKNOWN_MEM"), "got: {text}");
    let envelope = result.structured_content.as_ref().unwrap();
    assert_eq!(envelope["code"], "UNKNOWN_MEM");
    let known = envelope["details"]["known_mems"].as_array().unwrap();
    assert!(known.iter().any(|v| v == "specs"));

    // Conflict: both name and mem → INVALID_INPUT.
    let result = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: Some("default".to_string()),
        mem: Some("specs".to_string()),
        types: None,
        token_budget: None,
    }));
    assert!(result.is_error.unwrap_or(false));
    let envelope = result.structured_content.as_ref().unwrap();
    assert_eq!(envelope["code"], "INVALID_INPUT");

    // Neither: also INVALID_INPUT.
    let result = server.memstead_schema(Parameters(SchemaParams {
        verbosity: None,
        name: None,
        mem: None,
        types: None,
        token_budget: None,
    }));
    assert!(result.is_error.unwrap_or(false));
    let envelope = result.structured_content.as_ref().unwrap();
    assert_eq!(envelope["code"], "INVALID_INPUT");
}

/// `memstead_reload` end-to-end through the unified engine. Asserts
/// the rich-shape `reports[]` wire contract.
#[test]
fn test_memstead_reload_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_reload(Parameters(ReloadParams {
        mem: Some("specs".to_string()),
        full: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    // Rich-shape response: each report carries mem, head_before,
    // head_after, entities_loaded, changed_entity_ids.
    assert!(text.contains("\"reports\""));
    assert!(text.contains("\"mem\""));
    assert!(text.contains("\"head_before\""));
    assert!(text.contains("\"head_after\""));
    assert!(text.contains("\"entities_loaded\""));
    assert!(text.contains("\"changed_entity_ids\""));
    // Default form carries NO refresh block — the content reload
    // is unchanged for callers that don't ask for the full mode.
    assert!(
        !text.contains("\"refresh\""),
        "default reload must not carry a refresh block: {text}"
    );
    // `full` is workspace-scoped: combining it with `mem` refuses.
    let result = server.memstead_reload(Parameters(ReloadParams {
        mem: Some("specs".to_string()),
        full: Some(true),
    }));
    assert!(result.is_error.unwrap_or(false));
    let envelope = result.structured_content.as_ref().unwrap();
    assert_eq!(envelope["code"], "INVALID_INPUT");
}

/// End to end: the EXACT sequence a user
/// reported blocked — install a new schema out of band, create a
/// mem pinned to it in-band, write an entity into it — completes
/// warm, with `memstead_reload full=true` as the only extra step
/// and no process restart. Pre-refresh, the same `mem_create`
/// still fails with `SCHEMA_NOT_FOUND` (the refresh changes the
/// outcome, not incidental timing).
#[test]
fn test_memstead_reload_full_closes_the_out_of_band_schema_gap() {
    use memstead_base::workspace_store::WorkspaceStoreAdapter;

    let tmp = TempDir::new().unwrap();
    // Canonicalize so the mem-create location gate (which
    // canonicalizes candidates) sees paths inside the root even
    // on macOS's symlinked tmp.
    let root = tmp.path().canonicalize().unwrap();
    let root = root.as_path();
    let mem_dir = root.join("specs");
    fs::create_dir_all(&mem_dir).unwrap();
    fs::create_dir_all(root.join(".memstead")).unwrap();
    // Workspace policy on disk: wildcard create rule, so the
    // in-band `memstead_mem_create` is admitted (and survives the
    // refresh's settings re-read).
    fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n\n[[mem_management.create]]\npattern = \"*\"\nschemas = [\"*\"]\n",
        )
        .unwrap();
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder {
            path: mem_dir.clone(),
        },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
            &memstead_base::workspace::Workspace {
                mounts: vec![mount],
                settings: memstead_base::workspace::WorkspaceSettings::default(),
            },
        )
        .unwrap();
    let engine = memstead_base::Engine::from_workspace_root(root).expect("workspace boots");
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    // Out of band, while the server runs: install a schema the
    // way `memstead schema install` does on a folder workspace —
    // the package lands under `.memstead/schemas/<name>@<ver>/`.
    let pkg = root
        .join(".memstead")
        .join("schemas")
        .join("authored@0.1.0");
    fs::create_dir_all(pkg.join("types")).unwrap();
    fs::write(
        pkg.join("schema.yaml"),
        r#"name: authored
version: 0.1.0
description: out-of-band installed schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
    )
    .unwrap();
    fs::write(
        pkg.join("types").join("doc.yaml"),
        r#"name: doc
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#,
    )
    .unwrap();

    let mem_create = |name: &str| {
        server.memstead_mem_create(Parameters(crate::lifecycle::MemCreateParams {
            title: None,
            description: None,
            subject: None,
            name: name.to_string(),
            location: name.to_string(),
            schema: "authored@0.1.0".to_string(),
            vcs: None,
            note: None,
            recovery: None,
            include_schema: false,
            schema_verbosity: None,
            write_guidance: Default::default(),
        }))
    };

    // Refusal complement: BEFORE the refresh, the in-band create
    // still fails with SCHEMA_NOT_FOUND — this is the reported
    // five-round blockage.
    let result = mem_create("notes");
    assert!(result.is_error.unwrap_or(false));
    let envelope = result.structured_content.as_ref().unwrap();
    assert_eq!(envelope["code"], "SCHEMA_NOT_FOUND", "{envelope}");

    // The full refresh makes the schema resolvable, warm.
    let result = server.memstead_reload(Parameters(ReloadParams {
        mem: None,
        full: Some(true),
    }));
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    let text = extract_text(&result);
    assert!(text.contains("\"refresh\""), "{text}");
    assert!(text.contains("authored@0.1.0"), "{text}");

    // In-band mem create pinned to the fresh schema now succeeds…
    let result = mem_create("notes");
    assert!(
        !result.is_error.unwrap_or(false),
        "mem_create must succeed after the refresh: {result:?}"
    );

    // …and an entity lands in it. End-to-end closure, no restart.
    let result = server.memstead_create(Parameters(crate::tools::mutation::CreateParams {
        title: "First Entry".to_string(),
        entity_type: "doc".to_string(),
        mem: Some("notes".to_string()),
        sections: Some(indexmap::IndexMap::from_iter([(
            "body".to_string(),
            "written warm".to_string(),
        )])),
        metadata: None,
        relations: None,
        anchors: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "entity create into the fresh mem must succeed: {result:?}"
    );
}

/// `memstead_changes_since` end-to-end with `include_notes = false`.
/// Folder-backed mem with no changelog returns an empty
/// `changes` array but still produces a well-formed response
/// carrying `mem`, `since`, `head`.
#[test]
fn test_memstead_changes_since_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let result = server.memstead_changes_since(Parameters(ChangesSinceParams {
        mem: "specs".to_string(),
        since: memstead_base::ops::EMPTY_TREE_SHA.to_string(),
        rename_similarity: None,
        include_notes: false,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    // Unified ChangesReport shape: mem + since + head + changes
    // (notes / memstead_ref absent — those are git-branch-specific).
    assert!(text.contains("\"mem\""));
    assert!(text.contains("\"since\""));
    assert!(text.contains("\"head\""));
    assert!(text.contains("\"changes\""));
}

/// Validation envelopes from the mutation handlers carry the
/// recovery payload: UNKNOWN_SECTION ships
/// `details.declared` + `suggestion`, INVALID_REL_TYPE ships
/// `details.allowed[]` + `suggestion`, etc.
#[test]
fn test_unified_validation_envelopes_carry_recovery_payload() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // INVALID_REL_TYPE: relate with a vocabulary that doesn't
    // exist on the strict-mode default schema.
    let bad_rel = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "TOTALLY_MADE_UP_REL".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(bad_rel.is_error.unwrap_or(false));
    let body = bad_rel.structured_content.unwrap();
    assert_eq!(body["code"], "INVALID_REL_TYPE");
    assert_eq!(body["details"]["input"], "TOTALLY_MADE_UP_REL");
    assert!(
        body["details"]["allowed"].is_array(),
        "details.allowed must list the schema's declared rel types: {body}",
    );

    // UNKNOWN_SECTION: update with a section key the schema
    // does not declare.
    let entity_hash = {
        let unified = server.unified_engine().lock().unwrap();
        unified
            .get_entity(&EntityId("specs--entity-a".to_string()))
            .unwrap()
            .content_hash
            .clone()
    };
    let mut sections = IndexMap::new();
    sections.insert("totally-fake-section".to_string(), "x".to_string());
    let bad_section = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: "specs--entity-a".to_string(),
        expected_hash: Some(entity_hash),
        sections: Some(sections),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(bad_section.is_error.unwrap_or(false));
    let body = bad_section.structured_content.unwrap();
    assert_eq!(body["code"], "UNKNOWN_SECTION");
    assert_eq!(body["details"]["key"], "totally-fake-section");
    assert!(
        body["details"]["declared"].is_array(),
        "details.declared must list the schema's section keys: {body}",
    );
}

/// Mutation handlers surface typed `{code, message, details}`
/// envelopes via `engine_err_unified`. Verifies the high-value
/// error paths produce the canonical codes (HASH_MISMATCH
/// carrying current, ENTITY_NOT_FOUND carrying id).
#[test]
fn test_unified_mutation_handlers_emit_typed_error_envelopes() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // HASH_MISMATCH: update with a bogus expected_hash.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "edited".to_string());
    let bad_hash = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: "specs--entity-a".to_string(),
        expected_hash: Some("definitely-wrong".to_string()),
        sections: Some(sections),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(bad_hash.is_error.unwrap_or(false));
    let body = bad_hash.structured_content.unwrap();
    assert_eq!(body["code"], "HASH_MISMATCH");
    assert!(body["details"]["current"].is_string());

    // ENTITY_NOT_FOUND: rename a non-existent id.
    let missing = server.memstead_rename(Parameters(RenameParams {
        id: "specs--definitely-not-here".to_string(),
        new_title: "New Name".to_string(),
        expected_hash: "anything".to_string(),
        note: None,
        role: None,
        identity: None,
    }));
    assert!(missing.is_error.unwrap_or(false));
    let body = missing.structured_content.unwrap();
    assert_eq!(body["code"], "ENTITY_NOT_FOUND");
    assert!(
        body["details"]["id"]
            .as_str()
            .unwrap()
            .contains("definitely-not-here")
    );
}

/// #55: a not-found from the *generic* `engine_err_unified` mapper
/// carries the same recovery details as the dedicated handlers —
/// `suggestions` for ENTITY_NOT_FOUND, `known_mems` for
/// UNKNOWN_MEM — so the envelope is uniform regardless of which
/// internal path raised it.
#[test]
fn generic_not_found_envelopes_carry_recovery_details() {
    let (server, _tmp) = setup_dual_test_engine();

    // ENTITY_NOT_FOUND via the rename handler's generic mapper.
    let missing = server.memstead_rename(Parameters(RenameParams {
        id: "specs--definitely-not-here".to_string(),
        new_title: "X".to_string(),
        expected_hash: "anything".to_string(),
        note: None,
        role: None,
        identity: None,
    }));
    let body = missing.structured_content.unwrap();
    assert_eq!(body["code"], "ENTITY_NOT_FOUND");
    assert!(
        body["details"]["suggestions"].is_array(),
        "generic ENTITY_NOT_FOUND must carry suggestions: {body}"
    );

    // UNKNOWN_MEM via the reload handler's generic mapper.
    let bad_mem = server.memstead_reload(Parameters(ReloadParams {
        mem: Some("no-such-mem".to_string()),
        full: None,
    }));
    let body2 = bad_mem.structured_content.unwrap();
    assert_eq!(body2["code"], "UNKNOWN_MEM");
    assert!(
        body2["details"]["known_mems"].is_array(),
        "generic UNKNOWN_MEM must carry known_mems: {body2}"
    );
}

/// Text-channel mirror of the structured-error envelope: every
/// typed-error response prefixes the UPPER_SNAKE_CASE code into
/// the text channel as `ERROR [<CODE>]: <message>`. A consumer
/// that only reads `result.content[0].text` (Claude Code's
/// default rendering, CLI dump path, log scrapes) still recovers
/// the code with one regex match — the prior cycle's resolution
/// pinned the structured envelope only, leaving the text channel
/// emitting `"ERROR: <prose>"` with no programmatic handle.
/// Exercises a representative slice of the error vocabulary
/// across `memstead_update`, `memstead_relate`, `memstead_rename`,
/// `memstead_create`, and `memstead_delete` so the consolidation point
/// (`tool_error_with_payload`) is locked across every mutation
/// tool, not just one.
#[test]
fn text_channel_carries_typed_error_code_inline() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    /// Assert the text channel begins with `ERROR [<expected_code>]: `
    /// AND the structured envelope's code matches. Both halves must
    /// agree — drift between the two channels is the regression
    /// this guard catches.
    #[track_caller]
    fn assert_text_carries_code(result: &CallToolResult, expected_code: &str) {
        assert!(
            result.is_error.unwrap_or(false),
            "expected an error response, got success: {:?}",
            result.structured_content,
        );
        let text = extract_text(result);
        let prefix = format!("ERROR [{expected_code}]: ");
        assert!(
            text.starts_with(&prefix),
            "text channel must start with `{prefix}` (got: {text:?})",
        );
        let payload = result
            .structured_content
            .as_ref()
            .expect("typed error must carry structured_content");
        assert_eq!(
            payload["code"], expected_code,
            "structured `code` must match the text-channel code: {payload}",
        );
    }

    // UNKNOWN_MEM — create against a mem that isn't mounted.
    let unknown_mem = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("nonexistent-mem".to_string()),
        title: "Anything".to_string(),
        entity_type: "spec".to_string(),
        sections: None,
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert_text_carries_code(&unknown_mem, "UNKNOWN_MEM");

    // UNKNOWN_ENTITY_TYPE — create with an undeclared type.
    let unknown_type = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Misshapen".to_string(),
        entity_type: "definitely-not-a-real-type".to_string(),
        sections: None,
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert_text_carries_code(&unknown_type, "UNKNOWN_ENTITY_TYPE");

    // ENTITY_NOT_FOUND — rename a missing entity.
    let missing = server.memstead_rename(Parameters(RenameParams {
        id: "specs--definitely-not-here".to_string(),
        new_title: "New Name".to_string(),
        expected_hash: "anything".to_string(),
        note: None,
        role: None,
        identity: None,
    }));
    assert_text_carries_code(&missing, "ENTITY_NOT_FOUND");

    // ENTITY_NOT_FOUND — read a missing entity through `memstead_entity`.
    // Pre-fix the read path short-circuited through `tool_error`
    // and emitted `"ERROR: Entity not found: …"` with no code
    // prefix, leaving the documented `ERROR [<CODE>]: <message>`
    // contract broken for exactly one tool. The fix routes
    // `not_found_error` through `tool_error_with_payload` so the
    // text channel matches every other tool's not-found return.
    let missing_read = server.memstead_entity(Parameters(EntityParams {
        id: "specs--definitely-not-here".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert_text_carries_code(&missing_read, "ENTITY_NOT_FOUND");

    // HASH_MISMATCH — update with a bogus expected_hash.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "edited".to_string());
    let hash_mismatch = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: "specs--entity-a".to_string(),
        expected_hash: Some("definitely-wrong".to_string()),
        sections: Some(sections),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert_text_carries_code(&hash_mismatch, "HASH_MISMATCH");

    // INVALID_REL_TYPE — relate with a vocabulary the schema rejects.
    let bad_rel_type = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "TOTALLY_MADE_UP_REL".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert_text_carries_code(&bad_rel_type, "INVALID_REL_TYPE");

    // INVALID_ENTITY_ID — relate with a target id that violates
    // the wiki-link grammar.
    let bad_id = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--bad target with spaces!!".to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert_text_carries_code(&bad_id, "INVALID_ENTITY_ID");

    // ENTITY_ALREADY_EXISTS — create a duplicate. The engine
    // refuses on missing required sections, so the seed needs
    // identity + purpose to land before the duplicate-check fires.
    let mut dup_sections = IndexMap::new();
    dup_sections.insert("identity".to_string(), "the identity".to_string());
    dup_sections.insert("purpose".to_string(), "the purpose".to_string());
    let _seed = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Duplicate Probe".to_string(),
        entity_type: "spec".to_string(),
        sections: Some(dup_sections.clone()),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    let dup = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Duplicate Probe".to_string(),
        entity_type: "spec".to_string(),
        sections: Some(dup_sections),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert_text_carries_code(&dup, "ENTITY_ALREADY_EXISTS");
}

/// Criteria 1, 3 and 5 (consistency-sweep 03/04) on the full MCP surface.
/// An anchors-only update needs no compare-and-swap token, because the
/// token would compare a value the write provably cannot move; an update
/// that also changes content still needs one; and a payload with neither
/// anchors nor content is still an empty update.
#[test]
fn an_anchors_only_update_needs_no_expected_hash_but_a_content_change_does() {
    let (server, _tmp) = setup_test_engine();
    let create: CreateParams = serde_json::from_value(serde_json::json!({
        "mem": "specs",
        "title": "Hash Gate",
        "entity_type": "spec",
        "sections": { "identity": "id", "purpose": "purpose" },
    }))
    .unwrap();
    let created = server.memstead_create(Parameters(create));
    assert!(!created.is_error.unwrap_or(false));
    let before = extract_text(&created);

    // No `expected_hash` at all, and no read to obtain one.
    let anchors_only: UpdateParams = serde_json::from_value(serde_json::json!({
        "id": "specs--hash-gate",
        "anchors": [{
            "artifact": "src/lib.rs", "grain": "file", "class": "anchored",
            "hash": "h1", "hash_stability": "stable"
        }],
    }))
    .unwrap();
    let out = server.memstead_update(Parameters(anchors_only));
    assert!(
        !out.is_error.unwrap_or(false),
        "an anchors-only update must not demand a token for a value it cannot move: {:?}",
        out.structured_content
    );

    // Criterion 2: the content hash did not move. Compared, not merely
    // present: a first version captured `before` and threw it away, which
    // asserted nothing about the property the criterion is about.
    let hash_of = |t: &str| {
        t.lines()
            .find(|l| l.contains("_hash"))
            .map(|l| l.trim().to_string())
            .unwrap_or_default()
    };
    let after = extract_text(&out);
    assert!(!hash_of(&before).is_empty(), "the create stated a hash");
    assert_eq!(
        hash_of(&after),
        hash_of(&before),
        "an anchors-only update cannot move the content hash"
    );

    // Criterion 4: an EMPTY token is no token, not a token that can never
    // match. Accepting it here while the CLI discards it made one surface
    // refuse what the other wrote.
    let empty_token: UpdateParams = serde_json::from_value(serde_json::json!({
        "id": "specs--hash-gate",
        "expected_hash": "",
        "anchors": [{
            "artifact": "src/other.rs", "grain": "file", "class": "anchored",
            "hash": "h9", "hash_stability": "stable"
        }],
    }))
    .unwrap();
    let out_empty = server.memstead_update(Parameters(empty_token));
    assert!(
        !out_empty.is_error.unwrap_or(false),
        "an empty token on an anchors-only update is no token: {:?}",
        out_empty.structured_content
    );

    // Criterion 3: content plus anchors still demands the token.
    let with_content: UpdateParams = serde_json::from_value(serde_json::json!({
        "id": "specs--hash-gate",
        "sections": { "purpose": "changed" },
        "anchors": [{
            "artifact": "src/other.rs", "grain": "file", "class": "anchored",
            "hash": "h2", "hash_stability": "stable"
        }],
    }))
    .unwrap();
    let refused = server.memstead_update(Parameters(with_content));
    assert!(
        refused.is_error.unwrap_or(false),
        "content change without a token must refuse"
    );
    assert!(
        extract_text(&refused).contains("EXPECTED_HASH_REQUIRED"),
        "and say which field: {}",
        extract_text(&refused)
    );

    // Criterion 5: neither anchors nor content is still an empty update.
    let empty: UpdateParams =
        serde_json::from_value(serde_json::json!({ "id": "specs--hash-gate" })).unwrap();
    let empty_out = server.memstead_update(Parameters(empty));
    assert!(empty_out.is_error.unwrap_or(false));
    assert!(
        extract_text(&empty_out).contains("EMPTY_UPDATE"),
        "an empty payload is EMPTY_UPDATE, not a hash refusal: {}",
        extract_text(&empty_out)
    );
}

/// Create with `anchors[]` persists them, `memstead_entity`
/// surfaces them additively on `structured_content`, a malformed
/// anchor refuses `INVALID_ANCHOR` without writing the entity, and
/// `memstead_relate` rejects an `anchors` field (deny_unknown_fields).
#[test]
fn anchors_create_read_and_refuse_through_mcp_surface() {
    let (server, _tmp) = setup_test_engine();

    let ok: CreateParams = serde_json::from_value(serde_json::json!({
        "mem": "specs",
        "title": "Anchored Spec",
        "entity_type": "spec",
        "sections": { "identity": "id", "purpose": "purpose" },
        "anchors": [{
            "artifact": "src/lib.rs", "grain": "file", "class": "anchored",
            "hash": "h1", "hash_stability": "stable"
        }],
    }))
    .unwrap();
    let created = server.memstead_create(Parameters(ok));
    assert!(
        !created.is_error.unwrap_or(false),
        "create with anchors must succeed: {:?}",
        created.structured_content
    );

    // Read back — structured_content carries anchors + composition.
    let read = server.memstead_entity(Parameters(EntityParams {
        id: "specs--anchored-spec".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let sc = read
        .structured_content
        .expect("entity ships structured_content");
    let anchors = sc.get("anchors").expect("anchors field present");
    assert_eq!(anchors.as_array().unwrap().len(), 1);
    assert_eq!(anchors[0]["artifact"], "src/lib.rs");
    assert_eq!(anchors[0]["class"], "anchored");
    assert!(
        sc.get("anchor_composition").is_some(),
        "composition surfaces additively"
    );

    // Malformed anchor refuses INVALID_ANCHOR; entity not written.
    let bad: CreateParams = serde_json::from_value(serde_json::json!({
        "mem": "specs",
        "title": "Bad Anchor Spec",
        "entity_type": "spec",
        "sections": { "identity": "id", "purpose": "purpose" },
        "anchors": [{ "artifact": "x", "grain": "paragraph", "class": "anchored" }],
    }))
    .unwrap();
    let refused = server.memstead_create(Parameters(bad));
    assert!(refused.is_error.unwrap_or(false));
    assert_eq!(
        refused.structured_content.as_ref().unwrap()["code"],
        "INVALID_ANCHOR"
    );
    let missing = server.memstead_entity(Parameters(EntityParams {
        id: "specs--bad-anchor-spec".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(missing.is_error.unwrap_or(false), "entity was not written");

    // memstead_relate rejects an `anchors` field (deny_unknown_fields).
    let relate_parse =
        serde_json::from_value::<crate::tools::mutation::RelateParams>(serde_json::json!({
            "from": "specs--anchored-spec",
            "to": "specs--other",
            "rel_type": "REFERENCES",
            "anchors": [],
        }));
    assert!(
        relate_parse.is_err(),
        "memstead_relate must reject an `anchors` field via deny_unknown_fields"
    );
}

/// `memstead_relate(remove=true)` on a relation whose source body
/// still wiki-links the target must surface
/// `RELATION_HAS_BODY_LINKS` through the unified server's
/// envelope projection. Prior to F3 the variant fell through to
/// the wildcard `INTERNAL` arm in `engine_err_unified`, hiding
/// the typed code from agents branching on `structured_content`.
/// The filesystem-server projection already carries the variant;
/// this test exercises the mem-repo path that was diverging.
#[test]
fn relation_has_body_links_surfaces_on_unified_server() {
    let (server, _tmp) = setup_test_engine();

    let mut sections = IndexMap::new();
    // Seed
    // `identity` alongside `purpose` so the spec lands.
    sections.insert("identity".to_string(), "source identity".to_string());
    sections.insert(
        "purpose".to_string(),
        "discussion stems from [[entity-b]]".to_string(),
    );
    // Body wiki-link `[[entity-b]]` is auto-emitted as REFERENCES
    // via the alias-synthesis pass — explicit `relations:` for
    // REFERENCES is refused under the default schema's
    // `manual_authoring: forbidden` posture, so the declaration
    // stays empty and the synthesis path produces the relation.
    let created = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Body Link Source".to_string(),
        entity_type: "spec".to_string(),
        sections: Some(sections),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        !created.is_error.unwrap_or(false),
        "create must succeed: {}",
        extract_text(&created),
    );

    let remove = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--body-link-source".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "REFERENCES".to_string(),
            remove: Some(true),
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        remove.is_error.unwrap_or(false),
        "remove must refuse while body wiki-link survives: {}",
        extract_text(&remove),
    );
    let text = extract_text(&remove);
    assert!(
        text.starts_with("ERROR [RELATION_HAS_BODY_LINKS]: "),
        "text channel must carry the typed code: {text}",
    );
    let payload = remove
        .structured_content
        .as_ref()
        .expect("typed error must carry structured_content");
    assert_eq!(payload["code"], "RELATION_HAS_BODY_LINKS");
    let body_links = payload["details"]["body_links"]
        .as_array()
        .expect("details.body_links must be an array");
    assert!(
        body_links.iter().any(|v| v.as_str() == Some("purpose")),
        "details.body_links must name the surviving section: {payload}",
    );
    assert_eq!(payload["details"]["from_id"], "specs--body-link-source");
    assert_eq!(payload["details"]["to_id"], "specs--entity-b");
    assert_eq!(payload["details"]["rel_type"], "REFERENCES");
}

/// F2 + F4: write-side title length is now capped at the same
/// `memstead_base::ENTITY_ID_MAX_LEN` the read-path validator
/// enforces, so an `memstead_create` whose derived id would exceed
/// the limit refuses with an `INVALID_TITLE` envelope carrying
/// `details.length` / `details.max` for recovery. Boundary: a
/// title that lands exactly at the cap succeeds; one character
/// over fails. Read-path symmetry: the surviving accepted entity
/// is reachable via `memstead_entity` (a previously-permissive write
/// followed by a strict read would be the asymmetric bug this
/// test guards against).
#[test]
fn create_title_length_capped_in_sync_with_read_path() {
    let (server, _tmp) = setup_test_engine();
    let max = memstead_base::ENTITY_ID_MAX_LEN;
    let mem = "specs";
    // mem.len()=5 + "--"=2 ⇒ 7-char prefix. Slug-friendly title
    // entirely of letters; lowercase already, so slug == title.
    let prefix_len = mem.len() + "--".len();

    // Title whose derived id sits at the cap → accepted. Seed
    // identity + purpose so the spec lands.
    let just_fits_title = "a".repeat(max - prefix_len);
    let mut seed_sections = indexmap::IndexMap::new();
    seed_sections.insert("identity".to_string(), "the identity".to_string());
    seed_sections.insert("purpose".to_string(), "the purpose".to_string());
    let ok = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some(mem.to_string()),
        title: just_fits_title.clone(),
        entity_type: "spec".to_string(),
        sections: Some(seed_sections.clone()),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        !ok.is_error.unwrap_or(false),
        "at-cap title must succeed: {}",
        extract_text(&ok),
    );

    // Read-path symmetry — the surviving entity is reachable.
    let read = server.memstead_entity(Parameters(EntityParams {
        id: format!("{mem}--{just_fits_title}"),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(
        !read.is_error.unwrap_or(false),
        "read at the cap must succeed (write-read symmetry): {}",
        extract_text(&read),
    );

    // One character over → INVALID_TITLE envelope with recovery payload.
    let over_title = "a".repeat(max - prefix_len + 1);
    let too_long = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some(mem.to_string()),
        title: over_title.clone(),
        entity_type: "spec".to_string(),
        sections: None,
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        too_long.is_error.unwrap_or(false),
        "over-cap title must refuse",
    );
    let text = extract_text(&too_long);
    assert!(
        text.starts_with("ERROR [INVALID_TITLE]: "),
        "text channel must carry typed code: {text}",
    );
    let payload = too_long
        .structured_content
        .as_ref()
        .expect("INVALID_TITLE must carry structured_content");
    assert_eq!(payload["code"], "INVALID_TITLE");
    // The budget bounds the composed id, so the
    // payload describes the id throughout — `reason`, `input`, and
    // `length` agree (no `input.len() != length` contradiction).
    assert_eq!(payload["details"]["reason"], "id_too_long");
    assert_eq!(
        payload["details"]["length"].as_u64(),
        Some((max + 1) as u64)
    );
    assert_eq!(payload["details"]["max"].as_u64(), Some(max as u64));
    let echoed_id = format!("{mem}--{over_title}");
    assert_eq!(
        payload["details"]["input"].as_str(),
        Some(echoed_id.as_str())
    );
    assert_eq!(
        payload["details"]["input"]
            .as_str()
            .unwrap()
            .chars()
            .count() as u64,
        payload["details"]["length"].as_u64().unwrap(),
        "echoed input and reported length must measure the same quantity (the id)",
    );
}

/// F1 (B+A): non-Latin titles round-trip end-to-end through
/// `memstead_create` → store → `memstead_entity` → `memstead_relate` with
/// an Obsidian-style `[[title]]` reference. Covers three
/// non-Latin scripts (CJK,
/// End-to-end: a NUL (and other C0 control bytes) in a section
/// body is refused on both create and update with
/// `SECTION_CONTENT_INVALID`, and nothing is persisted — no binary
/// blob reaches disk. Mirrors the CLI campaign that created an
/// entity via `--from` JSON whose section content carried a raw NUL.
#[test]
fn section_body_nul_refused_on_create_and_update_nothing_persists() {
    let (server, _tmp) = setup_test_engine();

    // --- create with a NUL in a section body → refused ---
    let mut bad = indexmap::IndexMap::new();
    bad.insert("identity".to_string(), "ok".to_string());
    bad.insert("purpose".to_string(), "line1\u{0}line2".to_string());
    let create = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Nul Carrier".to_string(),
        entity_type: "spec".to_string(),
        sections: Some(bad),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        create.is_error.unwrap_or(false),
        "create with NUL must be refused"
    );
    let text = extract_text(&create);
    assert!(
        text.contains("SECTION_CONTENT_INVALID"),
        "refusal must carry SECTION_CONTENT_INVALID: {text}",
    );

    // Nothing persisted — the entity does not exist.
    let read = server.memstead_entity(Parameters(EntityParams {
        id: "specs--nul-carrier".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(
        read.is_error.unwrap_or(false),
        "refused create must not persist the entity: {}",
        extract_text(&read),
    );

    // --- update an existing clean entity with a NUL → refused ---
    let mut clean = indexmap::IndexMap::new();
    clean.insert("identity".to_string(), "ok".to_string());
    clean.insert("purpose".to_string(), "clean body".to_string());
    let ok = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Clean Carrier".to_string(),
        entity_type: "spec".to_string(),
        sections: Some(clean),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        !ok.is_error.unwrap_or(false),
        "clean create must succeed: {}",
        extract_text(&ok)
    );

    // Pass the live hash so the test exercises the section-content
    // gate, not HASH_MISMATCH (update always honours expected_hash).
    let hash = {
        let unified = server.unified_engine().lock().unwrap();
        unified
            .get_entity(&EntityId("specs--clean-carrier".to_string()))
            .unwrap()
            .content_hash
            .clone()
    };
    let mut bad_update = indexmap::IndexMap::new();
    bad_update.insert(
        "purpose".to_string(),
        "tab\tok but bell\u{7}bad".to_string(),
    );
    let update = server.memstead_update(Parameters(crate::tools::mutation::UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: "specs--clean-carrier".to_string(),
        expected_hash: Some(hash),
        sections: Some(bad_update),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: None,
        declare_relations: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        update.is_error.unwrap_or(false),
        "update with control byte must be refused"
    );
    assert!(
        extract_text(&update).contains("SECTION_CONTENT_INVALID"),
        "update refusal must carry SECTION_CONTENT_INVALID: {}",
        extract_text(&update),
    );
}

/// RTL Hebrew, Cyrillic). Verifies (a) the create succeeds,
/// (b) the id surfaces in its native script, (c) the read path
/// finds the entity at the same id, (d) a relate to a target
/// id in the same script grammar succeeds (i.e. the wiki-link
/// validator's widened character class accepts it).
#[test]
fn non_latin_titles_round_trip_create_and_relate() {
    let (server, _tmp) = setup_test_engine();

    for (title, expected_slug, label) in [
        ("日本語のタイトル", "日本語のタイトル", "CJK"),
        ("שלום עולם", "שלום-עולם", "Hebrew (no niqqud)"),
        ("Москва-проект", "москва-проект", "Cyrillic"),
    ] {
        // Seed identity + purpose so the spec lands.
        let mut native_sections = indexmap::IndexMap::new();
        native_sections.insert("identity".to_string(), "identity".to_string());
        native_sections.insert("purpose".to_string(), "purpose".to_string());
        let create = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            mem: Some("specs".to_string()),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections: Some(native_sections),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
            role: None,
            identity: None,
        }));
        assert!(
            !create.is_error.unwrap_or(false),
            "{label} create must succeed: {}",
            extract_text(&create),
        );
        let expected_id = format!("specs--{expected_slug}");
        let create_body = create
            .structured_content
            .as_ref()
            .expect("create response has structured_content");
        assert_eq!(
            create_body["id"].as_str(),
            Some(expected_id.as_str()),
            "{label} id must match the expected slug",
        );

        // Read-path symmetry: the entity is reachable at its
        // native-script id.
        let read = server.memstead_entity(Parameters(EntityParams {
            id: expected_id.clone(),
            include_relations: None,
            include_context: None,
            sections: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        assert!(
            !read.is_error.unwrap_or(false),
            "{label} read at native id must succeed: {}",
            extract_text(&read),
        );

        // Relate-path symmetry: a typed edge to this id (here,
        // we relate from the seeded `entity-a` to the new entity)
        // does not trip `INVALID_ENTITY_ID` — the wiki-link
        // grammar regex accepts the wider character class.
        let relate = server.memstead_relate(Parameters(RelateParams {
            relations: vec![RelateOpInput {
                from: "specs--entity-a".to_string(),
                to: expected_id.clone(),
                rel_type: "USES".to_string(),
                remove: None,
                description: None,
            }],
            note: None,
            role: None,
            identity: None,
            dry_run: None,
        }));
        assert!(
            !relate.is_error.unwrap_or(false),
            "{label} relate to native-script id must succeed: {}",
            extract_text(&relate),
        );
    }
}

/// F4 / F10: the strict mutation-entry gate refuses titles whose
/// alphanumeric filter would strip every character (all-emoji,
/// all-symbol, all-punctuation). Pre-gate the create used to fall
/// back to a `entity-<hash>` slug; the gate surfaces
/// `INVALID_TITLE` with `reason: empty` (everything drops, so the
/// slug derives empty). The loader-path `title_to_slug` keeps the
/// hash backstop so pre-gate entities remain readable — see
/// `entity::id::title_to_slug` unit tests.
/// The widened title grammar: a title with characters outside the
/// slug alphabet lands, the title round-trips verbatim on the
/// entity read, and the divergence rides as the typed
/// `TITLE_CHARS_DROPPED_FROM_SLUG` warning naming the dropped
/// characters and the derived slug. Complement: a clean title
/// carries no such warning.
#[test]
fn title_chars_dropped_warning_rides_create_and_clean_titles_stay_silent() {
    let (server, _tmp) = setup_test_engine();
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "I.".to_string());
    sections.insert("purpose".to_string(), "P.".to_string());
    let create = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Bösenberg Grundstücks GmbH & Co. KG".to_string(),
        entity_type: "spec".to_string(),
        sections: Some(sections.clone()),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        !create.is_error.unwrap_or(false),
        "widened grammar admits the title: {}",
        extract_text(&create),
    );
    let payload = create.structured_content.as_ref().expect("payload");
    assert_eq!(payload["id"], "specs--bösenberg-grundstücks-gmbh-co-kg");
    let warning = payload["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .find(|w| w["code"] == "TITLE_CHARS_DROPPED_FROM_SLUG")
        .expect("divergence warning present")
        .clone();
    let dropped: Vec<&str> = warning["details"]["dropped_chars"]
        .as_array()
        .expect("dropped_chars array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        dropped.contains(&"&") && dropped.contains(&"."),
        "{warning}"
    );
    assert_eq!(
        warning["details"]["slug"],
        "bösenberg-grundstücks-gmbh-co-kg"
    );
    // Verbatim title on the entity read.
    let read = server.memstead_entity(Parameters(EntityParams {
        id: "specs--bösenberg-grundstücks-gmbh-co-kg".to_string(),
        sections: None,
        token_budget: None,
        chunk: None,
        include_relations: None,
        include_context: None,
        include_provenance: None,
    }));
    let entity = read.structured_content.as_ref().expect("entity payload");
    assert_eq!(entity["title"], "Bösenberg Grundstücks GmbH & Co. KG");

    // Complement: a clean title carries no divergence warning.
    let clean = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Plain Title".to_string(),
        entity_type: "spec".to_string(),
        sections: Some(sections),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(!clean.is_error.unwrap_or(false));
    let clean_payload = clean.structured_content.as_ref().expect("payload");
    assert!(
        clean_payload["warnings"]
            .as_array()
            .map(|ws| ws
                .iter()
                .all(|w| w["code"] != "TITLE_CHARS_DROPPED_FROM_SLUG"))
            .unwrap_or(true),
        "clean title must not warn: {clean_payload}"
    );
}

#[test]
fn all_emoji_title_refuses_with_invalid_title_envelope() {
    let (server, _tmp) = setup_test_engine();
    let create = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "🚀✨".to_string(),
        entity_type: "spec".to_string(),
        sections: None,
        metadata: None,
        relations: None,
        dry_run: Some(true),
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        create.is_error.unwrap_or(false),
        "all-emoji title must refuse under the strict gate: {}",
        extract_text(&create),
    );
    let text = extract_text(&create);
    assert!(
        text.starts_with("ERROR [INVALID_TITLE]: "),
        "text channel must carry typed code: {text}",
    );
    let payload = create
        .structured_content
        .as_ref()
        .expect("INVALID_TITLE must carry structured_content");
    assert_eq!(payload["code"], "INVALID_TITLE");
    // Under the widened grammar the emoji are admitted as display
    // text, but everything drops from the slug — the refusal is
    // the empty-derivation case, not a character-class one.
    assert_eq!(payload["details"]["reason"], "empty");
}

/// F8: a title with embedded control
/// characters (tab, newline) is refused with `INVALID_TITLE` /
/// `reason: control_chars` and a `proposed_slug` — not accepted then
/// silently truncated at the newline (which split the stored `# H1`
/// and dropped every word after the newline from search). The
/// control chars are escaped in the wire payload so the JSON stays
/// single-line.
#[test]
fn control_char_title_refuses_with_invalid_title_envelope() {
    let (server, _tmp) = setup_test_engine();
    let create = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        mem: Some("specs".to_string()),
        title: "Tab\tand\nnewline title".to_string(),
        entity_type: "spec".to_string(),
        sections: None,
        metadata: None,
        relations: None,
        dry_run: Some(true),
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        create.is_error.unwrap_or(false),
        "control-char title must refuse: {}",
        extract_text(&create),
    );
    let payload = create
        .structured_content
        .as_ref()
        .expect("INVALID_TITLE must carry structured_content");
    assert_eq!(payload["code"], "INVALID_TITLE");
    assert_eq!(payload["details"]["reason"], "control_chars");
    let control_chars = payload["details"]["control_chars"]
        .as_array()
        .expect("control_chars must be an array");
    assert!(
        control_chars.iter().any(|v| v.as_str() == Some("\\t"))
            && control_chars.iter().any(|v| v.as_str() == Some("\\n")),
        "control_chars must enumerate the escaped offenders: {payload}",
    );
    assert_eq!(
        payload["details"]["proposed_slug"].as_str(),
        Some("tab-and-newline-title"),
        "proposed_slug must offer the single-line retry: {payload}",
    );
}

/// `memstead_relate` end-to-end through the unified engine. Wire
/// JSON shape (no `action` field — consumers branch on
/// `write_id.is_empty()`).
#[test]
fn test_memstead_relate_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Add path: relate two existing entities. setup_test_engine
    // seeds entity-a and entity-b, both with USES in the schema's
    // declared vocabulary.
    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "{}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    // Full-shape wire fields: from/to/rel_type/source/content_hash/write_id.
    assert!(text.contains("\"from\""));
    assert!(text.contains("\"to\""));
    assert!(text.contains("\"rel_type\""));
    assert!(text.contains("\"source\": \"explicit\""));
    assert!(text.contains("\"_hash\""));
    assert!(text.contains("\"write_id\""));
    // Schema anchor injected via mem_schema_ref_unified.
    assert!(text.contains("\"_mem_schema\""));

    // Stub-creation path: relate to a non-existent target. The
    // unified engine creates the stub and surfaces it as an
    // `AUTO_STUB_CREATED` entry in `warnings[]` (Item 03 retired
    // the bespoke top-level `stub_warning` field). The text body
    // must carry both the code and the stub id so an agent
    // reading the text channel can recover the recovery payload
    // without parsing structured_content.
    let stub_relate = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--ghost-x".to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(!stub_relate.is_error.unwrap_or(false));
    let stub_text = extract_text(&stub_relate);
    assert!(
        stub_text.contains("\"AUTO_STUB_CREATED\""),
        "AUTO_STUB_CREATED code must surface in warnings[]: {stub_text}",
    );
    assert!(stub_text.contains("ghost-x"));
    // Old top-level field must be gone — uniform diagnostic shape.
    assert!(
        !stub_text.contains("\"stub_warning\""),
        "stub_warning field must not appear on the wire: {stub_text}",
    );
}

/// `memstead_update` end-to-end through the unified engine on a
/// section-replace request. Wire JSON ships nested
/// `modified_sections` / `modified_metadata` envelopes.
#[test]
fn test_memstead_update_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Read entity-a's hash through the unified engine.
    let entity_hash = {
        let unified = server.unified_engine().lock().unwrap();
        unified
            .get_entity(&EntityId("specs--entity-a".to_string()))
            .expect("entity-a must exist")
            .content_hash
            .clone()
    };

    // Section-replace path (the unified-supported subset).
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "edited body".to_string());
    let result = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: "specs--entity-a".to_string(),
        expected_hash: Some(entity_hash),
        sections: Some(sections),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "{}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    // Full-shape wire fields: id, title, nested
    // modified_sections/modified_metadata, content_hash,
    // write_id, _mem_schema.
    assert!(text.contains("\"id\""));
    assert!(text.contains("\"title\""));
    assert!(text.contains("\"modified_sections\""));
    assert!(text.contains("\"replaced\""));
    assert!(text.contains("\"identity\""));
    assert!(text.contains("\"modified_metadata\""));
    assert!(text.contains("\"_hash\""));
    assert!(text.contains("\"write_id\""));
    assert!(text.contains("\"_mem_schema\""));
    // Empty append/patch slots stripped to match full's
    // skip_serializing_if convention.
    assert!(
        !text.contains("\"appended\""),
        "empty appended must be stripped",
    );
    assert!(
        !text.contains("\"patched\""),
        "empty patched must be stripped",
    );
}

/// `memstead_create` end-to-end through the unified engine.
/// Asserts the `CreateResult` wire shape for the greenfield +
/// stub-adoption paths.
#[test]
fn test_memstead_create_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Greenfield create — no relations, no dry_run, no
    // pre-existing stub. Routes through the unified engine.
    // The
    // engine refuses on missing required sections; seed both.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "the identity".to_string());
    sections.insert("purpose".to_string(), "the purpose".to_string());
    let result = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        title: "Brand New Spec".to_string(),
        entity_type: "spec".to_string(),
        mem: None,
        sections: Some(sections),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "{}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    // Full-shape wire fields: id, title, mem, file_path,
    // created_date, content_hash, write_id, _mem_schema.
    assert!(text.contains("\"id\""));
    assert!(text.contains("\"title\": \"Brand New Spec\""));
    assert!(text.contains("\"mem\": \"specs\""));
    assert!(text.contains("\"file_path\": \"brand-new-spec.md\""));
    assert!(text.contains("\"created_date\""));
    assert!(text.contains("\"_hash\""));
    assert!(text.contains("\"write_id\""));
    assert!(text.contains("\"_mem_schema\""));
    // Greenfield: incoming_count + incoming skip-serialised.
    assert!(
        !text.contains("\"incoming_count\""),
        "greenfield create must skip-empty incoming_count",
    );

    // dry_run path falls back to the full engine — verified by
    // observing the response carries the same full shape but the
    // unified engine wasn't touched (we'd see two entities if
    // it had been). With no full mem wired, dry_run on the
    // unified branch is unreachable in this test fixture; we
    // skip exercising it here (covered by full-side tests).

    // Stub-adoption path: pre-existing stub created via
    // memstead_relate, then memstead_create at the same id promotes it.
    // Use a separate title that slugifies to a new id, then
    // a relate to a different stub target.
    let stub_id = "specs--future-decision";
    let _stub_relate = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: stub_id.to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    // Seed
    // identity + purpose so the spec lands; stub adoption still
    // surfaces via the typed `incoming` shape below.
    let mut adopt_sections = IndexMap::new();
    adopt_sections.insert("identity".to_string(), "future identity".to_string());
    adopt_sections.insert("purpose".to_string(), "future purpose".to_string());
    let adopt = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        title: "Future Decision".to_string(),
        entity_type: "spec".to_string(),
        mem: None,
        sections: Some(adopt_sections),
        metadata: None,
        relations: None,
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(!adopt.is_error.unwrap_or(false), "{}", extract_text(&adopt));
    let adopt_text = extract_text(&adopt);
    // Stub adoption: incoming_count + incoming surface the
    // adopted edge from entity-a.
    assert!(adopt_text.contains("\"incoming_count\": 1"));
    assert!(adopt_text.contains("\"from\""));
    assert!(adopt_text.contains("entity-a"));
}

/// `memstead_retype` end-to-end through the unified engine: the
/// same outcome the CLI verb produces — type and mapped sections
/// rewritten, id and path unchanged, `checks_stale` stated — and a
/// typed report-all refusal before it, over the same wire envelope.
#[test]
fn test_memstead_retype_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);
    let hash_of = |id: &str| {
        let unified = server.unified_engine().lock().unwrap();
        unified
            .get_entity(&EntityId(id.to_string()))
            .expect("entity must exist")
            .content_hash
            .clone()
    };

    // Report-all refusal: a spec's `level` field is not a memo
    // field, and its `identity`/`purpose` sections are not memo
    // sections — both classes arrive in one typed envelope.
    let refused = server.memstead_retype(Parameters(RetypeParams {
        id: "specs--entity-b".to_string(),
        target_type: "memo".to_string(),
        section_map: None,
        drop_metadata: None,
        expected_hash: Some(hash_of("specs--entity-b")),
        dry_run: None,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(refused.is_error.unwrap_or(false));
    let body = refused.structured_content.clone().unwrap();
    assert_eq!(body["code"], "UNKNOWN_SECTION", "{body}");
    let codes: Vec<&str> = body["details"]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"UNKNOWN_SECTION"), "{codes:?}");
    assert!(codes.contains(&"MISSING_REQUIRED_SECTION"), "{codes:?}");
    assert!(codes.contains(&"UNKNOWN_METADATA_FIELD"), "{codes:?}");
    assert_eq!(
        body["details"]["proposed_section_map"]["identity"],
        "substance"
    );
    assert!(body["details"]["target_sections"].is_array());

    // The spec-only `level` field is dropped explicitly on the way.
    let result = server.memstead_retype(Parameters(RetypeParams {
        id: "specs--entity-b".to_string(),
        target_type: "memo".to_string(),
        section_map: Some(
            [
                ("identity".to_string(), "claim".to_string()),
                ("purpose".to_string(), "context".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        drop_metadata: Some(vec!["level".to_string()]),
        expected_hash: Some(hash_of("specs--entity-b")),
        dry_run: None,
        note: Some("retype over MCP".to_string()),
        role: None,
        identity: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "{}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    assert!(text.contains("\"old_type\": \"spec\""), "{text}");
    assert!(text.contains("\"new_type\": \"memo\""), "{text}");
    assert!(text.contains("\"checks_stale\": true"), "{text}");
    assert!(text.contains("\"staleness_note\""), "{text}");
    assert!(text.contains("\"_hash\""), "{text}");
    assert!(text.contains("\"_mem_schema\""), "{text}");
    {
        let unified = server.unified_engine().lock().unwrap();
        let e = unified
            .get_entity(&EntityId("specs--entity-b".to_string()))
            .expect("still there under the same id");
        assert_eq!(e.entity_type, "memo");
        assert_eq!(e.file_path, "entity-b.md");
        assert!(e.sections.contains_key("claim"));
        assert!(e.sections.contains_key("context"));
        // The incoming USES edge from entity-a stays.
        assert_eq!(
            unified
                .store()
                .incoming(&EntityId("specs--entity-b".to_string()))
                .len(),
            1
        );
    }
}

/// `memstead_delete` end-to-end through the unified engine. Wire
/// JSON ships `id` / `relations_removed` / `write_id` and
/// skips `file_path` / `removed_incoming`.
#[test]
fn test_memstead_delete_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Read entity-a's hash through the unified engine so the
    // hash check passes.
    let entity_hash = {
        let unified = server.unified_engine().lock().unwrap();
        unified
            .get_entity(&EntityId("specs--entity-a".to_string()))
            .expect("entity-a must exist")
            .content_hash
            .clone()
    };

    let result = server.memstead_delete(Parameters(DeleteParams {
        id: "specs--entity-a".to_string(),
        expected_hash: entity_hash,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "{}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    // Full-shape wire fields: id + relations_removed + write_id.
    assert!(text.contains("\"id\""));
    assert!(text.contains("\"relations_removed\""));
    assert!(text.contains("\"write_id\""));
    // Unified-only fields stay engine-side, not wire.
    assert!(
        !text.contains("\"file_path\""),
        "wire shape must skip file_path (full DeleteResult omits)",
    );
    assert!(
        !text.contains("\"removed_incoming\""),
        "wire shape must skip removed_incoming",
    );
    // Schema anchor injected.
    assert!(text.contains("\"_mem_schema\""));

    // Confirm the entity is gone from the unified store.
    {
        let unified = server.unified_engine().lock().unwrap();
        assert!(
            unified
                .get_entity(&EntityId("specs--entity-a".to_string()))
                .is_none(),
            "entity-a must be deleted from the unified store"
        );
    }
}

/// `memstead_rename` end-to-end through the unified engine.
/// Outcome carries `old_path` / `new_path` directly; slug-noop
/// surfaces as `TitleNormalizedToSlugNoop`.
#[test]
fn test_memstead_rename_via_unified_engine_path() {
    let tmp = setup_test_workspace();
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Read the entity through the unified engine to grab its
    // current content_hash (handler requires expected_hash).
    let entity_hash = {
        let unified = server.unified_engine().lock().unwrap();
        unified
            .get_entity(&EntityId("specs--entity-a".to_string()))
            .expect("entity-a must exist in fixture")
            .content_hash
            .clone()
    };

    // Real-rename path.
    let result = server.memstead_rename(Parameters(RenameParams {
        id: "specs--entity-a".to_string(),
        new_title: "Renamed Entity A".to_string(),
        expected_hash: entity_hash.clone(),
        note: None,
        role: None,
        identity: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "{}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    // Full-shape wire fields: old_id/new_id/old_path/new_path/
    // content_hash/write_id. Schema anchor present.
    assert!(text.contains("\"old_id\""));
    assert!(text.contains("\"new_id\""));
    assert!(text.contains("\"old_path\""));
    assert!(text.contains("\"new_path\""));
    assert!(text.contains("\"_hash\""));
    assert!(text.contains("\"write_id\""));
    assert!(text.contains("\"_mem_schema\""));
    // The new title's slug should appear in the new id/path.
    assert!(text.contains("renamed-entity-a"));

    // Slug-noop path: rename entity-b to a title that normalises
    // to the same slug. Hash is fresh from the unified store
    // (entity-b unchanged by the previous rename).
    let entity_b_hash = {
        let unified = server.unified_engine().lock().unwrap();
        unified
            .get_entity(&EntityId("specs--entity-b".to_string()))
            .expect("entity-b must exist in fixture")
            .content_hash
            .clone()
    };
    let noop = server.memstead_rename(Parameters(RenameParams {
        id: "specs--entity-b".to_string(),
        new_title: "Entity  B".to_string(), // collapses to entity-b
        expected_hash: entity_b_hash,
        note: None,
        role: None,
        identity: None,
    }));
    assert!(!noop.is_error.unwrap_or(false), "{}", extract_text(&noop));
    let noop_text = extract_text(&noop);
    // Slug-noop wire shape: old_id == new_id, write_id empty,
    // warnings carries TITLE_NORMALIZED_TO_SLUG_NOOP.
    assert!(noop_text.contains("\"write_id\": \"\""));
    assert!(noop_text.contains("TITLE_NORMALIZED_TO_SLUG_NOOP"));
}

#[test]
fn test_memstead_entity_with_sections_filter() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_entity(Parameters(EntityParams {
        id: "specs--entity-a".to_string(),
        include_relations: None,
        include_context: None,
        sections: Some(vec!["identity".to_string()]),
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let text = extract_text(&result);
    assert!(text.contains("## Identity"));
    // Purpose section should not appear since we only asked for identity
    assert!(!text.contains("## Purpose"));
}

#[test]
fn entity_with_include_relations_renders_markdown_section() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_entity(Parameters(EntityParams {
        id: "specs--entity-a".to_string(),
        include_relations: Some(true),
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let text = extract_text(&result);
    assert!(
        text.contains("## Relations"),
        "expected `## Relations` heading in entity output"
    );
    assert!(
        !text.contains("## Relations (JSON)"),
        "old JSON-code-block render form must be gone"
    );
    assert!(
        text.contains("### Outgoing") || text.contains("(no relations"),
        "expected outgoing subsection or the empty-relations marker"
    );
    // Structured envelope
    // populated alongside the text channel. The text channel is
    // the rendered markdown (asserted above); the structured
    // envelope carries the typed Entity shape — agents branch on
    // it without parsing the text channel.
    let sc = result
        .structured_content
        .as_ref()
        .expect("memstead_entity must populate structured_content");
    assert!(sc.get("_hash").and_then(|v| v.as_str()).is_some());
    assert!(sc.get("relationships").and_then(|v| v.as_array()).is_some());
}

#[test]
fn entity_with_include_context_appends_community_section() {
    let (server, _tmp) = setup_dual_test_engine();
    // Build a community cache first so context has something to surface.
    let _ = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let result = server.memstead_entity(Parameters(EntityParams {
        id: "specs--entity-a".to_string(),
        include_relations: None,
        include_context: Some(true),
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let text = extract_text(&result);
    assert!(
        text.contains("## Community Context"),
        "expected `## Community Context` heading in entity output, got: {text}"
    );
}

#[test]
fn entity_with_both_flags_returns_all_sections() {
    let (server, _tmp) = setup_dual_test_engine();
    let _ = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let result = server.memstead_entity(Parameters(EntityParams {
        id: "specs--entity-a".to_string(),
        include_relations: Some(true),
        include_context: Some(true),
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let text = extract_text(&result);
    assert!(text.contains("# Entity A"));
    assert!(text.contains("## Relations"));
    assert!(text.contains("## Community Context"));
}

#[test]
fn test_memstead_search_text() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_search(Parameters(SearchParams {
        query: Some(Query {
            any: vec!["Entity".into(), "A".into()],
            ..Default::default()
        }),
        direction: None,
        mem: None,
        entity_type: None,
        expand_via: None,
        expand_depth: None,
        related_to: None,
        depth: None,
        edge_type: None,
        limit: None,
        offset: None,
        filters: None,
        range_filters: None,
        stub: None,
        token_budget: None,
    }));
    let text = extract_text(&result);
    assert!(text.contains("_total:"));
    assert!(text.contains("entity-a"));
}

/// With no `query`, `memstead_search` behaves as a pure
/// metadata/structural filter — the path that replaces the removed
/// `memstead_list` tool.
#[test]
fn test_memstead_search_no_text_filters_by_schema() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_search(Parameters(SearchParams {
        query: None,
        mem: None,
        entity_type: Some("spec".to_string()),
        expand_via: None,
        expand_depth: None,
        related_to: None,
        depth: None,
        edge_type: None,
        limit: None,
        offset: None,
        filters: None,
        range_filters: None,
        stub: None,
        token_budget: None,
        direction: None,
    }));
    let text = extract_text(&result);
    assert!(text.contains("_total:"));
    assert!(text.contains("_total_tokens:"));
}

/// Both text-query and no-text paths must populate `structured_content`
/// with precomputed summary fields. Absorbed memstead_list's coverage.
#[test]
fn test_memstead_search_markdown_shape_in_both_modes() {
    let (server, _tmp) = setup_dual_test_engine();

    // Text-query mode — at least one hit, markdown surfaces every
    // per-result and per-hit field the tool description promises.
    let search = server.memstead_search(Parameters(SearchParams {
        query: Some(Query {
            any: vec!["Entity".into(), "A".into()],
            ..Default::default()
        }),
        direction: None,
        mem: None,
        entity_type: None,
        expand_via: None,
        expand_depth: None,
        related_to: None,
        depth: None,
        edge_type: None,
        limit: None,
        offset: None,
        filters: None,
        range_filters: None,
        stub: None,
        token_budget: None,
    }));
    // Structured envelope
    // populated on the wire — the test channel below still pins
    // the markdown render shape (rendered prose with frontmatter
    // markers) so consumers of the text channel see the same
    // human-readable form as before.
    let sc = search
        .structured_content
        .as_ref()
        .expect("memstead_search must populate structured_content");
    assert!(sc.get("_total").and_then(|v| v.as_u64()).is_some());
    assert!(sc.get("hits").and_then(|v| v.as_array()).is_some());
    let text = extract_text(&search);
    assert!(
        text.contains("_total:"),
        "frontmatter must carry _total; got:\n{text}"
    );
    assert!(
        text.contains("_offset: 0"),
        "frontmatter must carry _offset; got:\n{text}"
    );
    assert!(
        text.contains("_total_tokens:"),
        "frontmatter must carry _total_tokens; got:\n{text}"
    );
    // At least one `### <id> — <title> (_score: ..., _tokens: ...)` heading.
    assert!(
        text.contains("### specs--") || text.contains("### memos--"),
        "expected at least one hit heading in markdown; got:\n{text}"
    );
    // One of the summary labels for spec/memo/concept appears.
    assert!(
        text.contains("**Identity**")
            || text.contains("**Claim**")
            || text.contains("**Definition**"),
        "expected schema-driven summary label in markdown; got:\n{text}"
    );

    // Filter-only mode — absorbed-list path.
    let filter_only = server.memstead_search(Parameters(SearchParams {
        query: None,
        mem: None,
        entity_type: Some("spec".to_string()),
        expand_via: None,
        expand_depth: None,
        related_to: None,
        depth: None,
        edge_type: None,
        limit: None,
        offset: None,
        filters: None,
        range_filters: None,
        stub: None,
        token_budget: None,
        direction: None,
    }));
    let filter_sc = filter_only
        .structured_content
        .as_ref()
        .expect("filter-only memstead_search must populate structured_content");
    assert!(filter_sc.get("_total").and_then(|v| v.as_u64()).is_some());
    let filter_text = extract_text(&filter_only);
    assert!(filter_text.contains("_total:"));
    assert!(filter_text.contains("_total_tokens:"));
    assert!(
        filter_text.contains("### specs--"),
        "expected at least one spec hit heading; got:\n{filter_text}"
    );
}

#[test]
fn test_memstead_search_no_text_pagination() {
    let (server, _tmp) = setup_dual_test_engine();

    // First page: limit=1, offset=0
    let page1 = extract_text(&server.memstead_search(Parameters(SearchParams {
        query: None,
        mem: None,
        entity_type: Some("spec".to_string()),
        expand_via: None,
        expand_depth: None,
        related_to: None,
        depth: None,
        edge_type: None,
        limit: Some(1),
        offset: Some(0),
        filters: None,
        range_filters: None,
        stub: None,
        token_budget: None,
        direction: None,
    })));
    assert!(page1.contains("_returned: 1"));
    assert!(page1.contains("_offset: 0"));

    // Second page: limit=1, offset=1
    let page2 = extract_text(&server.memstead_search(Parameters(SearchParams {
        query: None,
        mem: None,
        entity_type: Some("spec".to_string()),
        expand_via: None,
        expand_depth: None,
        related_to: None,
        depth: None,
        edge_type: None,
        limit: Some(1),
        offset: Some(1),
        filters: None,
        range_filters: None,
        stub: None,
        token_budget: None,
        direction: None,
    })));
    assert!(page2.contains("_returned: 1"));
    assert!(page2.contains("_offset: 1"));

    // With no text, all scores tie at 0.0; stable-sort falls back to
    // title-ascending — entity-a before entity-b.
    let has_a_p1 = page1.contains("specs--entity-a");
    let has_a_p2 = page2.contains("specs--entity-a");
    assert_ne!(has_a_p1, has_a_p2, "Pages should not overlap");
}

/// `memstead_search` end-to-end through the unified engine.
#[test]
fn test_memstead_search_via_unified_engine_path() {
    let tmp = setup_test_workspace();

    // Construct a unified engine reading the same mem directory
    // the full engine reads. The folder backend trait impl on
    // FilesystemBackend walks the mem tree on `from_mounts`,
    // populating the unified store with the same entities full
    // already loaded.
    let mem_dir = tmp.path().join("specs");
    let writer = memstead_base::storage::FilesystemBackend::new(mem_dir.clone());
    let mount = memstead_base::Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::MountStorage::Folder { path: mem_dir },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let unified = memstead_base::Engine::from_mounts(vec![(
        mount,
        Box::new(writer) as Box<dyn memstead_base::backend::MemBackend>,
    )])
    .unwrap();
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // The handler now reads through the unified engine.
    let result = server.memstead_search(Parameters(SearchParams {
        query: Some(Query {
            any: vec!["Entity".into(), "A".into()],
            ..Default::default()
        }),
        direction: None,
        mem: None,
        entity_type: None,
        expand_via: None,
        expand_depth: None,
        related_to: None,
        depth: None,
        edge_type: None,
        limit: None,
        offset: None,
        filters: None,
        range_filters: None,
        stub: None,
        token_budget: None,
    }));
    let text = extract_text(&result);
    // Same shape and content as the legacy-path test
    // `test_memstead_search_text` — confirms behavioural equivalence.
    assert!(text.contains("_total:"));
    assert!(text.contains("entity-a"));
}

#[test]
fn test_memstead_overview() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let text = extract_text(&result);
    assert!(text.contains("_cluster_count:"));
}

/// Overview carries schemas, mems, communities, budget metadata
/// as Markdown (no JSON sidecar). Schema bodies live on
/// `memstead_schema(name=...)` — overview lists `{ref, description}` only.
#[test]
fn overview_includes_schemas_mems_communities() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(16000),
    }));
    assert!(
        result.structured_content.is_none(),
        "read tools must not emit structured_content on success"
    );
    let parsed = ParsedOverview::from(&result);

    // Schemas block — one writable mem → one schema entry.
    assert_eq!(parsed.schema_refs(), vec!["default@1.0.0".to_string()]);

    // Schema bodies are no longer in overview — the lite catalogue
    // names the schema and points at memstead_schema for full bodies.
    assert!(
        parsed.text.contains("memstead_schema(name="),
        "Schemas block must point at the new memstead_schema reader; got:\n{}",
        parsed.text
    );
    assert!(
        !parsed.text.contains("**Types:**"),
        "Types must NOT render under overview's Schemas block — full bodies live on memstead_schema; got:\n{}",
        parsed.text
    );
    assert!(
        !parsed.text.contains("**Relationships:**"),
        "Relationship vocabulary must NOT render under overview — call memstead_schema; got:\n{}",
        parsed.text
    );

    // Mems block.
    assert_eq!(parsed.mem_names(), vec!["specs".to_string()]);
    assert!(parsed.text.contains("- **Schema:** default@1.0.0"));
    // F1: per-mem `version` surfaces under the Mems block so
    // an agent reading the overview sees the publish version
    // without a separate `memstead_health include_config` round-trip.
    // The test fixture seeds `version: "0.1.0"`.
    assert!(
        parsed.text.contains("- **Version:** 0.1.0"),
        "Mems block must surface the per-mem version; got:\n{}",
        parsed.text,
    );

    // Budget + mode.
    assert_eq!(parsed.overview_mode(), "complete");
    assert!(parsed.budget_used() > 0);
    assert!(parsed.text.contains("_cluster_count:"));
}

/// Cold-start path: empty graph, but the schema block must still be full.
/// Guards against the failure mode where an agent hits a fresh mem, has
/// nothing to read-before-write, and lacks any template to author from.
#[test]
fn overview_on_empty_graph_still_returns_schema() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("empty");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    // See `overview_includes_schemas_mems_communities` for the
    // budget rationale.
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(16000),
    }));
    assert!(result.structured_content.is_none());
    let parsed = ParsedOverview::from(&result);

    assert_eq!(parsed.schema_refs(), vec!["default@1.0.0".to_string()]);
    // Schema bodies (types, sections) live on memstead_schema; overview
    // surfaces the catalogue pointer so the agent knows where to drill.
    assert!(
        parsed.text.contains("memstead_schema(name="),
        "empty-graph schema catalogue must still point at memstead_schema; got:\n{}",
        parsed.text
    );

    assert_eq!(parsed.mem_names(), vec!["empty".to_string()]);
    assert!(parsed.text.contains("- **Entities:** 0"));

    // Empty graph: the heavy-content pool is trivial — nothing to drop.
    assert_eq!(parsed.overview_mode(), "complete");
    assert!(
        parsed.hint_keys().is_empty(),
        "empty graph must not produce hints; got {:?}",
        parsed.hint_keys()
    );
}

/// Two writable mems pinned to the same schema collapse into one
/// `schemas[]` entry — deduplicated by `ref`, with `used_by` carrying
/// both mem names in sorted order. Keeps the cold-start payload
/// tractable when a workspace has many mems on a shared schema.
#[test]
fn overview_dedups_shared_schema() {
    let tmp = TempDir::new().unwrap();

    let alpha_dir = tmp.path().join("alpha");
    fs::create_dir_all(alpha_dir.join(".memstead")).unwrap();
    fs::write(
        alpha_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    let beta_dir = tmp.path().join("beta");
    fs::create_dir_all(beta_dir.join(".memstead")).unwrap();
    fs::write(
        beta_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    let _ = (alpha_dir, beta_dir);
    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    assert!(result.structured_content.is_none());
    let parsed = ParsedOverview::from(&result);

    assert_eq!(
        parsed.schema_refs(),
        vec!["default@1.0.0".to_string()],
        "two mems on the same schema → exactly one schema heading"
    );
    // `used_by` left overview's `## Schemas` section with the schema-tool
    // split; both mems still appear under `## Mems`, and an agent
    // resolves the pinning by calling `memstead_schema(name=default@1.0.0)`.
    assert!(
        !parsed.text.contains("**Used by:**"),
        "Used by must NOT render in overview's schema entries; got:\n{}",
        parsed.text
    );
    assert_eq!(
        parsed.mem_names(),
        vec!["alpha".to_string(), "beta".to_string()],
        "both writable mems must appear"
    );
    assert!(!parsed.overview_mode().is_empty());
}

/// Build a two-mem engine (alpha + beta, both pinned to `default@1.0.0`)
/// with one entity each — enough to exercise the `mem` filter
/// without fighting per-test fixture boilerplate. Returns the server
/// plus the tempdir so callers can keep it alive for the duration
/// of the test.
fn setup_two_mem_engine() -> (McpServer, TempDir) {
    let tmp = TempDir::new().unwrap();

    let alpha_dir = tmp.path().join("alpha");
    fs::create_dir_all(alpha_dir.join(".memstead")).unwrap();
    fs::write(
        alpha_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            alpha_dir.join("alpha-root.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Alpha Root\n\n## Identity\n\nAlpha mem's seed entity.\n\n## Purpose\n\nFilter test fixture.\n",
        )
        .unwrap();

    let beta_dir = tmp.path().join("beta");
    fs::create_dir_all(beta_dir.join(".memstead")).unwrap();
    fs::write(
        beta_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            beta_dir.join("beta-root.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Beta Root\n\n## Identity\n\nBeta mem's seed entity.\n\n## Purpose\n\nFilter test fixture.\n",
        )
        .unwrap();

    let _ = (alpha_dir, beta_dir);
    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());

    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );
    (server, tmp)
}

/// `mem` filter narrows `mems[]` to one entry and `schemas[]` to the
/// ref that mem uses — but `used_by` inside each schema still lists
/// every mem sharing it, so the agent keeps global context.
#[test]
fn overview_filtered_to_one_mem() {
    let (server, _tmp) = setup_two_mem_engine();

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: Some("alpha".to_string()),
        include: None,
        token_budget: None,
    }));
    assert!(result.structured_content.is_none());
    let parsed = ParsedOverview::from(&result);

    // Filtered mem — only `alpha` appears in the mem block.
    assert_eq!(parsed.mem_names(), vec!["alpha".to_string()]);
    assert_eq!(parsed.schema_refs(), vec!["default@1.0.0".to_string()]);
    // `used_by` moved to memstead_schema's response; overview no longer
    // ships it. Agents fetch the pinning list with one
    // `memstead_schema(name=default@1.0.0)` call.
    assert!(
        !parsed.text.contains("**Used by:**"),
        "Used by must NOT render in overview's schema entries; got:\n{}",
        parsed.text
    );

    // community_bridges is in the default greedy-fill pool; on this
    // fixture there are no cross-cluster edges, so the bridges block is
    // absent entirely.
    assert!(
        parsed.bridge_headings().is_empty(),
        "no edges → no bridges on this fixture; got {:?}",
        parsed.bridge_headings()
    );
}

// ----------------------------------------------------------------------
// Budget-driven overview coverage.
// ----------------------------------------------------------------------

/// Build a two-mem engine then establish a cross-mem edge via
/// `memstead_relate` so community detection treats the two mems as
/// separate clusters bridged by an inter-cluster edge. Returns the
/// server plus the tempdir.
///
/// Note: `memstead_relate` rejects cross-mem edges, so for these tests
/// we seed two entities inside one mem but in structurally-distinct
/// Louvain neighbourhoods by wiring many same-mem leaves to each
/// root. Louvain produces two clusters, the single root↔root edge is
/// inter-cluster, and `community_bridges` picks it up.
fn setup_bridge_engine() -> (McpServer, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("bridge");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    // Two dense hub clusters: alpha-hub has 4 alpha-leaves linked to it;
    // beta-hub has 4 beta-leaves linked to it; alpha-hub USES beta-hub.
    let fm =
        "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n";
    for (name, rels) in [
        ("alpha-hub", "- **USES**: [[beta-hub]]\n"),
        ("beta-hub", ""),
    ] {
        let body = format!(
            "{fm}# {name}\n\n## Identity\n\nHub {name}.\n\n## Purpose\n\nBridge fixture.\n\n## Relationships\n\n{rels}"
        );
        fs::write(mem_dir.join(format!("{name}.md")), body).unwrap();
    }
    for side in ["alpha", "beta"] {
        for i in 0..4 {
            let body = format!(
                "{fm}# {side} leaf {i}\n\n## Identity\n\nLeaf {i} of {side}.\n\n## Purpose\n\nCluster filler.\n\n## Relationships\n\n- **USES**: [[{side}-hub]]\n"
            );
            fs::write(mem_dir.join(format!("{side}-leaf-{i}.md")), body).unwrap();
        }
    }

    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());

    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );
    (server, tmp)
}

/// Default budget resolves to 8000 when the caller omits
/// `token_budget` — surfaced in frontmatter.
#[test]
fn overview_budget_default_uses_8000() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: None,
    }));
    let parsed = ParsedOverview::from(&result);
    assert_eq!(parsed.budget_requested(), 8000);
}

/// Caller-supplied `token_budget` is echoed verbatim in frontmatter.
#[test]
fn overview_budget_respects_user_override() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(2000),
    }));
    let parsed = ParsedOverview::from(&result);
    assert_eq!(parsed.budget_requested(), 2000);
}

/// Small graph fits under the default budget — no hints, every
/// heavy block rendered.
#[test]
fn overview_small_graph_under_budget_ships_complete() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(16000),
    }));
    let parsed = ParsedOverview::from(&result);
    assert_eq!(parsed.overview_mode(), "complete");
    assert!(parsed.hint_keys().is_empty(), "no hints when complete");
    // Schema bodies (types, sections, write_rules) left overview with
    // the schema-tool split — overview lists schemas as
    // `{ref, description}` only and points at memstead_schema for bodies.
    assert!(
        !parsed.text.contains("**Types:**"),
        "Types must NOT render under overview — full bodies live on memstead_schema; got:\n{}",
        parsed.text
    );
    assert!(
        parsed.text.contains("memstead_schema(name="),
        "Schemas block must point at the new memstead_schema reader; got:\n{}",
        parsed.text
    );
    // mem_distribution shipped → per-mem `By type` row renders.
    assert!(
        parsed.text.contains("**By type:**"),
        "mem_distribution shipped ⇒ 'By type' row; got:\n{}",
        parsed.text
    );
    assert!(parsed.budget_used() <= 16000);
}

/// Tight budget forces heavy keys (community_members, mem_distribution,
/// community_bridges, dangling_links) to drop into `## Hints`. Schema
/// bodies are no longer in this set — they live on `memstead_schema`.
#[test]
fn overview_large_graph_over_budget_reduces_with_hints() {
    let (server, _tmp) = setup_dual_test_engine();
    // Budget of 30 is below community_members + mem_distribution costs
    // on the test fixture, but slim schema list + mem roster must
    // always ship as hard-required.
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(30),
    }));
    let parsed = ParsedOverview::from(&result);

    // overview_mode is `reduced` when hard-required content fits and
    // some heavy keys were dropped; `overbudget` if even hard-required
    // exceeded the budget.
    let mode = parsed.overview_mode();
    assert!(mode == "reduced" || mode == "overbudget", "got mode={mode}");

    let hints = parsed.hint_keys();
    assert!(
        !hints.is_empty(),
        "tight budget must produce hints; got:\n{}",
        parsed.text
    );

    // Hard-required content still ships even in overbudget mode.
    assert!(
        !parsed.schema_refs().is_empty(),
        "schemas block must survive tight budget; got:\n{}",
        parsed.text
    );
}

/// `include` forces a heavy key even when budget would have dropped it.
/// Schema bodies are no longer in the heavy-key set; we exercise
/// `community_members` instead — same forcing semantics.
#[test]
fn overview_include_overrides_budget() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["community_members".to_string()]),
        token_budget: Some(50),
    }));
    let parsed = ParsedOverview::from(&result);

    // community_members forced → cluster member ids render even though
    // the budget would have dropped them. The fallback marker is
    // absent because the section landed.
    assert!(
        !parsed
            .text
            .contains("call with include=[\"community_members\"] to see member lists"),
        "community_members forced ⇒ fallback marker must be absent; got:\n{}",
        parsed.text
    );
}

/// Overview rejects the legacy `schema_types` include key with a
/// typed `INVALID_INPUT` envelope that names the new tool. Pre-release
/// breaking change — agents update to `memstead_schema(name=...)`.
#[test]
fn overview_rejects_legacy_schema_types_include() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["schema_types".to_string()]),
        token_budget: None,
    }));
    assert_eq!(result.is_error, Some(true), "schema_types must error");
    let sc = result
        .structured_content
        .as_ref()
        .expect("error envelope present");
    assert_eq!(sc["code"].as_str(), Some("INVALID_INPUT"));
    let msg = sc["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("memstead_schema"),
        "error message must name the new tool; got: {msg}"
    );
}

/// When budget forces keys to drop, the Markdown body must carry
/// the same `hints` listing so an agent reading only the text
/// channel can re-query the dropped keys.
#[test]
fn overview_hints_render_in_markdown_body() {
    let (server, _tmp) = setup_dual_test_engine();
    // Budget 30 is below community_members + mem_distribution costs
    // on the test fixture, forcing at least one hint.
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(30),
    }));
    let parsed = ParsedOverview::from(&result);
    let hints = parsed.hint_keys();
    assert!(
        !hints.is_empty(),
        "precondition: tight budget must produce hints"
    );

    assert!(
        parsed.text.contains("## Hints"),
        "Hints header missing from markdown; got:\n{}",
        parsed.text
    );
    assert!(
        parsed.text.contains("re-query with `include:"),
        "Hints re-query hint missing; got:\n{}",
        parsed.text
    );
}

/// Every hint line carries `estimated_tokens: N` — an agent reading the
/// text channel can see how expensive a `include[]` re-query would be.
#[test]
fn overview_hints_include_estimated_tokens() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(100),
    }));
    let parsed = ParsedOverview::from(&result);
    assert!(!parsed.hint_keys().is_empty());

    let mut saw_positive = false;
    for line in parsed.text.lines() {
        let l = line.trim();
        if l.starts_with("- `")
            && let Some((_, rest)) = l.split_once("estimated_tokens: ")
            && let Ok(n) = rest.trim().parse::<u64>()
            && n > 0
        {
            saw_positive = true;
        }
    }
    assert!(
        saw_positive,
        "at least one hint must carry a positive cost; got:\n{}",
        parsed.text
    );
}

/// Hard-required content is never truncated. With an impossibly small
/// budget, the schema list (`{ref, description}`) and mem roster
/// still ship. Schema bodies (relationship vocabulary, types) live on
/// `memstead_schema(name=...)` and are no longer overview's concern.
#[test]
fn overview_hard_required_never_truncated() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(10),
    }));
    let parsed = ParsedOverview::from(&result);
    assert!(
        !parsed.schema_refs().is_empty(),
        "schemas catalogue must survive tight budget; got:\n{}",
        parsed.text
    );
    assert!(
        parsed.text.contains("memstead_schema(name="),
        "Schemas block must continue to point at memstead_schema; got:\n{}",
        parsed.text
    );
}

/// Overbudget mode surfaces when hard-required content alone exceeds the
/// caller-requested budget; hints enumerate every heavy key.
#[test]
fn overview_overbudget_mode_surfaces_when_hard_required_exceeds() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: None,
        token_budget: Some(10),
    }));
    let parsed = ParsedOverview::from(&result);
    assert_eq!(parsed.overview_mode(), "overbudget");
    assert!(
        parsed.budget_used() > parsed.budget_requested(),
        "overbudget ⇒ used > requested"
    );
    let hints: std::collections::BTreeSet<String> = parsed.hint_keys().into_iter().collect();
    // All four heavy keys appear as hints — nothing fit. Schema
    // bodies left the heavy set with the schema-tool split.
    assert!(hints.contains("mem_distribution"));
    assert!(hints.contains("community_members"));
    assert!(hints.contains("community_bridges"));
    assert!(hints.contains("dangling_links"));
    assert!(
        !hints.contains("schema_types"),
        "schema_types is no longer an overview key — schema bodies live on memstead_schema; got: {hints:?}"
    );
}

/// Unknown `include` keys surface as a typed warning with the
/// `UNKNOWN_INCLUDE_KEY` code in the `## Warnings` section.
#[test]
fn overview_unknown_include_key_warns() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["bogus".to_string()]),
        token_budget: None,
    }));
    let parsed = ParsedOverview::from(&result);
    assert!(
        parsed
            .warning_codes()
            .iter()
            .any(|c| c == "UNKNOWN_INCLUDE_KEY"),
        "UNKNOWN_INCLUDE_KEY should appear under ## Warnings; got:\n{}",
        parsed.text
    );
    assert!(
        parsed.text.contains("'bogus'"),
        "offending key should appear in the warning message; got:\n{}",
        parsed.text
    );
}

/// Inter-cluster edges show up as undirected bridge headings when
/// opted in via `include`. Pair key is lexicographically normalised.
#[test]
fn community_bridges_aggregates_undirected() {
    let (server, _tmp) = setup_bridge_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["community_bridges".to_string()]),
        token_budget: None,
    }));
    let parsed = ParsedOverview::from(&result);
    let headings = parsed.bridge_headings();
    assert!(
        !headings.is_empty(),
        "bridge engine must produce at least one bridge"
    );
    // Heading shape: `<from> ↔ <to> (N edges)`. Pair lex-normalised ⇒
    // the from cluster id ≤ to cluster id textually.
    let h = &headings[0];
    let parts: Vec<&str> = h.splitn(2, " ↔ ").collect();
    assert_eq!(parts.len(), 2, "unexpected bridge heading shape: {h}");
    let from = parts[0];
    let to = parts[1].split_once(' ').map(|(a, _)| a).unwrap_or(parts[1]);
    assert!(
        from <= to,
        "bridge pair must be lex-normalised: {from} > {to}"
    );
    // The block must carry at least one `- **Edge types:**` row and one
    // sample edge bullet.
    assert!(parsed.text.contains("- **Edge types:**"));
}

/// With a mem filter, `community_bridges` lists only edges whose
/// source entity lives in that mem — asymmetric, matching
/// `memstead_health`. Build a two-mem engine with beta having no outgoing
/// edges, then filter to beta: the bridge pool must be empty even
/// though the cross-cluster edge exists in the global graph.
#[test]
fn overview_mem_filter_scopes_bridges_source_side() {
    let tmp = TempDir::new().unwrap();

    let alpha_dir = tmp.path().join("alpha");
    fs::create_dir_all(alpha_dir.join(".memstead")).unwrap();
    fs::write(
        alpha_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            alpha_dir.join("alpha-root.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Alpha Root\n\n## Identity\n\nAlpha.\n\n## Purpose\n\nFixture.\n",
        )
        .unwrap();

    let beta_dir = tmp.path().join("beta");
    fs::create_dir_all(beta_dir.join(".memstead")).unwrap();
    fs::write(
        beta_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            beta_dir.join("beta-root.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Beta Root\n\n## Identity\n\nBeta.\n\n## Purpose\n\nFixture.\n",
        )
        .unwrap();

    let _ = (alpha_dir, beta_dir);
    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    // Filter to beta: no outgoing edges from beta entities → no bridges.
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: Some("beta".to_string()),
        include: Some(vec!["community_bridges".to_string()]),
        token_budget: None,
    }));
    let parsed = ParsedOverview::from(&result);
    assert!(
        parsed.bridge_headings().is_empty(),
        "bridges filtered to beta must be empty — source-in-mem only; got:\n{}",
        parsed.text
    );
}

/// `memstead_overview include=["dangling_links"]` renders the
/// dangling-reference section. This fixture exercises one of the three
/// conditions, `DANGLING_LINK_TARGET_MISSING`: a body wiki-link whose
/// target has no markdown file. Pre-fix this slot returned a hardcoded `[]` and
/// the `## Dangling Links` block never rendered; the test fails
/// against that state.
#[test]
fn overview_dangling_links_surfaces_stub_targets_single_mem() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    // `a.md` references `[[gone]]` — no on-disk file, auto-stubs at
    // load. The stub is the dangling signal mirrored from the
    // health surface's `health_dangling_links_surfaces_stub_targets`.
    fs::write(
            mem_dir.join("a.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# A\n\n## Identity\n\nFixture.\n\n## Purpose\n\nRefers to [[gone]] in prose.\n",
        )
        .unwrap();

    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["dangling_links".to_string()]),
        token_budget: None,
    }));
    let parsed = ParsedOverview::from(&result);
    assert!(
        parsed.text.contains("## Dangling Links"),
        "overview must render the Dangling Links section when an opt-in caller finds one:\n{}",
        parsed.text
    );
    assert!(
        parsed.text.contains("specs--a"),
        "Dangling Links must name the linking entity:\n{}",
        parsed.text
    );
    assert!(
        parsed.text.contains("specs--gone"),
        "Dangling Links must name the dangling target:\n{}",
        parsed.text
    );
    // The condition, not only the two ids: this doc comment claims the
    // fixture raises target-missing, so the rendering must say so.
    assert!(
        parsed.text.contains("[DANGLING_LINK_TARGET_MISSING]"),
        "Dangling Links must name which of the three conditions:\n{}",
        parsed.text
    );
}

/// Cross-mem dangling resolution: a link from mem A to a real
/// target in mem B is NOT dangling (the unified engine resolves
/// targets across all mounts); a link from mem A to a non-
/// existent target in mem B IS dangling. This pins full's
/// multi-mount semantics — naïvely scanning each mount in
/// isolation would mis-flag the cross-mem hit.
#[test]
fn overview_dangling_links_cross_mem_respects_unified_store() {
    let tmp = TempDir::new().unwrap();
    let alpha_dir = tmp.path().join("alpha");
    fs::create_dir_all(alpha_dir.join(".memstead")).unwrap();
    fs::write(
        alpha_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    // alpha--root carries two body wiki-links: `[[orphan]]` aliases
    // a relation to an absent target (dangling — target missing
    // case), `[[beta:anchor]]` is backed by an explicit
    // cross-mem REFERENCES entry (alias is valid, target lives).
    // Acceptance under the alias model: only the missing-target
    // case appears in dangling_links.
    fs::write(
            alpha_dir.join("root.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Root\n\n## Identity\n\nFixture root.\n\n## Purpose\n\nPoints at [[orphan]] (gone) and [[beta:anchor]] (lives).\n\n## Relationships\n\n- **REFERENCES**: [[orphan]]\n- **REFERENCES**: [[beta:anchor]]\n",
        )
        .unwrap();

    let beta_dir = tmp.path().join("beta");
    fs::create_dir_all(beta_dir.join(".memstead")).unwrap();
    fs::write(
        beta_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            beta_dir.join("anchor.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Anchor\n\n## Identity\n\nReachable cross-mem target.\n\n## Purpose\n\nMust not appear in dangling links.\n",
        )
        .unwrap();

    let _ = (alpha_dir, beta_dir);
    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["dangling_links".to_string()]),
        token_budget: None,
    }));
    let parsed = ParsedOverview::from(&result);
    // Slice out the Dangling Links block — `beta--anchor` may
    // surface in other sections (Communities lists every member),
    // so the cross-mem non-flagging assertion has to look only
    // at the block under test.
    let dangling_block: String = {
        let mut block = String::new();
        let mut inside = false;
        for line in parsed.text.lines() {
            if line.starts_with("## Dangling Links") {
                inside = true;
                continue;
            }
            if inside {
                if line.starts_with("## ") {
                    break;
                }
                block.push_str(line);
                block.push('\n');
            }
        }
        block
    };
    assert!(
        parsed.text.contains("## Dangling Links"),
        "overview must render Dangling Links when the cross-mem sweep finds one:\n{}",
        parsed.text
    );
    assert!(
        dangling_block.contains("alpha--orphan"),
        "dangling target inside the source mem must be listed: dangling-block=\n{dangling_block}\nfull=\n{}",
        parsed.text
    );
    assert!(
        !dangling_block.contains("beta--anchor"),
        "cross-mem link to a real target must NOT be flagged dangling: dangling-block=\n{dangling_block}"
    );
}

/// `sample_edges` is capped at 3 entries, sorted by (rel_type, from, to).
/// Build a fixture with two well-separated Louvain clusters (two dense
/// triangles) bridged by five same-type edges; cap applies to the bridge.
#[test]
fn overview_community_bridges_caps_sample_edges_at_three() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("caps");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();

    let fm =
        "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n";
    // Five sources a0..a4, all USES the same target hub. Each source
    // also USES two other sources to create a dense cluster. The hub
    // sits in its own cluster thanks to four target-side leaves.
    for i in 0..5 {
        let mut rels = "- **USES**: [[hub]]\n".to_string();
        for j in 0..5 {
            if j != i {
                rels.push_str(&format!("- **USES**: [[a{j}]]\n"));
            }
        }
        let body = format!(
            "{fm}# a{i}\n\n## Identity\n\nSource {i}.\n\n## Purpose\n\nBridge fixture.\n\n## Relationships\n\n{rels}"
        );
        fs::write(mem_dir.join(format!("a{i}.md")), body).unwrap();
    }
    for i in 0..4 {
        let mut rels = "- **USES**: [[hub]]\n".to_string();
        for j in 0..4 {
            if j != i {
                rels.push_str(&format!("- **USES**: [[b{j}]]\n"));
            }
        }
        let body = format!(
            "{fm}# b{i}\n\n## Identity\n\nHub leaf {i}.\n\n## Purpose\n\nBridge fixture.\n\n## Relationships\n\n{rels}"
        );
        fs::write(mem_dir.join(format!("b{i}.md")), body).unwrap();
    }
    fs::write(
        mem_dir.join("hub.md"),
        format!("{fm}# hub\n\n## Identity\n\nHub.\n\n## Purpose\n\nBridge fixture.\n"),
    )
    .unwrap();

    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["community_bridges".to_string()]),
        token_budget: None,
    }));
    let parsed = ParsedOverview::from(&result);

    // Bridges block: each heading is `from ↔ to (N edges)`. Samples
    // render as `  - \`rel\` from → to` bullets under each heading,
    // sorted by (rel_type, from, to). The cap (≤3 samples per pair) is
    // enforced by counting sample-bullet rows per heading.
    let mut headings = Vec::new();
    let mut samples: std::collections::HashMap<String, Vec<String>> = Default::default();
    let mut current: Option<String> = None;
    let mut in_bridges = false;
    for line in parsed.text.lines() {
        if line.starts_with("## Community Bridges") {
            in_bridges = true;
            continue;
        }
        if in_bridges && line.starts_with("## ") && !line.starts_with("### ") {
            break;
        }
        if in_bridges {
            if let Some(h) = line.strip_prefix("### ") {
                headings.push(h.to_string());
                current = Some(h.to_string());
                samples.insert(h.to_string(), Vec::new());
            } else if line.starts_with("  - `")
                && let Some(cur) = &current
            {
                samples.get_mut(cur).unwrap().push(line.to_string());
            }
        }
    }

    assert!(
        !headings.is_empty(),
        "fixture must produce at least one bridge heading"
    );
    let mut saw_capped = false;
    for (h, ss) in &samples {
        assert!(
            ss.len() <= 3,
            "sample_edges capped at 3 per pair; pair={h} got {}",
            ss.len()
        );
        // Sort order check: extract (rel_type, from, to) from each line.
        let mut parsed_keys: Vec<(String, String, String)> = Vec::new();
        for s in ss {
            // Line shape: `  - `<rel_type>` <from> → <to>`
            if let Some(rest) = s.strip_prefix("  - `")
                && let Some((rel, rest)) = rest.split_once('`')
            {
                let rest = rest.trim_start();
                if let Some((from, to)) = rest.split_once(" → ") {
                    parsed_keys.push((rel.to_string(), from.to_string(), to.to_string()));
                }
            }
        }
        for w in parsed_keys.windows(2) {
            assert!(
                w[0] <= w[1],
                "samples must be sorted by (rel_type, from, to)"
            );
        }

        // Find edge_count on the heading — trailing `(N edges)`.
        let count: u64 = h
            .rsplit_once('(')
            .and_then(|(_, tail)| tail.strip_suffix(" edges)"))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        if count >= 4 {
            assert_eq!(ss.len(), 3, "4+ edges on one pair → capped at 3");
            saw_capped = true;
        }
    }
    assert!(
        saw_capped,
        "fixture should produce at least one bridge with 4+ edges"
    );
}

/// `rebuild: true` discards the Louvain memo, so an edge added between
/// two calls is reflected in the next bridge count.
#[test]
fn overview_rebuild_refreshes_bridges() {
    let (server, _tmp) = setup_bridge_engine();

    let first = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["community_bridges".to_string()]),
        token_budget: None,
    }));
    let before_total = sum_bridge_edge_counts(&ParsedOverview::from(&first));

    // Add another cross-cluster edge via memstead_relate.
    let add = server.memstead_relate(Parameters(crate::tools::mutation::RelateParams {
        relations: vec![RelateOpInput {
            from: "bridge--beta-hub".to_string(),
            to: "bridge--alpha-hub".to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        !add.is_error.unwrap_or(false),
        "relate must succeed: {}",
        extract_text(&add)
    );

    let second = server.memstead_overview(Parameters(OverviewParams {
        rebuild: Some(true),
        chunk: None,
        mem: None,
        include: Some(vec!["community_bridges".to_string()]),
        token_budget: None,
    }));
    let after_total = sum_bridge_edge_counts(&ParsedOverview::from(&second));

    assert!(
        after_total > before_total,
        "rebuild must surface the new edge (before={before_total}, after={after_total})"
    );
}

/// Sum every `### <from> ↔ <to> (N edges)` heading's N under the
/// `## Community Bridges` block.
fn sum_bridge_edge_counts(parsed: &ParsedOverview) -> u64 {
    parsed
        .bridge_headings()
        .iter()
        .filter_map(|h| {
            h.rsplit_once('(')
                .and_then(|(_, tail)| tail.strip_suffix(" edges)"))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .sum()
}

/// `hints[]` is deterministic across identical calls — locks the
/// iteration order against future refactors.
#[test]
fn overview_hints_ordered_deterministically_across_calls() {
    let (server, _tmp) = setup_dual_test_engine();
    let mut runs: Vec<Vec<(String, u64)>> = Vec::new();
    for _ in 0..3 {
        let r = server.memstead_overview(Parameters(OverviewParams {
            rebuild: Some(true),
            chunk: None,
            mem: None,
            include: None,
            token_budget: Some(100),
        }));
        let parsed = ParsedOverview::from(&r);
        let mut hints: Vec<(String, u64)> = Vec::new();
        for line in parsed.text.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("- `")
                && let Some((key, tail)) = rest.split_once("` — estimated_tokens: ")
                && let Ok(n) = tail.trim().parse::<u64>()
            {
                hints.push((key.to_string(), n));
            }
        }
        runs.push(hints);
    }
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[1], runs[2]);
    assert!(!runs[0].is_empty(), "tight budget must produce hints");
}

/// Char-counting keeps multibyte content from inflating token
/// estimates — `Ä` counts as one char whether serialised as `"Ä"`
/// (2 UTF-8 bytes) or as `"A"` (1 byte).
#[test]
fn multibyte_content_does_not_inflate_estimate() {
    use memstead_base::chunking::estimate_tokens;
    let ascii = "Grosse Aenderung - uebersicht";
    let multi = "Große Änderung — übersicht ✨";
    let a = estimate_tokens(ascii);
    let b = estimate_tokens(multi);
    let diff = a.abs_diff(b);
    let max = a.max(b).max(1);
    assert!(
        diff * 10 <= max,
        "char-counting should keep multibyte estimates within ±10%: ascii={a}, multi={b}"
    );
}

/// Unknown mem name is a tool error, not a silent empty response —
/// and the error enumerates valid mems so the agent can retry
/// without a second round-trip.
#[test]
fn overview_unknown_mem_errors() {
    let (server, _tmp) = setup_two_mem_engine();
    let result = server.memstead_overview(Parameters(OverviewParams {
        rebuild: None,
        chunk: None,
        mem: Some("ghost".to_string()),
        include: None,
        token_budget: None,
    }));
    assert_eq!(result.is_error, Some(true));
    let text = extract_text(&result);
    assert!(
        text.contains("ghost"),
        "error should name the bad mem: {text}"
    );
    assert!(
        text.contains("alpha") && text.contains("beta"),
        "error should list writable mems: {text}"
    );
}

/// Health filter narrows every entity-scoped count, distribution, and
/// detail list to the filter mem while keeping the workspace roster
/// (`writable_mems`/`read_mems`) and community count global.
#[test]
fn health_filtered_to_one_mem() {
    let (server, _tmp) = setup_two_mem_engine();

    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["orphans".to_string(), "most_connected".to_string()]),
        limit: None,
        mem: Some("alpha".to_string()),
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(json["mem"], "alpha", "mem echo marks filter mode");
    assert_eq!(json["summary"]["total_entities"].as_u64().unwrap(), 1);
    assert_eq!(json["real_nodes"].as_u64().unwrap(), 1);

    // Type distribution narrowed to alpha's single entity.
    let type_dist = json["type_distribution"].as_array().unwrap();
    let total: u64 = type_dist.iter().map(|t| t["count"].as_u64().unwrap()).sum();
    assert_eq!(total, 1, "type_distribution narrows to alpha");

    // Writable mems still lists both — roster must not be filtered.
    let writable: Vec<String> = json["writable_mems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        writable,
        vec!["alpha".to_string(), "beta".to_string()],
        "writable_mems stays global under a filter"
    );

    // mem_schemas narrows to alpha's single pin.
    let mem_schemas = json["mem_schemas"].as_array().unwrap();
    assert_eq!(mem_schemas.len(), 1);
    assert_eq!(mem_schemas[0]["mem"], "alpha");

    // Most-connected list must not leak beta entities.
    if let Some(arr) = json.get("most_connected").and_then(|v| v.as_array()) {
        for hit in arr {
            let id = hit["id"].as_str().unwrap();
            assert!(id.starts_with("alpha--"), "most_connected leaks beta: {id}");
        }
    }
}

/// Default (unfiltered) health keeps the global aggregate shape —
/// no per-mem filtering unless a `mem` argument is supplied.
#[test]
fn health_unfiltered_aggregates_all() {
    let (server, _tmp) = setup_two_mem_engine();

    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert!(json["mem"].is_null(), "unfiltered mode echoes null");
    assert_eq!(json["summary"]["total_entities"].as_u64().unwrap(), 2);
    assert_eq!(json["real_nodes"].as_u64().unwrap(), 2);

    // mem_schemas sources from MemState.name — both mems visible.
    let names: Vec<String> = json["mem_schemas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["mem"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"alpha".to_string()));
    assert!(names.contains(&"beta".to_string()));
}

/// Unknown mem on `health` errors with the same contract as `overview`.
#[test]
fn health_unknown_mem_errors() {
    let (server, _tmp) = setup_two_mem_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: Some("ghost".to_string()),
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    assert_eq!(result.is_error, Some(true));
    let text = extract_text(&result);
    assert!(text.contains("ghost"));
    assert!(text.contains("alpha") && text.contains("beta"));
}

#[test]
fn memstead_create_rejects_unknown_type() {
    // `EngineError::UnknownType` carries `name`, `schema_ref`,
    // `declared`, `suggestion`. The error message enumerates
    // valid types.
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        title: "Bad Type Entity".to_string(),
        entity_type: "bogus".to_string(),
        mem: Some("specs".to_string()),
        sections: None,
        metadata: None,
        relations: None,
        dry_run: None,

        note: None,

        role: None,
        identity: None,
    }));
    assert_eq!(result.is_error, Some(true));
    let text = extract_text(&result);
    assert!(
        text.contains("bogus"),
        "error should name the bad type: {text}"
    );
    assert!(
        text.contains("spec") && text.contains("memo"),
        "error should list valid types: {text}"
    );
}

/// `dry_run` preview on `memstead_create` returns the prospective id +
/// 16-hex `_hash` with no disk write / no commit. A follow-up real
/// call with the same inputs produces the same hash — the content
/// hash covers the `now()`-stamped `created_date`, so the test
/// pins the engine mutation clock to make the equality
/// deterministic (unpinned it would only hold within one
/// wall-clock second — the create-path caveat the dry_run
/// docstring names).
#[test]
fn memstead_create_dry_run_preview() {
    let (server, _tmp) = setup_dual_test_engine();
    // Pin the mutation clock: `created_date` enters `_hash`, so the
    // dry-run == real hash-equality assertion below is only
    // deterministic when both calls stamp the same instant —
    // unpinned, the test fails whenever a wall-clock second ticks
    // between them.
    server
        .unified_engine()
        .lock()
        .unwrap()
        .set_mutation_clock(std::sync::Arc::new(|| {
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_754_000_000)
        }));
    let result = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        title: "Große Änderung".to_string(),
        entity_type: "spec".to_string(),
        mem: Some("specs".to_string()),
        sections: Some(IndexMap::from_iter([
            ("identity".to_string(), "Preview.".to_string()),
            ("purpose".to_string(), "Preview.".to_string()),
        ])),
        metadata: None,
        relations: None,
        dry_run: Some(true),

        note: None,

        role: None,
        identity: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "dry_run must succeed: {}",
        extract_text(&result)
    );
    let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
    // F1 (B+A): the slug now preserves precomposed Latin
    // diacritics (`Große Änderung` → `große-änderung`) instead
    // of transliterating to `grosse-aenderung`.
    assert_eq!(json["id"].as_str().unwrap(), "specs--große-änderung");
    assert_eq!(json["write_id"].as_str().unwrap(), "");
    // Engine's compute_hash truncates SHA-256 to 16 hex chars — MCP
    // serialises that directly.
    assert_eq!(json["_hash"].as_str().unwrap().len(), 16);

    // Follow-up real call with the same inputs → same hash (both
    // calls share the wall-clock second, so the `created_date`
    // stamp matches; see the test doc for the cross-second caveat).
    let real = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        title: "Große Änderung".to_string(),
        entity_type: "spec".to_string(),
        mem: Some("specs".to_string()),
        sections: Some(IndexMap::from_iter([
            ("identity".to_string(), "Preview.".to_string()),
            ("purpose".to_string(), "Preview.".to_string()),
        ])),
        metadata: None,
        relations: None,
        dry_run: Some(false),

        note: None,

        role: None,
        identity: None,
    }));
    let real_json: serde_json::Value = serde_json::from_str(&extract_text(&real)).unwrap();
    assert_eq!(real_json["_hash"], json["_hash"]);
}

/// #06: JSON object key order flowing into CreateParams.sections must be
/// preserved — IndexMap's serde Deserialize is insertion-order aware, so
/// an agent that emits a specific sections order sees it honoured at the
/// MCP boundary (before engine-side re-parse normalises to schema order).
#[test]
fn create_params_preserves_json_key_order() {
    let payload = serde_json::json!({
        "title": "Ordered",
        "entity_type": "spec",
        "sections": { "identity": "A", "purpose": "B", "specifies": "C" }
    });
    let params: CreateParams = serde_json::from_value(payload).unwrap();
    let sections = params.sections.unwrap();
    let keys: Vec<&str> = sections.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["identity", "purpose", "specifies"],
        "IndexMap deserialisation must preserve JSON key order"
    );
}

#[test]
fn memstead_create_requires_entity_type_argument() {
    // Required-at-deserialization: a JSON payload missing `entity_type`
    // (and no `schema` alias) must fail to deserialize into CreateParams.
    let payload = serde_json::json!({
        "title": "No Type",
        "mem": "specs"
    });
    let err = serde_json::from_value::<CreateParams>(payload)
        .expect_err("deserialization should fail without entity_type");
    let msg = err.to_string();
    assert!(
        msg.contains("entity_type") || msg.contains("schema"),
        "error should mention entity_type: {msg}"
    );
}

/// `memstead_health` response must expose load-time nested-prefix
/// warnings on `warnings` so agents see drift without reaching into
/// engine internals. Bootstraps a fixture whose plugin mem has an
/// inline `[[plugin--foo]]` link and asserts the wire carries a
/// `SUSPICIOUS_NESTED_PREFIX` envelope.
#[test]
fn health_surfaces_nested_prefix_load_warnings() {
    let tmp = TempDir::new().unwrap();
    let plugin_dir = tmp.path().join("test-mem-plugin");
    fs::create_dir_all(plugin_dir.join(".memstead")).unwrap();
    fs::write(
        plugin_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            plugin_dir.join("foo.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Foo\n\n## Identity\n\nTarget.\n\n## Purpose\n\nExists.\n",
        )
        .unwrap();
    fs::write(
            plugin_dir.join("drifted.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Drifted\n\n## Identity\n\nDrifted.\n\n## Purpose\n\nReferences [[plugin--foo]] via nested prefix.\n",
        )
        .unwrap();

    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
    let warnings = json["warnings"]
        .as_array()
        .expect("warnings must be an array with the drift finding");
    let drift = warnings
        .iter()
        .find(|w| w["code"] == "SUSPICIOUS_NESTED_PREFIX")
        .expect("nested-prefix warning must appear on the wire");
    assert_eq!(
        drift["details"]["from"].as_str(),
        Some("test-mem-plugin--drifted"),
        "details.from must identify the authoring entity"
    );
    assert_eq!(
        drift["details"]["resolved_id"].as_str(),
        Some("plugin--foo"),
        "details.resolved_id must carry the tier-0 cross-mem id the body link resolves to"
    );
    assert_eq!(
        drift["details"]["candidate_target"].as_str(),
        Some("test-mem-plugin--foo"),
        "pass-2 fallback must find the same-mem bare-slug target"
    );
    assert_eq!(drift["details"]["section"].as_str(), Some("purpose"));
}

/// `memstead_health(mem=X)` filters mem-attributable warnings to
/// mem X. Pre-fix the SUSPICIOUS_NESTED_PREFIX warning emitted
/// for one mem's drift leaked into queries scoped to a sibling
/// mem — agents couldn't act on it because the offending entity
/// wasn't in their scope. The fix keeps the warning on global
/// queries and on queries scoped to the warning's source mem,
/// and drops it from queries scoped elsewhere.
#[test]
fn health_warnings_respect_mem_filter() {
    let tmp = TempDir::new().unwrap();
    // Mem A — carries the nested-prefix drift.
    let plugin_dir = tmp.path().join("test-mem-plugin");
    fs::create_dir_all(plugin_dir.join(".memstead")).unwrap();
    fs::write(
        plugin_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            plugin_dir.join("foo.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Foo\n\n## Identity\n\nTarget.\n\n## Purpose\n\nExists.\n",
        )
        .unwrap();
    fs::write(
            plugin_dir.join("drifted.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Drifted\n\n## Identity\n\nDrifted.\n\n## Purpose\n\nReferences [[plugin--foo]] via nested prefix.\n",
        )
        .unwrap();

    // Mem B — clean.
    let clean_dir = tmp.path().join("specs");
    fs::create_dir_all(clean_dir.join(".memstead")).unwrap();
    fs::write(
        clean_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    fs::write(
            clean_dir.join("clean.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Clean\n\n## Identity\n\nClean.\n\n## Purpose\n\nNo drift.\n",
        )
        .unwrap();

    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    // Global (no mem filter): drift surfaces.
    let global = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let global_json: serde_json::Value = serde_json::from_str(&extract_text(&global)).unwrap();
    let global_warnings = global_json["warnings"]
        .as_array()
        .expect("global query carries the drift warning");
    assert!(
        global_warnings
            .iter()
            .any(|w| w["code"] == "SUSPICIOUS_NESTED_PREFIX"),
        "global query must surface the drift"
    );

    // Scoped to the offending mem: drift surfaces.
    let scoped_to_offender = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: Some("test-mem-plugin".to_string()),
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let scoped_offender_json: serde_json::Value =
        serde_json::from_str(&extract_text(&scoped_to_offender)).unwrap();
    let offender_warnings = scoped_offender_json["warnings"]
        .as_array()
        .expect("scoped-to-offender carries the drift");
    assert!(
        offender_warnings
            .iter()
            .any(|w| w["code"] == "SUSPICIOUS_NESTED_PREFIX"),
        "scoped-to-offender query must surface the drift"
    );

    // Scoped to a clean sibling: drift filtered out — pre-fix it
    // leaked into this query and agents couldn't act on it.
    let scoped_clean = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: Some("specs".to_string()),
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let scoped_clean_json: serde_json::Value =
        serde_json::from_str(&extract_text(&scoped_clean)).unwrap();
    let clean_warnings_opt = scoped_clean_json["warnings"].as_array();
    if let Some(clean_warnings) = clean_warnings_opt {
        assert!(
            !clean_warnings
                .iter()
                .any(|w| w["code"] == "SUSPICIOUS_NESTED_PREFIX"),
            "scoped-to-clean must NOT surface another mem's drift; got {clean_warnings:?}"
        );
    }
}

#[test]
fn test_memstead_health_default_is_compact() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    assert!(
        text.len() < 2000,
        "Default health should be compact, got {} chars",
        text.len()
    );
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json["summary"]["total_entities"].is_number());
    assert!(json["summary"]["total_orphans"].is_number());
    assert!(json["summary"]["total_stubs"].is_number());
    // Detail sections should NOT be present by default
    assert!(json.get("orphans").is_none());
    assert!(json.get("stubs").is_none());
    assert!(json.get("most_connected").is_none());
}

#[test]
fn test_memstead_health_with_details() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["orphans".into()]),
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        json["orphans"].is_array(),
        "orphans detail should be present when requested"
    );
    assert!(json["summary"]["total_orphans"].is_number());
}

#[test]
fn test_create_update_delete_cycle() {
    let (server, _tmp) = setup_dual_test_engine();

    // Create
    let result = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        title: "New Entity".to_string(),
        entity_type: "spec".to_string(),
        mem: Some("specs".to_string()),
        sections: Some(IndexMap::from_iter([
            ("identity".to_string(), "A new entity".to_string()),
            ("purpose".to_string(), "Testing CRUD".to_string()),
        ])),
        metadata: None,
        relations: None,
        dry_run: None,

        note: None,

        role: None,
        identity: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "Create failed: {}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    let create_json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let id = create_json["id"].as_str().unwrap().to_string();
    assert_eq!(
        id, "specs--new-entity",
        "ID should be derived from mem + slug(title)"
    );
    let hash = create_json["_hash"].as_str().unwrap().to_string();

    // Read
    let result = server.memstead_entity(Parameters(EntityParams {
        id: id.clone(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let text = extract_text(&result);
    assert!(text.contains("# New Entity"));

    // Update
    let result = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: id.clone(),
        expected_hash: Some(hash),
        sections: Some(IndexMap::from_iter([(
            "identity".to_string(),
            "An updated entity".to_string(),
        )])),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: None,

        note: None,

        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "Update failed: {}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    let update_json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let post_update_hash = update_json["_hash"]
        .as_str()
        .expect("update must return content_hash")
        .to_string();

    // Delete — requires the current (post-update) hash;
    // `expected_hash` is mandatory on `memstead_delete`.
    let result = server.memstead_delete(Parameters(DeleteParams {
        id: id.clone(),
        expected_hash: post_update_hash,

        note: None,

        role: None,
        identity: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "Delete failed: {}",
        extract_text(&result)
    );

    // Verify deletion
    let result = server.memstead_entity(Parameters(EntityParams {
        id,
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(result.is_error.unwrap_or(false));
}

#[test]
fn test_memstead_relate() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "DEPENDS_ON".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "Relate failed: {}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["results"][0]["rel_type"], "DEPENDS_ON");
}

#[test]
fn relate_cycle_surfaces_relationship_cycle_envelope() {
    // `EngineError::RelationshipCycle` surfaces a typed
    // RELATIONSHIP_CYCLE envelope; cycle detection runs via
    // `would_cycle` from `memstead_base::graph::query`.
    let (server, _tmp) = setup_dual_test_engine();
    // a DEPENDS_ON b is fine.
    let first = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "DEPENDS_ON".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(!first.is_error.unwrap_or(false));
    // b DEPENDS_ON a closes a cycle and must reject with the
    // structured envelope.
    let cycle = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-b".to_string(),
            to: "specs--entity-a".to_string(),
            rel_type: "DEPENDS_ON".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        cycle.is_error.unwrap_or(false),
        "cycle-closing relate must error"
    );
    let sc = cycle
        .structured_content
        .as_ref()
        .expect("RELATIONSHIP_CYCLE must carry structured_content");
    assert_eq!(sc["code"], "RELATIONSHIP_CYCLE");
    assert_eq!(sc["details"]["rel_type"], "DEPENDS_ON");
    assert_eq!(sc["details"]["from"], "specs--entity-b");
    assert_eq!(sc["details"]["to"], "specs--entity-a");
    assert_eq!(sc["details"]["path_truncated"], false);
    let path = sc["details"]["existing_path"]
        .as_array()
        .expect("existing_path array");
    assert_eq!(path.len(), 2);
    assert_eq!(path[0], "specs--entity-a");
    assert_eq!(path[1], "specs--entity-b");
}

/// `memstead_relate` on a self-loop over a rel-type the source
/// type lists in `no_self_loop_relationships` refuses with
/// `RELATIONSHIP_CYCLE`, independent of the rel-type's `acyclic`
/// flag. The default schema's spec type declares
/// `no_self_loop_relationships: [DEPENDS_ON, USES]`, so a
/// `USES from spec--X to spec--X` self-loop fires the gate even
/// though USES carries `acyclic: false`.
#[test]
fn relate_self_loop_refuses_on_listed_no_self_loop_rel_type() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-a".to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        result.is_error.unwrap_or(false),
        "self-loop on a listed no-self-loop rel-type must refuse: {}",
        extract_text(&result)
    );
    let sc = result
        .structured_content
        .as_ref()
        .expect("RELATIONSHIP_CYCLE must carry structured_content");
    assert_eq!(sc["code"], "RELATIONSHIP_CYCLE");
    assert_eq!(sc["details"]["rel_type"], "USES");
    assert_eq!(sc["details"]["from"], "specs--entity-a");
    assert_eq!(sc["details"]["to"], "specs--entity-a");
}

/// `memstead_relate` rejects cross-mem edges with the typed
/// `CROSS_MEM_LINK_NOT_ALLOWED` envelope on `structured_content`
/// when the workspace's `[cross_mem_links]` policy denies the
/// pairing. The default-empty policy denies every cross-mem
/// pair, so this fixture (two mems, no policy) trips the
/// envelope.
#[test]
fn relate_cross_mem_surfaces_typed_envelope() {
    let (server, _tmp) = setup_two_mem_engine();
    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "alpha--alpha-root".to_string(),
            to: "beta--beta-root".to_string(),
            rel_type: "REFERENCES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        result.is_error.unwrap_or(false),
        "cross-mem relate must error when policy denies"
    );
    let sc = result
        .structured_content
        .as_ref()
        .expect("CROSS_MEM_LINK_NOT_ALLOWED must carry structured_content");
    assert_eq!(sc["code"], "CROSS_MEM_LINK_NOT_ALLOWED");
    assert_eq!(sc["details"]["from_mem"], "alpha");
    assert_eq!(sc["details"]["to_mem"], "beta");
    assert!(
        sc["message"].as_str().is_some_and(|m| !m.is_empty()),
        "message field must be populated: {sc:?}"
    );
}

/// `memstead_update` with overlapping `metadata` + `metadata_unset`
/// keys returns the typed `SET_AND_UNSET_CONFLICT` envelope.
/// Locks Item 02.
#[test]
fn update_set_and_unset_overlap_surfaces_typed_envelope() {
    let (server, _tmp) = setup_dual_test_engine();
    // Read the current hash for entity-a.
    let read = server.memstead_entity(Parameters(EntityParams {
        id: "specs--entity-a".to_string(),
        sections: None,
        include_relations: None,
        include_context: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let hash = extract_text(&read)
        .lines()
        .find(|l| l.starts_with("_hash:"))
        .map(|l| l.trim_start_matches("_hash:").trim().to_string())
        .expect("_hash present");

    let mut metadata = IndexMap::new();
    metadata.insert("tags".to_string(), "x".to_string());
    let result = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: "specs--entity-a".to_string(),
        expected_hash: Some(hash),
        sections: None,
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: Some(metadata),
        metadata_unset: Some(vec!["tags".to_string()]),
        dry_run: Some(true),
        note: None,
        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(
        result.is_error.unwrap_or(false),
        "set+unset overlap must error"
    );
    let sc = result
        .structured_content
        .as_ref()
        .expect("SET_AND_UNSET_CONFLICT must carry structured_content");
    assert_eq!(sc["code"], "SET_AND_UNSET_CONFLICT");
    let keys = sc["details"]["keys"]
        .as_array()
        .expect("keys array present");
    assert!(keys.iter().any(|k| k == "tags"));
}

/// `memstead_relate from=<stub-id>` returns the typed
/// `STUB_CANNOT_RELATE` envelope, not the pre-fix cryptic
/// `UnknownType { name: "" }`. Locks Item 04 of the graph-correctness contract.
#[test]
fn relate_from_stub_surfaces_typed_envelope() {
    let (server, _tmp) = setup_dual_test_engine();
    // Step 1: relate entity-a → ghost-target, auto-creates a
    // stub at specs--ghost-target with no entity_type. USES (not
    // REFERENCES) — explicit relate to the schema's pointer rel-
    // type is refused under `manual_authoring: forbidden`.
    let _ = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--ghost-target".to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    // Step 2: now relate FROM the stub — must surface
    // STUB_CANNOT_RELATE rather than the cryptic UnknownType
    // envelope.
    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--ghost-target".to_string(),
            to: "specs--entity-a".to_string(),
            rel_type: "USES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        result.is_error.unwrap_or(false),
        "relate from stub must error"
    );
    let sc = result
        .structured_content
        .as_ref()
        .expect("STUB_CANNOT_RELATE must carry structured_content");
    assert_eq!(sc["code"], "STUB_CANNOT_RELATE");
    assert_eq!(sc["details"]["id"], "specs--ghost-target");
    assert!(
        sc["message"]
            .as_str()
            .is_some_and(|m| m.contains("stub") && m.contains("memstead_create")),
        "message must name the constraint and the recovery path: {sc:?}"
    );
}

/// `memstead_update patch_sections` with an `old` substring that
/// isn't in the body returns the typed `PATCH_OLD_NOT_FOUND`
/// envelope with the truncated current-content snapshot.
/// Locks Item 02.
#[test]
fn update_patch_old_not_found_surfaces_typed_envelope() {
    let (server, _tmp) = setup_dual_test_engine();
    let read = server.memstead_entity(Parameters(EntityParams {
        id: "specs--entity-a".to_string(),
        sections: None,
        include_relations: None,
        include_context: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let hash = extract_text(&read)
        .lines()
        .find(|l| l.starts_with("_hash:"))
        .map(|l| l.trim_start_matches("_hash:").trim().to_string())
        .expect("_hash present");

    let mut patches: IndexMap<String, crate::tools::mutation::PatchesInput> = IndexMap::new();
    patches.insert(
        "identity".to_string(),
        crate::tools::mutation::PatchesInput::One(PatchInput {
            old: "this-substring-is-not-in-the-body".to_string(),
            new: "replacement".to_string(),
            all: None,
        }),
    );
    let result = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: "specs--entity-a".to_string(),
        expected_hash: Some(hash),
        sections: None,
        append_sections: None,
        patch_sections: Some(patches),
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: Some(true),
        note: None,
        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(result.is_error.unwrap_or(false));
    let sc = result
        .structured_content
        .as_ref()
        .expect("PATCH_OLD_NOT_FOUND must carry structured_content");
    assert_eq!(sc["code"], "PATCH_OLD_NOT_FOUND");
    assert_eq!(sc["details"]["section"], "identity");
    assert!(
        sc["details"]["current_content"].is_string(),
        "current_content snapshot must ship"
    );
    assert!(
        sc["details"]["truncated"].is_boolean(),
        "truncated flag must ship"
    );
}

/// `memstead_relate` with a target id that does not match the
/// wiki-link grammar must not seed a polluted stub — it returns
/// `INVALID_ENTITY_ID` with `details.id` and `details.reason`,
/// and no stub appears in subsequent searches. Locks Item 04
/// sub-case 1 of the graph-correctness contract.
#[test]
fn relate_rejects_malformed_target_id_with_typed_envelope() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--bad@chars$here".to_string(),
            rel_type: "REFERENCES".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        result.is_error.unwrap_or(false),
        "malformed target id must error: {:?}",
        extract_text(&result)
    );
    let sc = result
        .structured_content
        .as_ref()
        .expect("INVALID_ENTITY_ID must carry structured_content");
    assert_eq!(sc["code"], "INVALID_ENTITY_ID");
    assert_eq!(sc["details"]["id"], "specs--bad@chars$here");
    assert!(
        sc["details"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("wiki-link grammar")),
        "reason must name the grammar rule: {sc:?}"
    );

    // Sanity: no stub got created at the malformed id.
    let read = server.memstead_entity(Parameters(EntityParams {
        id: "specs--bad@chars$here".to_string(),
        sections: None,
        include_relations: None,
        include_context: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(
        read.is_error.unwrap_or(false),
        "no stub should exist at the malformed id"
    );
}

/// `memstead_relate` add path with a shape-violating pair on a
/// mem pinned to `software@0.1.0` returns the typed
/// `INVALID_REL_SHAPE` envelope with the documented
/// `details.*` payload. Locks Item 03 of the graph-correctness contract.
#[test]
fn relate_shape_violation_surfaces_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("software-mem");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"software@0.1.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    // Two specs — the source must be `spec` so OWNS (source_types=[actor])
    // is shape-violating; the target type does not gate the source-side
    // check.
    fs::write(
            mem_dir.join("source-spec.md"),
            "---\ntype: spec\ncreated_date: 2026-05-13\nlast_modified: 2026-05-13\nlevel: M0\nstability: evolving\n---\n# Source Spec\n\n## Identity\nSource entity body.\n\n## Purpose\nForcing a shape-violating OWNS.\n",
        )
        .unwrap();
    fs::write(
            mem_dir.join("target-spec.md"),
            "---\ntype: spec\ncreated_date: 2026-05-13\nlast_modified: 2026-05-13\nlevel: M0\nstability: evolving\n---\n# Target Spec\n\n## Identity\nTarget entity body.\n\n## Purpose\nReceiver of the OWNS attempt.\n",
        )
        .unwrap();
    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let engine = setup_unified_test_engine(tmp.path());
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    // Sanity: the spec source exists.
    let _ = server.memstead_entity(Parameters(EntityParams {
        id: "software-mem--source-spec".to_string(),
        sections: None,
        include_relations: None,
        include_context: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));

    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "software-mem--source-spec".to_string(),
            to: "software-mem--target-spec".to_string(),
            rel_type: "OWNS".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        result.is_error.unwrap_or(false),
        "shape violation must error: {:?}",
        extract_text(&result)
    );
    let sc = result
        .structured_content
        .as_ref()
        .expect("INVALID_REL_SHAPE must carry structured_content");
    assert_eq!(sc["code"], "INVALID_REL_SHAPE");
    assert_eq!(sc["details"]["rel_type"], "OWNS");
    assert_eq!(sc["details"]["from_type"], "spec");
    assert_eq!(sc["details"]["to_type"], "spec");
    let allowed_source = sc["details"]["allowed_source_types"]
        .as_array()
        .expect("allowed_source_types array");
    assert!(
        allowed_source.iter().any(|v| v == "actor"),
        "allowed_source_types must contain 'actor': {sc:?}"
    );
    // OWNS has no target_types restriction — the structured payload
    // omits the field when unconstrained (absence = "any"); the text
    // channel renders "allowed targets: any" inline.
    assert!(
        sc["details"].get("allowed_target_types").is_none(),
        "allowed_target_types must be omitted when unconstrained: {sc:?}"
    );
    let text = extract_text(&result);
    assert!(
        text.contains("allowed targets: any"),
        "text message must render `allowed targets: any` when target_types is unconstrained: {text}"
    );
}

/// `memstead_relate remove=true` skips shape validation so an edge
/// authored before the constraint landed remains cleanable.
/// Locks the migration path for Item 03.
#[test]
fn relate_remove_skips_shape_validation() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("software-mem");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"software@0.1.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    // Seed an entity that already has a shape-violating OWNS edge
    // baked into its markdown — simulates a pre-constraint state.
    fs::write(
            mem_dir.join("legacy-spec.md"),
            "---\ntype: spec\ncreated_date: 2026-05-13\nlast_modified: 2026-05-13\nlevel: M0\nstability: evolving\n---\n# Legacy Spec\n\n## Identity\nCarries a pre-constraint OWNS edge.\n\n## Purpose\nVerifies the cleanup path.\n\n## Relationships\n- **OWNS**: [[legacy-target]]\n",
        )
        .unwrap();
    fs::write(
            mem_dir.join("legacy-target.md"),
            "---\ntype: spec\ncreated_date: 2026-05-13\nlast_modified: 2026-05-13\nlevel: M0\nstability: evolving\n---\n# Legacy Target\n\n## Identity\nReceives the shape-violating OWNS.\n\n## Purpose\nMust remain removable.\n",
        )
        .unwrap();
    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let engine = setup_unified_test_engine(tmp.path());
    let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

    // Remove the existing shape-violating edge — must succeed.
    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "software-mem--legacy-spec".to_string(),
            to: "software-mem--legacy-target".to_string(),
            rel_type: "OWNS".to_string(),
            remove: Some(true),
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "remove path must not error on shape-violating edge: {:?}",
        extract_text(&result)
    );
}

#[test]
fn test_empty_id_returns_validation_error() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_entity(Parameters(EntityParams {
        id: "".to_string(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(result.is_error.unwrap_or(false));
    assert!(extract_text(&result).contains("must not be empty"));
}

#[test]
fn test_long_id_returns_validation_error() {
    let (server, _tmp) = setup_dual_test_engine();
    let long_id = "a".repeat(201);
    let result = server.memstead_entity(Parameters(EntityParams {
        id: long_id,
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(result.is_error.unwrap_or(false));
    assert!(extract_text(&result).contains("too long"));
}

/// `memstead_health.limit` is clamped at 100 to keep an over-eager
/// caller from materialising the whole graph as `most_connected`
/// records. The clamp surfaces as the uniform warning envelope
/// `{ code: "LIMIT_CLAMPED", message, details: { requested, actual } }`
/// so callers can branch on `code` without reparsing the message.
#[test]
fn health_limit_clamps_with_warning() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["most_connected".to_string()]),
        limit: Some(1000),
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "health call failed: {}",
        extract_text(&result)
    );
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();

    // Detail list never exceeds the 100-entry cap (the fixture is small,
    // so this is more about the contract than the count itself).
    let connected = json["most_connected"]
        .as_array()
        .expect("most_connected list present when included");
    assert!(
        connected.len() <= 100,
        "most_connected exceeds clamp: {} entries",
        connected.len()
    );

    // Warning surfaces the clamp as a typed `WarningHint::LimitClamped`.
    let warnings = json["warnings"]
        .as_array()
        .expect("clamped limit must surface a warnings entry");
    let clamp = warnings
        .iter()
        .find(|w| w["code"].as_str() == Some("LIMIT_CLAMPED"))
        .expect("expected a `code: \"LIMIT_CLAMPED\"` warning envelope");
    assert_eq!(clamp["details"]["requested"].as_u64(), Some(1000));
    assert_eq!(clamp["details"]["actual"].as_u64(), Some(100));
}

/// No clamp triggered ⇒ no `LIMIT_CLAMPED` entry.
/// The `OUTER_REPO_NOT_IGNORING_MEM_REPO` warning may fire when the
/// test workspace is embedded under a `/var/folders/.../.git`
/// leftover — that's an environmental warning unrelated to the
/// clamp behaviour the test pins. Filter to `LIMIT_CLAMPED`
/// specifically rather than asserting an empty `warnings` field.
#[test]
fn health_limit_under_cap_emits_no_warning() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["most_connected".to_string()]),
        limit: Some(10),
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let text = extract_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let clamp = json
        .get("warnings")
        .and_then(|w| w.as_array())
        .map(|arr| {
            arr.iter()
                .any(|w| w["code"].as_str() == Some("LIMIT_CLAMPED"))
        })
        .unwrap_or(false);
    assert!(!clamp, "no clamp ⇒ no LIMIT_CLAMPED warning; got {}", json);
}

/// `memstead_update` response must carry the structured
/// `modified_sections` / `modified_metadata` objects. A mixed-mode
/// call populates every sub-bucket exactly once; empty sub-vecs are
/// serde-omitted so callers don't have to special-case `[]`. The
/// wire shape is a stable object-of-arrays, not a flat
/// `modified_fields` string vec with mode-prefix encoding.
#[test]
fn update_response_wire_shape() {
    let (server, _tmp) = setup_dual_test_engine();

    // Bootstrap a fresh entity with a `tags` metadata and
    // `constraints` section body so we have something to unset and
    // something to patch.
    let create = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        title: "Wire Shape".to_string(),
        entity_type: "spec".to_string(),
        mem: Some("specs".to_string()),
        sections: Some(IndexMap::from_iter([
            ("identity".to_string(), "i".to_string()),
            ("purpose".to_string(), "p".to_string()),
            ("constraints".to_string(), "drop me".to_string()),
        ])),
        metadata: Some(IndexMap::from_iter([(
            "tags".to_string(),
            "x, y".to_string(),
        )])),
        relations: None,
        dry_run: Some(false),

        note: None,

        role: None,
        identity: None,
    }));
    let create_json: serde_json::Value = serde_json::from_str(&extract_text(&create)).unwrap();
    let id = create_json["id"].as_str().unwrap().to_string();
    let hash = create_json["_hash"].as_str().unwrap().to_string();

    // Mixed-mode update: replace, append, patch, metadata-set,
    // metadata-unset all in one call.
    let result = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        sections_unset: None,
        id,
        expected_hash: Some(hash),
        sections: Some(IndexMap::from_iter([(
            "identity".to_string(),
            "replaced.".to_string(),
        )])),
        append_sections: Some(IndexMap::from_iter([(
            "purpose".to_string(),
            "tail.".to_string(),
        )])),
        patch_sections: Some(IndexMap::from_iter([(
            "constraints".to_string(),
            crate::tools::mutation::PatchesInput::One(crate::tools::mutation::PatchInput {
                old: "drop me".to_string(),
                new: "keep me".to_string(),
                all: Some(false),
            }),
        )])),
        metadata: Some(IndexMap::from_iter([(
            "level".to_string(),
            "M1".to_string(),
        )])),
        metadata_unset: Some(vec!["tags".to_string()]),
        dry_run: Some(false),

        note: None,

        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "mixed update must succeed: {}",
        extract_text(&result)
    );
    let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();

    // Parent keys are always present as objects — stable shape for
    // callers.
    assert!(
        json["modified_sections"].is_object(),
        "modified_sections must be an object, got {}",
        json["modified_sections"]
    );
    assert!(
        json["modified_metadata"].is_object(),
        "modified_metadata must be an object, got {}",
        json["modified_metadata"]
    );

    // Sub-vecs carry exactly the keys we set — no mode-prefix
    // encoding, no cross-bucket noise.
    assert_eq!(
        json["modified_sections"]["replaced"],
        serde_json::json!(["identity"])
    );
    assert_eq!(
        json["modified_sections"]["appended"],
        serde_json::json!(["purpose"])
    );
    assert_eq!(
        json["modified_sections"]["patched"],
        serde_json::json!(["constraints"])
    );
    assert_eq!(
        json["modified_metadata"]["set"],
        serde_json::json!(["level"])
    );
    assert_eq!(
        json["modified_metadata"]["unset"],
        serde_json::json!(["tags"])
    );

    // The old flat field must be gone — guards against a future
    // backward-compat shim leaking it back.
    assert!(
        json.get("modified_fields").is_none(),
        "modified_fields must not be on the wire anymore"
    );
}

/// `memstead_update` dry-run returns BOTH the unchanged on-disk hash
/// (as `_hash`) AND the post-write hash the proposed change
/// would produce (as `prospective_hash`). The agent uses
/// `_hash` as the `expected_hash` on the follow-up real call
/// (the disk file is unchanged, so optimistic locking accepts it)
/// and predicts the post-write `_hash` via
/// `prospective_hash`.
///
/// End-to-end round-trip: create → dry-run → real update with the
/// dry-run's `_hash`. Asserts (a) `prospective_hash` differs from
/// `_hash` (otherwise the dry-run wasn't really previewing a
/// change), (b) the real call succeeds with the dry-run's `_hash`
/// as the lock, and (c) the real call's returned `_hash` matches
/// the dry-run's `prospective_hash`.
#[test]
fn update_dry_run_returns_prospective_and_current_hash() {
    let (server, _tmp) = setup_dual_test_engine();
    // Pin the mutation clock: the auto-stamped `last_modified`
    // enters the hash, so the prospective == real post-write
    // equality assertion below is only deterministic under a
    // frozen clock (unpinned, it fails across a second tick).
    server
        .unified_engine()
        .lock()
        .unwrap()
        .set_mutation_clock(std::sync::Arc::new(|| {
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_754_000_000)
        }));
    let id = "specs--entity-a".to_string();

    // Read current hash via memstead_entity.
    let entity_result = server.memstead_entity(Parameters(EntityParams {
        id: id.clone(),
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    let entity_text = extract_text(&entity_result);
    let initial_hash = entity_text
        .lines()
        .find(|l| l.starts_with("_hash:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .expect("entity response must include _hash frontmatter");

    // Dry-run update.
    let dry_run = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: id.clone(),
        expected_hash: Some(initial_hash.clone()),
        sections: Some(IndexMap::from_iter([(
            "identity".to_string(),
            "Dry-run preview content.".to_string(),
        )])),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: Some(true),

        note: None,

        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(
        !dry_run.is_error.unwrap_or(false),
        "dry-run failed: {}",
        extract_text(&dry_run)
    );
    let dry_text = extract_text(&dry_run);
    let dry_json: serde_json::Value = serde_json::from_str(&dry_text).unwrap();

    let dry_current_hash = dry_json["_hash"].as_str().unwrap().to_string();
    let prospective_hash = dry_json["prospective_hash"]
        .as_str()
        .expect("dry-run must return prospective_hash")
        .to_string();
    // Disk file untouched ⇒ content_hash echoes the entity's current hash.
    assert_eq!(
        dry_current_hash, initial_hash,
        "dry-run content_hash must mirror current on-disk hash"
    );
    // Proposed change must shift the hash; otherwise the test isn't
    // really previewing anything.
    assert_ne!(
        dry_current_hash, prospective_hash,
        "prospective_hash must differ from current_hash for a non-trivial change"
    );

    // Real update with the dry-run's content_hash (= the unchanged disk
    // hash) as the lock. This is the documented "preview → commit"
    // pattern.
    let real = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: id.clone(),
        expected_hash: Some(dry_current_hash),
        sections: Some(IndexMap::from_iter([(
            "identity".to_string(),
            "Dry-run preview content.".to_string(),
        )])),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: Some(false),

        note: None,

        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(
        !real.is_error.unwrap_or(false),
        "real update failed: {}",
        extract_text(&real)
    );
    let real_text = extract_text(&real);
    let real_json: serde_json::Value = serde_json::from_str(&real_text).unwrap();

    // Real call must NOT carry a prospective_hash (Option::is_none is
    // skipped in serialization).
    assert!(
        real_json.get("prospective_hash").is_none(),
        "real (non-dry-run) update must omit prospective_hash; got {}",
        real_json
    );

    // The real call's post-write content_hash must match the prospective
    // hash the dry-run predicted.
    let real_hash = real_json["_hash"].as_str().unwrap().to_string();
    assert_eq!(
        real_hash, prospective_hash,
        "real post-write hash must equal dry-run's prospective_hash"
    );
}

/// `dry_run` is a recovery path: a stale `expected_hash` must not
/// reject a dry-run call; the response carries the current on-disk
/// hash as `_hash`, which the agent uses as `expected_hash`
/// on the real follow-up.
#[test]
fn update_dry_run_recovers_stale_hash() {
    let (server, _tmp) = setup_dual_test_engine();
    let id = "specs--entity-a".to_string();

    let stale = "0".repeat(64);
    let dry_run = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id: id.clone(),
        expected_hash: Some(stale.clone()),
        sections: Some(IndexMap::from_iter([(
            "identity".to_string(),
            "Recovery preview.".to_string(),
        )])),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: Some(true),

        note: None,

        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(
        !dry_run.is_error.unwrap_or(false),
        "dry_run must ignore stale expected_hash: {}",
        extract_text(&dry_run)
    );
    let dry_json: serde_json::Value = serde_json::from_str(&extract_text(&dry_run)).unwrap();
    let recovered = dry_json["_hash"].as_str().unwrap();
    assert_ne!(
        recovered, stale,
        "content_hash must be the real on-disk hash, not the stale input"
    );
    // MCP serialises the engine's truncated SHA-256 (16 hex chars).
    assert_eq!(
        recovered.len(),
        16,
        "content_hash must be truncated SHA-256 hex"
    );

    // Follow-up real call with the recovered hash must succeed.
    let real = server.memstead_update(Parameters(UpdateParams {
        anchors: None,
        relations_unset: None,
        anchors_unset: None,
        id,
        expected_hash: Some(recovered.to_string()),
        sections: Some(IndexMap::from_iter([(
            "identity".to_string(),
            "Recovery preview.".to_string(),
        )])),
        append_sections: None,
        patch_sections: None,
        sections_unset: None,
        metadata: None,
        metadata_unset: None,
        dry_run: Some(false),

        note: None,

        role: None,
        identity: None,
        declare_relations: None,
    }));
    assert!(
        !real.is_error.unwrap_or(false),
        "real follow-up failed: {}",
        extract_text(&real)
    );
}

/// `memstead_delete` requires `expected_hash`. A stale hash must
/// produce an engine `HashMismatch` and leave the entity intact
/// (mirror of the contract `memstead_update` / `memstead_rename` enforce).
#[test]
fn delete_rejects_stale_hash() {
    let (server, _tmp) = setup_dual_test_engine();
    let id = "specs--entity-a".to_string();

    let result = server.memstead_delete(Parameters(DeleteParams {
        id: id.clone(),
        expected_hash: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),

        note: None,

        role: None,
        identity: None,
    }));
    assert!(
        result.is_error.unwrap_or(false),
        "delete with stale hash must fail"
    );

    // HashMismatch must carry a structured payload so agents can recover
    // without a follow-up memstead_entity read.
    let sc = result
        .structured_content
        .as_ref()
        .expect("HashMismatch must carry structured_content");
    assert_eq!(sc["code"], "HASH_MISMATCH");
    let current = sc["details"]["current"]
        .as_str()
        .expect("details.current must be a string");
    // `compute_hash` returns truncated SHA-256 (first 16 hex chars) —
    // same shape content_hash carries throughout the surface.
    assert_eq!(
        current.len(),
        16,
        "current must be truncated SHA-256 hex (16 chars), got {current:?}"
    );
    assert!(
        current.chars().all(|c| c.is_ascii_hexdigit()),
        "current must be hex, got {current:?}"
    );

    // Entity must survive the failed delete — prove it with a live read.
    let entity_result = server.memstead_entity(Parameters(EntityParams {
        id,
        include_relations: None,
        include_context: None,
        sections: None,
        token_budget: None,
        chunk: None,
        include_provenance: None,
    }));
    assert!(
        !entity_result.is_error.unwrap_or(false),
        "entity must still be readable after failed delete"
    );
}

/// `memstead_rename` response must carry a non-empty `_hash` on both
/// a real rename and the slug-noop short-circuit, so agents can chain
/// the next hash-protected op (memstead_update / memstead_delete / memstead_rename)
/// without a fresh memstead_entity read. Mirrors what memstead_relate already
/// returns — locks the cross-tool contract in wire-shape form.
#[test]
fn rename_response_wire_shape() {
    let (server, _tmp) = setup_dual_test_engine();

    // Bootstrap a fresh entity via memstead_create to get a known-valid hash
    // without parsing memstead_entity's markdown frontmatter.
    let create = server.memstead_create(Parameters(CreateParams {
        anchors: None,
        title: "Rename Wire".to_string(),
        entity_type: "spec".to_string(),
        mem: Some("specs".to_string()),
        sections: Some(IndexMap::from_iter([
            ("identity".to_string(), "x".to_string()),
            ("purpose".to_string(), "y".to_string()),
        ])),
        metadata: None,
        relations: None,
        dry_run: Some(false),

        note: None,

        role: None,
        identity: None,
    }));
    let create_json: serde_json::Value = serde_json::from_str(&extract_text(&create)).unwrap();
    let id = create_json["id"].as_str().unwrap().to_string();
    let create_hash = create_json["_hash"].as_str().unwrap().to_string();

    // Real rename → non-empty content_hash on the response.
    let real = server.memstead_rename(Parameters(RenameParams {
        id: id.clone(),
        new_title: "Rename Wire Changed".to_string(),
        expected_hash: create_hash.clone(),

        note: None,

        role: None,
        identity: None,
    }));
    assert!(
        !real.is_error.unwrap_or(false),
        "real rename must succeed: {}",
        extract_text(&real)
    );
    let real_json: serde_json::Value = serde_json::from_str(&extract_text(&real)).unwrap();
    let real_hash = real_json["_hash"]
        .as_str()
        .expect("content_hash must be a string on real rename");
    assert_eq!(
        real_hash.len(),
        16,
        "content_hash must be truncated SHA-256 hex"
    );

    // Slug-noop → echoes the current on-disk hash of the renamed entity.
    let noop = server.memstead_rename(Parameters(RenameParams {
        id: real_json["new_id"].as_str().unwrap().to_string(),
        new_title: "RENAME WIRE CHANGED".to_string(), // case-only — same slug
        expected_hash: real_hash.to_string(),

        note: None,

        role: None,
        identity: None,
    }));
    assert!(
        !noop.is_error.unwrap_or(false),
        "slug-noop rename must succeed: {}",
        extract_text(&noop)
    );
    let noop_json: serde_json::Value = serde_json::from_str(&extract_text(&noop)).unwrap();
    assert_eq!(
        noop_json["old_id"], noop_json["new_id"],
        "slug-noop must keep the id unchanged"
    );
    assert_eq!(
        noop_json["_hash"].as_str().unwrap(),
        real_hash,
        "slug-noop echoes the unchanged on-disk hash"
    );
}

/// Re-adding the same edge returns success but surfaces a typed
/// `WarningHint::DuplicateRelationship` on the wire (envelope
/// `code: "DUPLICATE_RELATIONSHIP"`).
#[test]
fn relate_duplicate_emits_typed_warning() {
    let (server, _tmp) = setup_dual_test_engine();

    // First call — establishes the edge cleanly.
    let first = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "DEPENDS_ON".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        !first.is_error.unwrap_or(false),
        "first relate failed: {}",
        extract_text(&first)
    );
    let first_json: serde_json::Value = serde_json::from_str(&extract_text(&first)).unwrap();
    assert!(
        first_json.get("warnings").is_none()
            || first_json["warnings"]
                .as_array()
                .map(|a| {
                    !a.iter()
                        .any(|w| w["code"].as_str() == Some("DUPLICATE_RELATIONSHIP"))
                })
                .unwrap_or(true),
        "first call must not carry duplicate warning; got {first_json}"
    );

    // Second call — same edge, must succeed AND emit the typed warning.
    let second = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "DEPENDS_ON".to_string(),
            remove: None,
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        !second.is_error.unwrap_or(false),
        "duplicate relate must succeed: {}",
        extract_text(&second)
    );
    let second_json: serde_json::Value = serde_json::from_str(&extract_text(&second)).unwrap();
    let warnings = second_json["warnings"]
        .as_array()
        .expect("duplicate relate must carry warnings");
    let dup = warnings
        .iter()
        .find(|w| w["code"].as_str() == Some("DUPLICATE_RELATIONSHIP"))
        .expect("DuplicateRelationship warning expected on repeat add");
    assert_eq!(dup["details"]["rel_type"].as_str(), Some("DEPENDS_ON"));
    assert_eq!(dup["details"]["from"].as_str(), Some("specs--entity-a"));
    assert_eq!(dup["details"]["to"].as_str(), Some("specs--entity-b"));
}

/// Removing an edge that was never there returns success but
/// surfaces a typed `WarningHint::NoSuchRelationship` on the wire.
#[test]
fn relate_remove_nonexistent_emits_typed_warning() {
    let (server, _tmp) = setup_dual_test_engine();

    // No DEPENDS_ON edge exists from a→b yet; removing it is a no-op.
    let result = server.memstead_relate(Parameters(RelateParams {
        relations: vec![RelateOpInput {
            from: "specs--entity-a".to_string(),
            to: "specs--entity-b".to_string(),
            rel_type: "DEPENDS_ON".to_string(),
            remove: Some(true),
            description: None,
        }],
        note: None,
        role: None,
        identity: None,
        dry_run: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "remove-nonexistent must succeed: {}",
        extract_text(&result)
    );
    let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
    let warnings = json["warnings"]
        .as_array()
        .expect("remove-nonexistent must carry warnings");
    let miss = warnings
        .iter()
        .find(|w| w["code"].as_str() == Some("NO_SUCH_RELATIONSHIP"))
        .expect("NoSuchRelationship warning expected on remove no-op");
    assert_eq!(miss["details"]["rel_type"].as_str(), Some("DEPENDS_ON"));
    assert_eq!(miss["details"]["from"].as_str(), Some("specs--entity-a"));
    assert_eq!(miss["details"]["to"].as_str(), Some("specs--entity-b"));
}

/// An unknown `include` key emits a typed
/// `WarningHint::UnknownIncludeKey` with the allowed list echoed
/// back so the agent can self-correct.
#[test]
fn health_unknown_include_emits_typed_warning() {
    let (server, _tmp) = setup_dual_test_engine();
    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["orphans".into(), "bogus".into()]),
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "health call failed: {}",
        extract_text(&result)
    );
    let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
    // Known key still materialises its detail list.
    assert!(
        json["orphans"].is_array(),
        "orphans detail must still appear alongside bogus key"
    );
    let warnings = json["warnings"]
        .as_array()
        .expect("unknown include key must surface a warnings entry");
    let unknown = warnings
        .iter()
        .find(|w| w["code"].as_str() == Some("UNKNOWN_INCLUDE_KEY"))
        .expect("UnknownIncludeKey warning expected");
    assert_eq!(unknown["details"]["key"].as_str(), Some("bogus"));
    let allowed = unknown["details"]["allowed"]
        .as_array()
        .expect("allowed list must echo back to the caller");
    // Canonical set — keeps the wire contract stable for machine consumers.
    let allowed_set: std::collections::HashSet<String> = allowed
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    for expected in [
        "orphans",
        "stubs",
        "most_connected",
        "missing_fields",
        "stale",
        "dangling_links",
        "tags",
    ] {
        assert!(
            allowed_set.contains(expected),
            "allowed list must include `{expected}`: got {allowed:?}"
        );
    }
}

/// An inline `[[id]]` in an entity's section body whose target has
/// no on-disk markdown file (post-delete, rename-without-rewrite,
/// or typo) surfaces as a `dangling_links` detail entry when the
/// caller opts in via `include=["dangling_links"]`.
#[test]
fn health_dangling_links_surfaces_stub_targets() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    // Only `a.md` on disk — its body references `[[gone]]`, which
    // has no file and therefore auto-stubs at load time. The stub
    // is the dangling signal.
    fs::write(
            mem_dir.join("a.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# A\n\n## Identity\n\nFixture.\n\n## Purpose\n\nRefers to [[gone]] in prose.\n",
        )
        .unwrap();

    memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
    let server = McpServer::new(
        setup_unified_test_engine(tmp.path()),
        crate::config::DEFAULT_TOKEN_BUDGET,
    );

    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["dangling_links".to_string()]),
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    assert!(
        !result.is_error.unwrap_or(false),
        "health call failed: {}",
        extract_text(&result)
    );
    let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
    let arr = json["dangling_links"]
        .as_array()
        .expect("dangling_links array present when key opted in");
    assert_eq!(arr.len(), 1, "exactly one dangling link expected: {arr:?}");
    let entry = &arr[0];
    assert_eq!(entry["from"].as_str(), Some("specs--a"));
    assert_eq!(entry["target_id"].as_str(), Some("specs--gone"));
    assert_eq!(entry["target_path"].as_str(), Some("gone"));
    assert_eq!(entry["section"].as_str(), Some("purpose"));
    // The condition reaches the response, not just the code (04/06).
    // The old assertion set was satisfied by a payload that said
    // nothing about which of three repairs applies.
    assert_eq!(entry["kind"].as_str(), Some("DANGLING_LINK_TARGET_MISSING"));
}

/// `include=["tags"]` populates `tag_distribution`,
/// `tag_distribution_folded`, and `untagged_entities`; absence of
/// the key leaves every field unset (matches the `dangling_links`
/// handler-driven contract).
#[test]
fn health_tags_include_switch_emits_field() {
    let (server, _tmp) = setup_dual_test_engine();

    // With include=["tags"]: every field present.
    let result = server.memstead_health(Parameters(HealthParams {
        include: Some(vec!["tags".to_string()]),
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    assert!(!result.is_error.unwrap_or(false));
    let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
    assert!(
        json.get("tag_distribution").is_some(),
        "tag_distribution present when key opted in"
    );
    assert!(
        json["tag_distribution"].is_array(),
        "tag_distribution must be an array"
    );
    assert!(
        json.get("untagged_entities").is_some(),
        "untagged_entities present when key opted in"
    );
    assert!(
        json["untagged_entities"].is_object(),
        "untagged_entities must be an object"
    );
    assert!(
        json.get("tag_distribution_folded").is_some(),
        "tag_distribution_folded present when key opted in"
    );

    // Without include: none of the three appear.
    let result_no_include = server.memstead_health(Parameters(HealthParams {
        include: None,
        limit: None,
        mem: None,
        include_config: false,
        token_budget: None,
        chunk: None,
        target_schema: None,
    }));
    let json_no: serde_json::Value =
        serde_json::from_str(&extract_text(&result_no_include)).unwrap();
    assert!(json_no.get("tag_distribution").is_none());
    assert!(json_no.get("tag_distribution_folded").is_none());
    assert!(json_no.get("untagged_entities").is_none());
}

// ------------------------------------------------------------------
// memstead_mem_create — envelope + happy-path wire tests.
//
// The engine-level contract is covered in
// `memstead-git-branch/tests/mem_management.rs`; these tests pin the
// MCP-layer envelope shape (structured_content `{code, message,
// details}`) and the success-path JSON response so the contract the
// agent sees doesn't drift from the handler's wire surface.
// ------------------------------------------------------------------

use crate::lifecycle::MemCreateParams as TlsMemCreateParams;
use memstead_base::WorkspaceSettings;

/// Build an `McpServer` around an empty engine with permissive
/// `[[mem_management.create]]` rules rooted at the given
/// TempDir. Two patterns: a flat `*` (single-segment leaves) and a
/// `**` (any depth, including hierarchical path-and-leaf
/// candidates) — the minimum surface `memstead_mem_create` tests
/// need to exercise both flat and hierarchical layouts under
/// gitignore-style matching.
fn setup_lifecycle_server(tmp: &TempDir) -> McpServer {
    // The engine produces real 40-char hex `seed_write_id`
    // values when mounted on a mem-repo (backend factory +
    // storage heuristic). Use the git-branch test-engine helper
    // so memstead_mem_create asserts against real commit shas.
    let full_settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        WorkspaceSettings {
            mem_create_rules: vec![
                memstead_base::CreateRuleSetting {
                    pattern: "*".to_string(),
                    schemas: vec!["default@1.0.0".to_string()],
                    default_cross_links: None,
                },
                memstead_base::CreateRuleSetting {
                    pattern: "**".to_string(),
                    schemas: vec!["default@1.0.0".to_string()],
                    default_cross_links: None,
                },
            ],
            mem_delete_rules: vec![],
            ..Default::default()
        },
    );
    let unified_settings = full_settings.clone();
    let mut unified = setup_unified_test_engine_git_branch(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET)
}

#[test]
fn memstead_mem_create_happy_path_returns_seed_sha() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server(&tmp);
    let target = tmp.path().join("fresh");
    let result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "fresh".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: Some("mcp handler happy path".to_string()),
        recovery: None,
        include_schema: false,
    }));
    assert!(
        result.is_error.is_none() || result.is_error == Some(false),
        "happy path must not be an error: {:?}",
        result
    );
    let json: serde_json::Value =
        serde_json::from_str(&extract_text(&result)).expect("response must be JSON");
    assert_eq!(json["name"], "fresh");
    assert!(
        json["seed_write_id"]
            .as_str()
            .is_some_and(|s| s.len() == 40),
        "seed_write_id must be a 40-char hex string: {}",
        json["seed_write_id"]
    );
    assert!(
        json.get("belongs_to").is_none(),
        "belongs_to is dropped from the response (workspace-cross-link-policy)"
    );
}

/// Goal 2 (hierarchical content branches): a `memstead_mem_create`
/// call that supplies an organizational `path` lands the new
/// content branch at `refs/heads/<path>/<leaf>` and the matching
/// per-mem config at `__MEMSTEAD:mems/<path>/<leaf>/config.json`. The
/// leaf-only API surface still works — `read_config(ws, leaf)`
/// transparently resolves to the full hierarchical path.
#[test]
fn memstead_mem_create_hierarchical_path_lands_branch_and_config() {
    // Hierarchical paths are first-class. `name = "planning/hier"` is the
    // canonical hierarchical-mem input — no separate `path`
    // wire field. The branch ref + `__MEMSTEAD` config blob land at
    // `refs/heads/planning/hier` / `mems/planning/hier/config.json`.
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server(&tmp);
    let target = tmp.path().join("hier");
    let result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "planning/hier".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: Some("hierarchical layout".to_string()),
        recovery: None,
        include_schema: false,
    }));
    assert!(
        result.is_error.is_none() || result.is_error == Some(false),
        "happy path with hierarchical name must succeed: {:?}",
        result
    );

    // Branch landed at `refs/heads/planning/hier`.
    let gitdir = tmp.path().join("mem-repo").join(".git");
    let repo = gix::open(&gitdir).expect("mem-repo must open");
    assert!(
        matches!(
            repo.try_find_reference("refs/heads/planning/hier"),
            Ok(Some(_))
        ),
        "hierarchical content branch refs/heads/planning/hier must exist"
    );
    assert!(
        matches!(repo.try_find_reference("refs/heads/hier"), Ok(None)),
        "flat fallback refs/heads/hier must NOT exist for hierarchical create"
    );

    // Config readable by leaf (resolver maps leaf → full path).
    let cfg = memstead_git_branch::mem_repo_config::read_config(tmp.path(), "hier")
        .expect("read_config must resolve leaf to hierarchical path");
    // Goal 3 made `name` optional in the on-disk config. The
    // engine omits it on writes; the legacy fixture path may
    // populate it. Tolerate both.
    assert!(cfg.name.is_none() || cfg.name.as_deref() == Some("hier"));
}

/// A mem-create whose leaf already exists in the mem-repo at
/// a different organizational path is rejected with
/// `MEM_NAME_COLLISION` before any disk side effect lands. The
/// envelope payload carries the colliding full paths so the agent
/// can disambiguate without a second round trip.
#[test]
fn memstead_mem_create_tree_walk_collision_rejected_with_paths() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server(&tmp);

    // Seed an existing hierarchical branch under `demo/engine`
    // via a successful create. After this, leaf `engine` is
    // sealed at `refs/heads/demo/engine`.
    let first_target = tmp.path().join("engine");
    let first = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "engine".to_string(),
        location: first_target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert!(
        first.is_error.is_none() || first.is_error == Some(false),
        "first hierarchical create must succeed: {:?}",
        first
    );

    // Second create with the same leaf at a different path. The
    // memory-router probe ALSO catches this (engine was registered),
    // but the tree-walk probe enriches the envelope with
    // `colliding_paths` and `suggestion` regardless.
    let second_target = tmp.path().join("engine-second");
    let second = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "engine".to_string(),
        location: second_target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert_eq!(
        second.is_error,
        Some(true),
        "second create with colliding leaf must surface an error envelope"
    );

    let envelope = second
        .structured_content
        .clone()
        .expect("collision must carry envelope");
    assert_eq!(envelope["code"], "MEM_NAME_COLLISION");
    let details = &envelope["details"];
    assert_eq!(details["name"], "engine");
    // Collisions surface through the snapshot probe
    // (`mem_router.origin_for_mem`) with the `{name, source}`
    // payload. Agents branch on `code` only. The collision is
    // detected and rejected before any disk write at the second
    // target.
    assert!(
        !second_target.exists(),
        "collision must reject before disk side effects"
    );
}

/// The mem-name
/// grammar refuses malformed hierarchical paths (leading slash,
/// double slash, etc.) before any disk side effect lands.
/// `INVALID_INPUT` envelope is returned.
#[test]
fn memstead_mem_create_rejects_invalid_name_grammar() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server(&tmp);
    let target = tmp.path().join("bad");
    let result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "/leading-slash".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: Some("invalid grammar".to_string()),
        recovery: None,
        include_schema: false,
    }));
    assert!(
        result.is_error == Some(true),
        "invalid name grammar must surface as an error envelope"
    );
    let text = extract_text(&result);
    // Structural-failure
    // refusals surface as the typed `INVALID_MEM_NAME` code
    // with a `details.reason` discriminator. `/leading-slash`
    // would fail the regex grammar, classified as `invalid_char`.
    assert!(
        text.contains("INVALID_MEM_NAME"),
        "envelope must surface INVALID_MEM_NAME refusal, got: {}",
        text
    );
}

#[test]
fn memstead_mem_create_path_not_allowed_emits_structured_envelope() {
    // Empty allowlist surfaces MEM_PATH_NOT_ALLOWED through
    // `memstead_base::mem_management::create_mem`'s pre-check.
    let tmp = TempDir::new().unwrap();
    // Empty allowlist.
    let settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        WorkspaceSettings {
            mem_create_rules: vec![],
            mem_delete_rules: vec![],
            ..Default::default()
        },
    );
    let unified_settings = settings.clone();
    let _ = settings;
    let mut unified = setup_unified_test_engine(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let target = tmp.path().join("blocked");
    let result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "blocked".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert_eq!(result.is_error, Some(true));
    let envelope = result
        .structured_content
        .expect("error must carry structured envelope");
    assert_eq!(envelope["code"], "MEM_PATH_NOT_ALLOWED");
    assert!(envelope.get("message").is_some());
    let details = envelope
        .get("details")
        .expect("envelope must carry details");
    assert!(details.get("attempted").is_some());
    assert_eq!(details["patterns"], serde_json::json!([]));
    assert_eq!(details["reason"], "no_allowlist_configured");
    // The structured remedy reaches the MCP wire — the agent can
    // recover from `details` without parsing prose. It names the
    // CLI command only: policy is an operator surface, so there is
    // no MCP tool to point at.
    assert!(
        details["remedy"]["cli"]
            .as_str()
            .unwrap()
            .contains("memstead workspace allow-create"),
        "{details}"
    );
    assert!(details["remedy"]["mcp"].is_null(), "{details}");
}

#[test]
fn memstead_mem_create_name_collision_envelope_carries_source() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server(&tmp);

    // First create — succeeds. Basename must equal the name under
    // the basename-invariant, so the location is `tmp/same/`.
    let first = tmp.path().join("same");
    let ok = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "same".to_string(),
        location: first.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert!(ok.is_error.is_none() || ok.is_error == Some(false));

    // Second create — same name, different canonical location
    // (nested under `b/`). Basename still matches `name`.
    std::fs::create_dir_all(tmp.path().join("b")).unwrap();
    let second = tmp.path().join("b").join("same");
    let err = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "same".to_string(),
        location: second.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert_eq!(err.is_error, Some(true));
    let envelope = err
        .structured_content
        .expect("collision must carry envelope");
    assert_eq!(envelope["code"], "MEM_NAME_COLLISION");
    let source = envelope["details"]["source"]
        .as_str()
        .expect("details.source must be a string");
    // The snapshot probe renders as `runtime-created at <ts>
    // by memstead_mem_create`.
    assert!(
        source.contains("runtime-created") && source.contains("memstead_mem_create"),
        "snapshot-probe collision source must identify the runtime registration: {source}"
    );
}

/// Cold-boot persistence regression.
///
/// Reproduces a showstopper where a successful
/// `memstead_mem_create` writes the per-mem branch + `__MEMSTEAD`
/// config but NOT the workspace mount manifest, so the very next
/// CLI / MCP process boots without the new mem.
///
/// Asserts the engine-side persistence fix:
/// 1. The mount manifest (`.memstead/state/mounts.json`) is rewritten
///    on create — a fresh `FileWorkspaceStore::load` sees the
///    new mount.
/// 2. A second create with the same name surfaces
///    `MEM_NAME_COLLISION` (in-process; the in-memory router
///    already carries the mount). Combined with point 1, the
///    cold-boot collision is symmetric.
/// 3. After `memstead_mem_delete`, the manifest no longer carries
///    the mount.
#[test]
fn memstead_mem_create_persists_mount_for_cold_boot() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server_with_delete(&tmp);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());

    // No mount file yet — auto_seed doesn't create one when no
    // pre-existing disk mems are present.
    let mounts_path = canonical_root
        .join(".memstead")
        .join("state")
        .join("mounts.json");

    let target = canonical_root.join("persisted");
    let ok = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "persisted".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert!(
        ok.is_error.is_none() || ok.is_error == Some(false),
        "create must succeed: {:?}",
        ok
    );

    // Manifest now exists and a freshly-loaded workspace sees the
    // new mount.
    assert!(
        mounts_path.is_file(),
        "create must write {} so cold-boot sees the new mem",
        mounts_path.display()
    );
    let reloaded =
        <memstead_base::FileWorkspaceStore as memstead_base::WorkspaceStoreAdapter>::load(
            &memstead_base::FileWorkspaceStore::new(),
            &canonical_root,
        )
        .expect("reload must succeed");
    assert!(
        reloaded.mounts.iter().any(|m| m.mem == "persisted"),
        "cold-boot workspace must include the freshly-created mem: {:?}",
        reloaded.mounts.iter().map(|m| &m.mem).collect::<Vec<_>>()
    );

    // Second create with the same name trips MEM_NAME_COLLISION —
    // F3 follows from the persistence fix.
    let dup = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "persisted".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert_eq!(
        dup.is_error,
        Some(true),
        "second create of an already-persisted mem must collide"
    );
    let envelope = dup
        .structured_content
        .expect("collision must carry envelope");
    assert_eq!(envelope["code"], "MEM_NAME_COLLISION");

    // Delete persists too — the manifest no longer carries the
    // mount after a successful unregister.
    let del = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "persisted".to_string(),
        note: None,
    }));
    assert!(
        del.is_error.is_none() || del.is_error == Some(false),
        "delete must succeed: {:?}",
        del
    );
    let after_delete =
        <memstead_base::FileWorkspaceStore as memstead_base::WorkspaceStoreAdapter>::load(
            &memstead_base::FileWorkspaceStore::new(),
            &canonical_root,
        )
        .expect("post-delete reload must succeed");
    assert!(
        after_delete.mounts.iter().all(|m| m.mem != "persisted"),
        "cold-boot workspace must NOT include the deleted mem"
    );
}

#[test]
fn memstead_mem_create_invalid_schema_ref_emits_invalid_input() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server(&tmp);
    let result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "bad-schema".to_string(),
        location: tmp.path().join("bad-schema").to_string_lossy().into_owned(),
        schema: "this-is-not-a-ref".to_string(),

        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert_eq!(result.is_error, Some(true));
    let envelope = result
        .structured_content
        .expect("invalid ref must carry envelope");
    assert_eq!(envelope["code"], "INVALID_INPUT");
}

// ------------------------------------------------------------------
// memstead_mem_delete — envelope + happy-path wire tests.
// ------------------------------------------------------------------

use crate::lifecycle::MemDeleteParams as TlsMemDeleteParams;

/// Build an `McpServer` around an empty engine with matching create +
/// delete allowlists rooted at the given TempDir, so `memstead_mem_delete`
/// can actually unregister what a prior `memstead_mem_create` set up.
fn setup_lifecycle_server_with_delete(tmp: &TempDir) -> McpServer {
    let full_settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        WorkspaceSettings {
            mem_create_rules: vec![memstead_base::CreateRuleSetting {
                pattern: "*".to_string(),
                schemas: vec!["default@1.0.0".to_string()],
                default_cross_links: None,
            }],
            mem_delete_rules: vec![memstead_base::DeleteRuleSetting {
                pattern: "*".to_string(),
            }],
            ..Default::default()
        },
    );
    let unified_settings = full_settings.clone();
    let mut unified = setup_unified_test_engine_git_branch(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET)
}

#[test]
fn memstead_mem_delete_happy_path_returns_destructive_response() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server_with_delete(&tmp);

    // Seed a mem first so delete has something to remove.
    let target = tmp.path().join("wipe");
    let create_result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "wipe".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert!(create_result.is_error.is_none() || create_result.is_error == Some(false));

    // MCP `memstead_mem_delete`
    // is always destructive. The wrapper hardcodes `delete_files:
    // true`; the engine prunes the per-mem branch + `__MEMSTEAD` config
    // blob in one ref-edit transaction and the response carries
    // `files_deleted: true`.
    let result = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "wipe".to_string(),
        note: Some("mcp handler happy path".to_string()),
    }));
    assert!(
        result.is_error.is_none() || result.is_error == Some(false),
        "happy path must not be an error: {:?}",
        result
    );
    let json: serde_json::Value =
        serde_json::from_str(&extract_text(&result)).expect("response must be JSON");
    assert_eq!(json["name"], "wipe");
    assert_eq!(json["deleted_from_router"], serde_json::Value::Bool(true));
    assert_eq!(json["files_deleted"], serde_json::Value::Bool(true));
    let _ = target;
}

#[test]
fn memstead_mem_delete_path_not_allowed_emits_structured_envelope() {
    let tmp = TempDir::new().unwrap();
    // Create allowlist is permissive, delete allowlist is empty.
    let settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        WorkspaceSettings {
            mem_create_rules: vec![memstead_base::CreateRuleSetting {
                pattern: "*".to_string(),
                schemas: vec!["default@1.0.0".to_string()],
                default_cross_links: None,
            }],
            mem_delete_rules: vec![],
            ..Default::default()
        },
    );
    let unified_settings = settings.clone();
    let _ = settings;
    let mut unified = setup_unified_test_engine(tmp.path());
    unified.set_settings(unified_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // Set up a mem we can try to delete.
    let target = tmp.path().join("pinned");
    let _ = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "pinned".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),

        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));

    let result = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "pinned".to_string(),
        note: None,
    }));
    assert_eq!(result.is_error, Some(true));
    let envelope = result
        .structured_content
        .expect("error must carry structured envelope");
    assert_eq!(envelope["code"], "MEM_PATH_NOT_ALLOWED");
    let details = envelope
        .get("details")
        .expect("envelope must carry details");
    assert_eq!(details["reason"], "no_allowlist_configured");
}

/// The session's declared role and identity ride the one commit a
/// lifecycle tool produces, the create's seed commit and the delete's
/// prune commit on the schema-and-config ref, exactly as they ride
/// every entity mutation's commit. Until 2026-09-11 the two wrappers
/// never set them on the engine, so the trailers were whatever the
/// previous tool call had left there (a grader found the prune commit
/// of a session's first call carrying neither).
#[test]
fn lifecycle_commits_carry_the_sessions_role_and_identity() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server_with_delete(&tmp)
        .with_default_role(memstead_base::vcs::Role::Author)
        .with_default_identity(Some("session-42".to_string()));
    let tip_message = |reference: &str| {
        let repo = gix::open(tmp.path().join("mem-repo").join(".git")).expect("open mem-repo");
        let tip = repo
            .find_reference(reference)
            .expect("ref exists")
            .into_fully_peeled_id()
            .expect("peel");
        let commit = repo
            .find_object(tip.detach())
            .expect("commit")
            .into_commit();
        String::from_utf8_lossy(commit.message_raw().expect("message")).to_string()
    };

    let target = tmp.path().join("noted");
    let created = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "noted".to_string(),
        location: target.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: Some("seeded for the trailer pin".to_string()),
        recovery: None,
        include_schema: false,
    }));
    assert!(created.is_error.is_none() || created.is_error == Some(false));
    // The seed commit lands on the mem's own branch; the config blob
    // on the schema-and-config ref is a separate write.
    let seed = tip_message("refs/heads/noted");
    for needle in [
        "seeded for the trailer pin",
        "Tool: memstead_mem_create",
        "Actor: agent",
        "Role: author",
        "Identity: session-42",
    ] {
        assert!(
            seed.contains(needle),
            "seed commit lacks {needle:?}: {seed:?}"
        );
    }

    let deleted = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "noted".to_string(),
        note: Some("retired after the pin".to_string()),
    }));
    assert!(deleted.is_error.is_none() || deleted.is_error == Some(false));
    let prune = tip_message("refs/heads/__MEMSTEAD");
    assert!(prune.starts_with("memstead: prune __MEMSTEAD:mems/noted/config.json"));
    for needle in [
        "\n\nretired after the pin\n\n",
        "Tool: memstead_mem_delete",
        "Actor: agent",
        "Role: author",
        "Identity: session-42",
    ] {
        assert!(
            prune.contains(needle),
            "prune commit lacks {needle:?}: {prune:?}"
        );
    }
}

#[test]
fn memstead_mem_delete_unknown_mem_emits_unknown_mem_envelope() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server_with_delete(&tmp);

    let result = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "ghost".to_string(),
        note: None,
    }));
    assert_eq!(result.is_error, Some(true));
    let envelope = result
        .structured_content
        .expect("unknown must carry envelope");
    assert_eq!(envelope["code"], "UNKNOWN_MEM");
}

/// `MEM_REFERENCED_BY_POLICY` fires when the workspace-level
/// `[cross_mem_links]` policy grants any other writable mem
/// permission to write into the delete target. Build an engine
/// where `plan-x = ["primary"]` is declared in the effective
/// cross-link map, then attempt to delete `primary` and assert
/// the envelope surfaces the granting mem.
#[test]
fn memstead_mem_delete_policy_grant_envelope_carries_referring_mems() {
    use std::collections::BTreeMap;
    let tmp = TempDir::new().unwrap();

    // Seed both mems via the create-allowlisted server, then
    // re-init the engine with explicit cross-link policy (no MCP
    // path mutates `[cross_mem_links]`; the operator edits
    // `.memstead/workspace.toml` directly).
    let server = setup_lifecycle_server_with_delete(&tmp);
    let _ = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "primary".to_string(),
        location: tmp.path().join("primary").to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    let _ = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "plan-x".to_string(),
        location: tmp.path().join("plan-x").to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    // Drop the seeded server; rebuild the engine with explicit
    // `cross_mem_links` so plan-x → primary is the effective
    // policy. Engine re-init reads the existing mem-repo state.
    drop(server);

    let mut links: BTreeMap<String, memstead_schema::workspace_config::CrossLinkValue> =
        BTreeMap::new();
    links.insert(
        "plan-x".to_string(),
        memstead_schema::workspace_config::CrossLinkValue::List(vec!["primary".to_string()]),
    );
    let settings = WorkspaceSettings {
        mem_create_rules: vec![memstead_base::CreateRuleSetting {
            pattern: "*".to_string(),
            schemas: vec!["default@1.0.0".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: vec![memstead_base::DeleteRuleSetting {
            pattern: "*".to_string(),
        }],
        cross_mem_links: links,
        ..Default::default()
    };
    // Build an engine that mounts the runtime-created
    // git-branch mems with the cross_mem_links policy. Walk
    // `mem-repo/.git/refs/heads/` to enumerate mounts —
    // `setup_unified_test_engine_git_branch` looks for
    // disk-shape mem dirs which don't exist for pure
    // runtime-created git-branch mems.
    let gitdir = tmp.path().join("mem-repo").join(".git");
    let canonical_gitdir = gitdir.canonicalize().unwrap_or(gitdir.clone());
    let mut mounts: Vec<(
        memstead_base::Mount,
        Box<dyn memstead_base::backend::MemBackend>,
    )> = Vec::new();
    for mem_name in ["primary", "plan-x"] {
        let mount = memstead_base::Mount {
            migration_target: None,
            mem: mem_name.to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: memstead_base::MountStorage::GitBranch {
                gitdir: canonical_gitdir.clone(),
                branch: format!("refs/heads/{mem_name}"),
            },
            capability: memstead_base::MountCapability::Write,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
        };
        let backend = memstead_git_branch::storage::instantiate_full_backend(&mount).unwrap();
        mounts.push((mount, backend));
    }
    let mut unified = memstead_base::Engine::from_mounts(mounts).unwrap();
    unified.set_backend_factory(memstead_git_branch::storage::instantiate_full_backend);
    unified.set_settings(settings.clone());
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    // The MCP surface is
    // always destructive (no `delete_files` parameter on the wire).
    // The policy safeguard fires whenever a `[cross_mem_links]`
    // grant points at the target — router-only unregister with
    // storage-preserved is reachable only via the CLI's
    // `memstead mem unregister` verb.
    let result = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "primary".to_string(),
        note: None,
    }));
    assert_eq!(result.is_error, Some(true));
    let envelope = result
        .structured_content
        .expect("reference-block must carry envelope");
    assert_eq!(envelope["code"], "MEM_REFERENCED_BY_POLICY");
    let details = envelope
        .get("details")
        .expect("envelope must carry details");
    assert_eq!(details["name"], "primary");
    assert_eq!(
        details["referring_mems"],
        serde_json::json!(["plan-x"]),
        "details.referring_mems must name the blocking referrer"
    );
}

/// `memstead_mem_delete delete_files=true` against a mem-db-backed
/// (git-branch) mount runs the symmetric cleanup: the per-mem
/// branch + `__MEMSTEAD:mems/<name>/config.json` are pruned, the
/// response carries `files_deleted: true`, and no
/// `MEM_FILES_NOT_DELETED` warning is emitted. Counterpart to
/// the folder-mount test below — together they pin the agent-
/// creatable-equals-agent-deletable contract for both backends.
#[test]
fn memstead_mem_delete_with_delete_files_true_on_mem_db_mount_prunes_branch_and_config() {
    let tmp = TempDir::new().unwrap();
    let server = setup_lifecycle_server_with_delete(&tmp);
    let gitdir = tmp.path().join("mem-repo").join(".git");

    let _ = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "ephemeral".to_string(),
        location: tmp.path().join("ephemeral").to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    // Pre-condition: create wrote the branch + the __MEMSTEAD entry.
    let repo = gix::open(&gitdir).expect("mem-repo gitdir open");
    assert!(
        repo.try_find_reference("refs/heads/ephemeral")
            .unwrap()
            .is_some(),
        "create must seed the per-mem branch"
    );
    let memstead_tree = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .unwrap()
        .expect("create must seed __MEMSTEAD")
        .into_fully_peeled_id()
        .unwrap()
        .object()
        .unwrap()
        .into_commit()
        .tree()
        .unwrap();
    assert!(
        memstead_tree
            .lookup_entry_by_path("mems/ephemeral/config.json")
            .unwrap()
            .is_some(),
        "create must seed __MEMSTEAD:mems/ephemeral/config.json"
    );

    let result = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "ephemeral".to_string(),
        note: None,
    }));
    assert!(
        result.is_error.is_none() || result.is_error == Some(false),
        "delete must succeed: {result:?}"
    );
    let sc = result
        .structured_content
        .as_ref()
        .expect("response must carry structured_content");
    assert_eq!(
        sc["files_deleted"],
        serde_json::Value::Bool(true),
        "mem-db delete_files=true now prunes the branch + __MEMSTEAD config — files_deleted must be true"
    );
    if let Some(warnings) = sc.get("warnings").and_then(|v| v.as_array()) {
        assert!(
            warnings
                .iter()
                .all(|w| w["code"] != "MEM_FILES_NOT_DELETED"),
            "no MEM_FILES_NOT_DELETED warning when cleanup succeeded: {warnings:?}"
        );
    }
    // Post-condition: branch gone, __MEMSTEAD entry gone.
    let repo = gix::open(&gitdir).expect("mem-repo gitdir reopen");
    assert!(
        repo.try_find_reference("refs/heads/ephemeral")
            .unwrap()
            .is_none(),
        "delete must drop refs/heads/ephemeral"
    );
    let memstead_tree = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .unwrap()
        .expect("__MEMSTEAD survives the per-mem prune")
        .into_fully_peeled_id()
        .unwrap()
        .object()
        .unwrap()
        .into_commit()
        .tree()
        .unwrap();
    assert!(
        memstead_tree
            .lookup_entry_by_path("mems/ephemeral/config.json")
            .unwrap()
            .is_none(),
        "delete must prune __MEMSTEAD:mems/ephemeral/config.json"
    );
}

/// Folder-backend happy path: `delete_files=true` on a mem whose
/// directory exists removes the directory and reports
/// `files_deleted: true` with no `MEM_FILES_NOT_DELETED` warning.
/// Companion to the mem-db-backed test above — together they
/// pin the two halves of the documented contract.
#[test]
fn memstead_mem_delete_with_delete_files_true_on_folder_mount_removes_dir() {
    let tmp = TempDir::new().unwrap();
    // Build an engine without the git-branch factory: folder backend only, no
    // mem-repo seeded. The create orchestrator's heuristic then
    // picks `MountStorage::Folder { path }` so `dir_for_mem`
    // returns Some on the registered mem.
    let mut unified = memstead_base::Engine::from_mounts(Vec::new()).unwrap();
    unified.set_settings(WorkspaceSettings {
        mem_create_rules: vec![memstead_base::CreateRuleSetting {
            pattern: "*".to_string(),
            schemas: vec!["default@1.0.0".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: vec![memstead_base::DeleteRuleSetting {
            pattern: "*".to_string(),
        }],
        ..Default::default()
    });
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

    let mem_dir = tmp.path().join("scratch");
    let _ = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "scratch".to_string(),
        location: mem_dir.to_string_lossy().into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert!(mem_dir.is_dir(), "create must produce the mem dir on disk");

    let result = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "scratch".to_string(),
        note: None,
    }));
    assert!(
        result.is_error.is_none() || result.is_error == Some(false),
        "delete must succeed: {result:?}"
    );
    let sc = result
        .structured_content
        .as_ref()
        .expect("response must carry structured_content");
    assert_eq!(
        sc["files_deleted"],
        serde_json::Value::Bool(true),
        "rmdir on a removable directory must report files_deleted: true"
    );
    // The directory must be physically gone.
    assert!(!mem_dir.exists(), "the on-disk directory must be removed");
    // No MEM_FILES_NOT_DELETED warning — the cleanup ran cleanly.
    if let Some(warnings) = sc.get("warnings").and_then(|v| v.as_array()) {
        assert!(
            warnings
                .iter()
                .all(|w| w["code"] != "MEM_FILES_NOT_DELETED"),
            "no MEM_FILES_NOT_DELETED warning when rmdir succeeded: {warnings:?}"
        );
    }
}

/// Hierarchical mem-db round-trip:
/// `memstead_mem_create name=plan-q4 path=planning schema=…` followed by
/// `memstead_mem_delete name=plan-q4 delete_files=true` leaves zero
/// trace of the hierarchical branch + `__MEMSTEAD` config. The branch
/// ref under `refs/heads/planning/plan-q4` and the tree path
/// `mems/planning/plan-q4/config.json` must both be gone after
/// the delete. Pairs with the flat case (`exec-*` / `ephemeral`)
/// above — together they pin the path-composition contract on
/// both halves of the lifecycle.
#[test]
fn memstead_mem_delete_hierarchical_path_prunes_branch_and_config() {
    let tmp = TempDir::new().unwrap();
    let full_settings = memstead_git_branch::test_support::auto_seed_with_settings(
        tmp.path(),
        WorkspaceSettings {
            mem_create_rules: vec![memstead_base::CreateRuleSetting {
                pattern: "planning/plan-*".to_string(),
                schemas: vec!["default@1.0.0".to_string()],
                default_cross_links: None,
            }],
            mem_delete_rules: vec![memstead_base::DeleteRuleSetting {
                pattern: "planning/plan-*".to_string(),
            }],
            ..Default::default()
        },
    );
    let mut unified = setup_unified_test_engine_git_branch(tmp.path());
    unified.set_settings(full_settings);
    let canonical_root =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    unified.set_workspace_root(canonical_root);
    let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);
    let gitdir = tmp.path().join("mem-repo").join(".git");

    // Create the hierarchical mem. The full
    // `planning/plan-q4` name is the canonical input — no
    // separate `path` field.
    let create_result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
        title: None,
        description: None,
        subject: None,
        schema_verbosity: None,
        write_guidance: Default::default(),
        name: "planning/plan-q4".to_string(),
        location: tmp
            .path()
            .join("planning")
            .join("plan-q4")
            .to_string_lossy()
            .into_owned(),
        schema: "default@1.0.0".to_string(),
        vcs: None,
        note: None,
        recovery: None,
        include_schema: false,
    }));
    assert!(
        create_result.is_error.is_none() || create_result.is_error == Some(false),
        "hierarchical create must succeed: {create_result:?}"
    );

    // Pre-condition: hierarchical branch + config exist.
    let repo = gix::open(&gitdir).expect("mem-repo open");
    assert!(
        repo.try_find_reference("refs/heads/planning/plan-q4")
            .unwrap()
            .is_some(),
        "create must seed refs/heads/planning/plan-q4"
    );
    let memstead_tree = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .unwrap()
        .expect("__MEMSTEAD exists after create")
        .into_fully_peeled_id()
        .unwrap()
        .object()
        .unwrap()
        .into_commit()
        .tree()
        .unwrap();
    assert!(
        memstead_tree
            .lookup_entry_by_path("mems/planning/plan-q4/config.json")
            .unwrap()
            .is_some(),
        "create must seed __MEMSTEAD:mems/planning/plan-q4/config.json"
    );

    // Symmetric cleanup — MCP delete is always destructive, and
    // the mem name IS the full hierarchical path.
    let delete_result = server.memstead_mem_delete(Parameters(TlsMemDeleteParams {
        name: "planning/plan-q4".to_string(),
        note: None,
    }));
    assert!(
        delete_result.is_error.is_none() || delete_result.is_error == Some(false),
        "hierarchical delete must succeed: {delete_result:?}"
    );
    let sc = delete_result
        .structured_content
        .as_ref()
        .expect("response carries structured_content");
    assert_eq!(
        sc["files_deleted"],
        serde_json::Value::Bool(true),
        "hierarchical mem-db delete_files=true must report files_deleted: true"
    );

    // Post-condition: both branch + config gone.
    let repo = gix::open(&gitdir).expect("mem-repo reopen");
    assert!(
        repo.try_find_reference("refs/heads/planning/plan-q4")
            .unwrap()
            .is_none(),
        "delete must drop refs/heads/planning/plan-q4"
    );
    let memstead_tree = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .unwrap()
        .expect("__MEMSTEAD survives")
        .into_fully_peeled_id()
        .unwrap()
        .object()
        .unwrap()
        .into_commit()
        .tree()
        .unwrap();
    assert!(
        memstead_tree
            .lookup_entry_by_path("mems/planning/plan-q4/config.json")
            .unwrap()
            .is_none(),
        "delete must prune __MEMSTEAD:mems/planning/plan-q4/config.json"
    );
    // The empty `planning/` ancestor directory is gix-pruned on the
    // tree write — assert it's gone too.
    assert!(
        memstead_tree
            .lookup_entry_by_path("mems/planning")
            .unwrap()
            .is_none(),
        "delete must collapse the empty `mems/planning/` ancestor"
    );
}

// ======================================================================
// MCP tool filter — [mcp].disabled_tools end-to-end
// ======================================================================

mod disabled_tools {
    use super::super::*;
    use super::{setup_dual_test_engine, setup_unified_test_engine};
    use std::collections::HashSet;

    /// Build an `McpServer` with the same engine as `setup_test_engine`
    /// plus an explicit disabled-tool set. Keeps the mem fixture
    /// identical so baseline assertions carry over.
    fn setup_filtered_server(disabled: &[&str]) -> (McpServer, tempfile::TempDir) {
        // Reuse the existing engine factory, then rewrap with the
        // filter. `setup_test_engine` returns an `McpServer` — we
        // throw it away and construct a fresh one sharing the same
        // tmp fixture so the filter lands on the same graph state.
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead/config.json"),
            r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
        )
        .unwrap();
        let _ = mem_dir;
        memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
        let set: HashSet<String> = disabled.iter().map(|s| s.to_string()).collect();
        let server = McpServer::new_with_filter(
            setup_unified_test_engine(tmp.path()),
            crate::config::DEFAULT_TOKEN_BUDGET,
            set,
            Some(PathBuf::from("/tmp/test.memstead.toml")),
        );
        (server, tmp)
    }

    #[test]
    fn empty_filter_lists_every_compiled_tool() {
        // Invariant: absent or empty filter is byte-identical to the
        // pre-filter surface.
        let (server, _tmp) = setup_dual_test_engine();
        let filtered: Vec<String> = server
            .filtered_tool_list()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        let router_all: Vec<String> = McpServer::tool_router()
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(filtered, router_all);
    }

    #[test]
    fn filter_omits_disabled_names_from_list() {
        let (server, _tmp) = setup_filtered_server(&["memstead_mem_create", "memstead_mem_delete"]);
        let names: Vec<String> = server
            .filtered_tool_list()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(!names.iter().any(|n| n == "memstead_mem_create"));
        assert!(!names.iter().any(|n| n == "memstead_mem_delete"));
        // Every other tool still present — verify by cardinality +
        // presence of a well-known read tool.
        assert!(names.iter().any(|n| n == "memstead_entity"));
        assert!(names.iter().any(|n| n == "memstead_overview"));
        let router_all: usize = McpServer::tool_router().list_all().len();
        assert_eq!(names.len(), router_all - 2);
    }

    #[test]
    fn get_tool_returns_none_for_disabled_name() {
        use rmcp::ServerHandler;
        let (server, _tmp) = setup_filtered_server(&["memstead_mem_create"]);
        assert!(server.get_tool("memstead_mem_create").is_none());
        // Non-disabled name still resolves.
        assert!(server.get_tool("memstead_entity").is_some());
    }

    #[test]
    fn tool_disabled_envelope_carries_code_and_details() {
        let (server, _tmp) = setup_filtered_server(&["memstead_mem_create"]);
        let result = server.tool_disabled_response("memstead_mem_create");
        assert_eq!(result.is_error, Some(true));
        let env = result
            .structured_content
            .expect("tool_disabled must carry structured_content");
        assert_eq!(env["code"], "TOOL_DISABLED");
        let details = env.get("details").expect("details present");
        assert_eq!(details["tool"], "memstead_mem_create");
        assert_eq!(
            details["config_source"], "/tmp/test.memstead.toml",
            "config_source must echo the resolved path"
        );
    }

    #[test]
    fn tool_disabled_envelope_omits_config_source_when_none() {
        // Construct without a config_source (tests / no file-backed
        // config). The envelope drops the field entirely so agents
        // don't decode a stub path.
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead/config.json"),
            r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
        )
        .unwrap();
        let _ = mem_dir;
        memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
        let mut set = HashSet::new();
        set.insert("memstead_entity".to_string());
        let server = McpServer::new_with_filter(
            memstead_base::Engine::from_mounts(vec![]).unwrap(),
            crate::config::DEFAULT_TOKEN_BUDGET,
            set,
            None,
        );
        let result = server.tool_disabled_response("memstead_entity");
        let env = result.structured_content.unwrap();
        let details = env.get("details").unwrap();
        assert_eq!(details["tool"], "memstead_entity");
        assert!(
            details.get("config_source").is_none(),
            "config_source must be absent when no file-backed config sourced the filter"
        );
    }

    #[test]
    fn chat_agent_scenario_hides_mem_lifecycle_pair() {
        // Mirrors an embedder chat-agent config (the retired macOS
        // app's pattern): the host owns mem lifecycle, so the
        // embedded chat agent never sees `memstead_mem_create` /
        // `memstead_mem_delete` on its MCP surface.
        use rmcp::ServerHandler;
        let (server, _tmp) = setup_filtered_server(&["memstead_mem_create", "memstead_mem_delete"]);
        // List omits both.
        let names: Vec<String> = server
            .filtered_tool_list()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(!names.iter().any(|n| n == "memstead_mem_create"));
        assert!(!names.iter().any(|n| n == "memstead_mem_delete"));
        // get_tool rejects.
        assert!(server.get_tool("memstead_mem_create").is_none());
        assert!(server.get_tool("memstead_mem_delete").is_none());
        // Direct response envelope for a filtered call.
        let resp = server.tool_disabled_response("memstead_mem_create");
        assert_eq!(resp.is_error, Some(true));
        // Non-filtered tools still reachable.
        assert!(server.get_tool("memstead_entity").is_some());
        assert!(server.get_tool("memstead_search").is_some());
        assert!(server.is_tool_disabled("memstead_mem_create"));
        assert!(!server.is_tool_disabled("memstead_entity"));
    }

    #[test]
    fn is_tool_disabled_matches_exactly_no_glob() {
        // Plan invariant: exact string equality, no glob / prefix
        // semantics. A partial match must not trigger the filter.
        let (server, _tmp) = setup_filtered_server(&["memstead_mem_create"]);
        assert!(server.is_tool_disabled("memstead_mem_create"));
        assert!(!server.is_tool_disabled("memstead_mem_create_extra"));
        assert!(!server.is_tool_disabled("memstead_mem"));
        assert!(!server.is_tool_disabled("memstead_"));
    }
}

// --------------------------------------------------------------
// `note` field on mutation tools + `[mutations].require_notes`
// WarningHint pipeline — commit-body layout and require-notes
// policy are the load-bearing assertions.
// --------------------------------------------------------------

mod mutation_note {
    use super::*;
    use crate::tools::mutation::{CreateParams, UpdateParams};

    /// Open the per-mem gitdir and return the `HEAD` commit's
    /// raw message. Shells out to `git log -1 --format=%B` so we
    /// don't pull `gix` into the `memstead-mcp` dev-dependency surface
    /// just for one test — the production crate already depends
    /// on `gix` transitively via `memstead-git-branch`, but MCP-level tests
    /// shouldn't take that direct dep. Uses the engine's own
    /// `gitdir_for` resolver so the test tracks whatever layout
    /// the default resolver produced (isolated `.git/` under the
    /// mem root today).
    fn head_commit_message(server: &McpServer, mem: &str) -> String {
        let gitdir = {
            let unified = server.unified_engine().clone();
            let engine = unified.lock().unwrap();
            engine
                .gitdir_for(mem)
                .expect("gitdir resolves for fixture mem")
        };
        // Mem-repo-backed mems commit to `refs/heads/<mem>`;
        // the shared `mem-repo/.git/`'s HEAD points at `main`
        // (workspace configs branch), so a plain `git log -1` would
        // read the seed commit instead of the entity write. Try the
        // per-mem branch first; fall back to HEAD for legacy
        // disk-backed mems whose only branch is HEAD.
        let per_mem_ref = format!("refs/heads/{mem}");
        let try_log = |rev: &str| {
            std::process::Command::new("git")
                .arg("--git-dir")
                .arg(&gitdir)
                .arg("log")
                .arg("-1")
                .arg("--format=%B")
                .arg(rev)
                .output()
                .expect("git log invocation succeeds")
        };
        let output = {
            let first = try_log(&per_mem_ref);
            if first.status.success() {
                first
            } else {
                try_log("HEAD")
            }
        };
        assert!(
            output.status.success(),
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("commit message is utf-8")
    }

    /// Build a server whose workspace config matches the one
    /// `[mutations].require_notes = require` produces after
    /// `config::resolve`. Fixture engine is shared with the
    /// module-level helper; only the constructor changes.
    fn server_with_require_notes(require: bool) -> (McpServer, TempDir) {
        let (_default_server, tmp) = setup_dual_test_engine();
        // Re-read the fixture mem under a fresh Engine because
        // `setup_test_engine` already consumed it; the simpler
        // path is to rebuild the engine from the same on-disk
        // state, which is canonical because `Engine::init` is
        // deterministic over the fixture directory.
        let _mem_dir = tmp.path().join("specs");
        memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
        let mutations = crate::config::MutationsSection {
            require_notes: Some(require),
        };
        // Build a git-branch engine so `memstead_create` /
        // `memstead_update` write to mem-repo and
        // `head_commit_message` reads from the same gitdir.
        let mut unified = setup_unified_test_engine_git_branch(tmp.path());
        let canonical_root =
            std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
        unified.set_workspace_root(canonical_root);
        let server = McpServer::new_with_config(
            unified,
            crate::config::DEFAULT_TOKEN_BUDGET,
            HashSet::new(),
            None,
            mutations,
            HashMap::new(),
        );
        (server, tmp)
    }

    /// Happy path: `memstead_create` with a `note` field threads the
    /// sentence into the commit body between the subject and the
    /// provenance trailers. The response also carries no
    /// `NOTE_MISSING` warning because the caller supplied one.
    #[test]
    fn memstead_create_with_note_lands_in_commit_body() {
        let tmp = setup_test_workspace();
        let mut unified = setup_unified_test_engine_git_branch(tmp.path());
        let canonical_root =
            std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
        unified.set_workspace_root(canonical_root);
        let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);
        let mut sections = IndexMap::new();
        sections.insert("identity".into(), "c".into());
        sections.insert("purpose".into(), "d".into());
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Foo Invariant".into(),
            entity_type: "spec".into(),
            mem: None,
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: Some(false),
            note: Some("documenting the foo invariant".into()),
            role: None,
            identity: None,
        }));
        assert!(
            result.is_error.is_none() || result.is_error == Some(false),
            "memstead_create must succeed: {}",
            extract_text(&result)
        );
        // Confirm the commit body carries the note between the
        // subject and the trailer block.
        let msg = head_commit_message(&server, "specs");
        assert!(
            msg.contains("\n\ndocumenting the foo invariant\n\n"),
            "commit message must contain the note between subject \
                 and trailers; got: {msg:?}"
        );
        // And the trailer block still carries the `Actor:` line
        // after the note paragraph.
        assert!(
            msg.contains("Actor: agent"),
            "provenance trailer block must survive the note insert; \
                 got: {msg:?}"
        );
        // No warning added when the note is supplied (default
        // posture: require_notes = false / None).
        let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
        let empty_vec: Vec<serde_json::Value> = vec![];
        let warnings = json["warnings"].as_array().unwrap_or(&empty_vec);
        assert!(
            !warnings
                .iter()
                .any(|w| w["code"].as_str() == Some("NOTE_MISSING")),
            "NOTE_MISSING must not appear when note is supplied: {warnings:?}"
        );
    }

    /// `[mutations].require_notes = true` + missing note: the
    /// mutation still commits, and the response carries a
    /// `NOTE_MISSING` WarningHint envelope so autonomous skills
    /// can audit their coverage. Verifies both the wire shape
    /// (warnings array contains the envelope) and the commit-body
    /// shape (no extra body paragraph — a missing note collapses
    /// to the no-note layout).
    #[test]
    fn require_notes_without_note_adds_warning_but_commit_still_lands() {
        let (server, _tmp) = server_with_require_notes(true);
        let result = server.memstead_update(Parameters(UpdateParams {
            anchors: None,
            relations_unset: None,
            anchors_unset: None,
            sections_unset: None,
            id: "specs--entity-a".into(),
            expected_hash: Some({
                // Read the current hash so we don't have to hard-code it.
                let entity = server.memstead_entity(Parameters(EntityParams {
                    id: "specs--entity-a".into(),
                    include_relations: None,
                    include_context: None,
                    sections: None,
                    token_budget: None,
                    chunk: None,
                    include_provenance: None,
                }));
                let text = extract_text(&entity);
                // Markdown rendering includes the frontmatter
                // line `_hash: <hex>`; extract it.
                text.lines()
                    .find(|l| l.starts_with("_hash: "))
                    .unwrap()
                    .trim_start_matches("_hash: ")
                    .to_string()
            }),
            sections: None,
            append_sections: Some({
                let mut m = IndexMap::new();
                m.insert("purpose".into(), "extra".into());
                m
            }),
            patch_sections: None,
            metadata: None,
            metadata_unset: None,
            dry_run: Some(false),
            note: None,
            role: None,
            identity: None,
            declare_relations: None,
        }));
        assert!(
            result.is_error.is_none() || result.is_error == Some(false),
            "memstead_update must succeed under require_notes: {}",
            extract_text(&result)
        );
        // Warning on the wire.
        let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
        let warnings = json["warnings"].as_array().expect("warnings array");
        // The engine is the single enforcement point; the warning's
        // `tool` is the engine-level verb (`update_entity`), matching
        // the commit `Tool:` provenance trailer — not the MCP tool
        // name. The CLI surface inherits the same value.
        let has_note_missing = warnings.iter().any(|w| {
            w["code"].as_str() == Some("NOTE_MISSING")
                && w["details"]["tool"].as_str() == Some("update_entity")
        });
        assert!(
            has_note_missing,
            "require_notes + missing note must append NOTE_MISSING \
                 warning; got: {warnings:?}"
        );
        // Commit still lands — body carries no extra paragraph
        // since the note was absent.
        let msg = head_commit_message(&server, "specs");
        assert!(
            msg.starts_with("memstead: update specs--entity-a"),
            "commit subject preserved: {msg:?}"
        );
        assert!(
            msg.contains("Actor: agent"),
            "provenance trailer survives the warning path: {msg:?}"
        );
    }

    /// `require_notes = true` + blank / whitespace-only note: the
    /// note collapses to "absent" semantics, so the pipeline
    /// still emits `NOTE_MISSING`. Guards against an agent who
    /// satisfies the type signature without actually documenting
    /// the change.
    #[test]
    fn require_notes_with_blank_note_still_warns() {
        let (server, _tmp) = server_with_require_notes(true);
        let mut sections = IndexMap::new();
        sections.insert("identity".into(), "c".into());
        sections.insert("purpose".into(), "d".into());
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Blank Note Path".into(),
            entity_type: "spec".into(),
            mem: None,
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: Some(false),
            note: Some("   \t ".into()),
            role: None,
            identity: None,
        }));
        let json: serde_json::Value = serde_json::from_str(&extract_text(&result)).unwrap();
        let warnings = json["warnings"].as_array().expect("warnings array");
        assert!(
            warnings
                .iter()
                .any(|w| w["code"].as_str() == Some("NOTE_MISSING")),
            "blank note must still trigger NOTE_MISSING: {warnings:?}"
        );
    }

    /// An over-cap note (> `NOTE_MAX_LEN` chars) is a hard
    /// `INVALID_INPUT` rejection before the engine is touched.
    /// No commit is produced; the agent sees the typed envelope.
    #[test]
    fn oversized_note_returns_invalid_input() {
        let (server, _tmp) = setup_dual_test_engine();
        let mut sections = IndexMap::new();
        sections.insert("identity".into(), "c".into());
        sections.insert("purpose".into(), "d".into());
        let oversized: String = "x".repeat(memstead_base::mem_management::NOTE_MAX_LEN + 1);
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Oversized Note".into(),
            entity_type: "spec".into(),
            mem: None,
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: Some(false),
            note: Some(oversized),
            role: None,
            identity: None,
        }));
        assert_eq!(result.is_error, Some(true));
        let payload = result
            .structured_content
            .as_ref()
            .expect("structured envelope present");
        assert_eq!(
            payload["code"].as_str(),
            Some("INVALID_INPUT"),
            "expected INVALID_INPUT envelope, got: {payload:?}"
        );
    }
}

/// Every single-mem response carries
/// `_mem_schema: <name>@<version>` so agents read
/// the canonical schema pin in the same response they're already
/// looking at. Multi-mem responses and pre-resolve errors carry no
/// anchor (no single mem to point at).
mod schema_anchor {
    use super::*;

    /// Pull the structured-content JSON object from a tool response.
    fn payload(result: &CallToolResult) -> serde_json::Value {
        result
            .structured_content
            .as_ref()
            .cloned()
            .expect("structured_content present")
    }

    /// JSON-shaped responses anchor the canonical schema-ref at the
    /// top level so agents reading a response do not need a follow-up
    /// `memstead_overview` to know which schema is in play.
    #[test]
    fn memstead_create_response_carries_anchor() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Anchor Probe".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(IndexMap::from_iter([
                ("identity".to_string(), "Probe.".to_string()),
                ("purpose".to_string(), "Probe.".to_string()),
            ])),
            metadata: None,
            relations: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        assert_eq!(
            payload(&result)["_mem_schema"].as_str(),
            Some("default@1.0.0"),
        );
    }

    /// `memstead_update` derives the mem from the entity ID — the anchor
    /// must follow the same mem even though the params have no
    /// explicit `mem` field.
    #[test]
    fn memstead_update_response_carries_anchor() {
        let (server, _tmp) = setup_dual_test_engine();
        // Read first to grab the hash.
        let read = server.memstead_entity(Parameters(EntityParams {
            id: "specs--entity-a".to_string(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        let read_text = extract_text(&read);
        let hash_line = read_text
            .lines()
            .find(|l| l.starts_with("_hash:"))
            .expect("_hash line present");
        let expected_hash = hash_line.trim_start_matches("_hash:").trim();

        // Empty payloads refuse with `EMPTY_UPDATE`. Pass same-content
        // section to keep this dry_run preview on the success
        // path that emits the anchor.
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "First test entity.".to_string());
        let result = server.memstead_update(Parameters(UpdateParams {
            anchors: None,
            relations_unset: None,
            anchors_unset: None,
            id: "specs--entity-a".to_string(),
            expected_hash: Some(expected_hash.to_string()),
            sections: Some(sections),
            append_sections: None,
            patch_sections: None,
            sections_unset: None,
            metadata: None,
            metadata_unset: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
            declare_relations: None,
        }));
        assert!(
            !result.is_error.unwrap_or(false),
            "{}",
            extract_text(&result)
        );
        assert_eq!(
            payload(&result)["_mem_schema"].as_str(),
            Some("default@1.0.0"),
        );
    }

    /// `memstead_update`'s new `declare_relations` parameter
    /// surfaces on the wire: the JSON body carries a
    /// `relations_declared: [...]` array per the additive
    /// response-shape contract. Verified end-to-end against the
    /// MCP server. The atomicity with the strict
    /// wiki-link/relation validator is covered at the engine
    /// layer (memstead-base) — this test pins the MCP plumbing.
    #[test]
    fn memstead_update_relations_declared_surfaces_on_response() {
        let (server, _tmp) = setup_dual_test_engine();
        // Read entity-a to grab the hash.
        let read = server.memstead_entity(Parameters(EntityParams {
            id: "specs--entity-a".to_string(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        let read_text = extract_text(&read);
        let hash_line = read_text
            .lines()
            .find(|l| l.starts_with("_hash:"))
            .expect("_hash line present");
        let expected_hash = hash_line.trim_start_matches("_hash:").trim();

        let result = server.memstead_update(Parameters(UpdateParams {
            anchors: None,
            relations_unset: None,
            anchors_unset: None,
            id: "specs--entity-a".to_string(),
            expected_hash: Some(expected_hash.to_string()),
            sections: None,
            append_sections: None,
            patch_sections: None,
            sections_unset: None,
            metadata: None,
            metadata_unset: None,
            dry_run: None,
            note: None,
            role: None,
            identity: None,
            declare_relations: Some(vec![crate::tools::mutation::RelationInput {
                target: "specs--entity-b".to_string(),
                rel_type: "USES".to_string(),
                description: None,
            }]),
        }));
        assert!(
            !result.is_error.unwrap_or(false),
            "{}",
            extract_text(&result)
        );
        let body = payload(&result);
        let declared = body["relations_declared"]
            .as_array()
            .expect("relations_declared must be an array on the response");
        assert_eq!(declared.len(), 1, "expected one declared relation: {body}");
        assert_eq!(declared[0]["rel_type"].as_str(), Some("USES"));
        assert_eq!(declared[0]["target"].as_str(), Some("specs--entity-b"),);
        assert_eq!(
            declared[0]["target_was_stubbed"].as_bool(),
            Some(false),
            "target already existed; target_was_stubbed must be false"
        );
    }

    /// `memstead_relate` resolves mem from the source entity. Even on a
    /// duplicate-relationship warning path the anchor must ship.
    #[test]
    fn memstead_relate_response_carries_anchor() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_relate(Parameters(RelateParams {
            relations: vec![RelateOpInput {
                from: "specs--entity-a".to_string(),
                to: "specs--entity-b".to_string(),
                rel_type: "USES".to_string(),
                remove: None,
                description: None,
            }],
            note: None,
            role: None,
            identity: None,
            dry_run: None,
        }));
        assert!(
            !result.is_error.unwrap_or(false),
            "{}",
            extract_text(&result)
        );
        assert_eq!(
            payload(&result)["_mem_schema"].as_str(),
            Some("default@1.0.0"),
        );
    }

    /// Rehearsal marker form, single-op path:
    /// `dry_run: true` reports the would-be edge and would-be stub
    /// with an EMPTY `write_id`, creates nothing, and the
    /// follow-up real call succeeds with a non-empty one.
    #[test]
    fn memstead_relate_dry_run_single_carries_marker_and_creates_nothing() {
        let (server, _tmp) = setup_dual_test_engine();
        let call = |dry: Option<bool>| {
            server.memstead_relate(Parameters(RelateParams {
                relations: vec![RelateOpInput {
                    from: "specs--entity-a".to_string(),
                    to: "specs--rehearsed-ghost".to_string(),
                    rel_type: "USES".to_string(),
                    remove: None,
                    description: None,
                }],
                note: None,
                role: None,
                identity: None,
                dry_run: dry,
            }))
        };
        let rehearsed = call(Some(true));
        assert!(
            !rehearsed.is_error.unwrap_or(false),
            "{}",
            extract_text(&rehearsed)
        );
        let body = payload(&rehearsed);
        assert_eq!(body["write_id"], "", "marker form: empty write_id");
        assert_eq!(body["results"][0]["action"], "added");
        let warnings = serde_json::to_string(&body["warnings"]).unwrap();
        assert!(
            warnings.contains("AUTO_STUB_CREATED"),
            "would-be stub must be reported: {warnings}"
        );
        // The stub was never created.
        let read = server.memstead_entity(Parameters(EntityParams {
            id: "specs--rehearsed-ghost".to_string(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        assert!(
            read.is_error.unwrap_or(false),
            "rehearsed stub must not exist: {}",
            extract_text(&read)
        );
        // The real call on the unchanged mem lands.
        let real = call(None);
        assert!(!real.is_error.unwrap_or(false), "{}", extract_text(&real));
        let real_body = payload(&real);
        assert_ne!(real_body["write_id"], "", "the real relate commits");
        // No cross-call hash-equality assertion: the auto-stamped
        // `last_modified` (second-resolution wall clock) enters
        // `_hash`, so rehearsed vs real legitimately diverge when
        // a second ticks between the calls. The clock-pinned
        // engine test asserts equality deterministically.
    }

    /// Rehearsal marker form, batch path: a multi-op `dry_run`
    /// list reports every would-be action with an empty
    /// `write_id`, reports (never creates) would-be stubs, and
    /// the follow-up real list succeeds.
    #[test]
    fn memstead_relate_dry_run_batch_carries_marker_and_creates_nothing() {
        let (server, _tmp) = setup_dual_test_engine();
        let ops = || {
            vec![
                // Reverse direction of the fixture's existing a→b
                // edge so this op is a genuine would-be add.
                RelateOpInput {
                    from: "specs--entity-b".to_string(),
                    to: "specs--entity-a".to_string(),
                    rel_type: "USES".to_string(),
                    remove: None,
                    description: None,
                },
                RelateOpInput {
                    from: "specs--entity-b".to_string(),
                    to: "specs--batch-ghost".to_string(),
                    rel_type: "USES".to_string(),
                    remove: None,
                    description: None,
                },
            ]
        };
        let rehearsed = server.memstead_relate(Parameters(RelateParams {
            relations: ops(),
            note: None,
            role: None,
            identity: None,
            dry_run: Some(true),
        }));
        assert!(
            !rehearsed.is_error.unwrap_or(false),
            "{}",
            extract_text(&rehearsed)
        );
        let body = payload(&rehearsed);
        assert_eq!(body["write_id"], "", "marker form: empty write_id");
        assert_eq!(body["results"][0]["action"], "added");
        assert_eq!(body["results"][1]["action"], "added");
        let warnings = serde_json::to_string(&body["warnings"]).unwrap();
        assert!(
            warnings.contains("AUTO_STUB_CREATED") && warnings.contains("specs--batch-ghost"),
            "would-be stub must be reported: {warnings}"
        );
        let read = server.memstead_entity(Parameters(EntityParams {
            id: "specs--batch-ghost".to_string(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        assert!(
            read.is_error.unwrap_or(false),
            "rehearsed stub must not exist"
        );
        // The real list on the unchanged mems lands.
        let real = server.memstead_relate(Parameters(RelateParams {
            relations: ops(),
            note: None,
            role: None,
            identity: None,
            dry_run: None,
        }));
        assert!(!real.is_error.unwrap_or(false), "{}", extract_text(&real));
        assert_ne!(payload(&real)["write_id"], "", "the real batch commits");
    }

    /// Markdown frontmatter on `memstead_entity` carries the anchor as the
    /// first line inside the `---` block. Position matters less than
    /// presence — agents parsing YAML frontmatter find it either way.
    #[test]
    fn memstead_entity_frontmatter_carries_anchor_for_real_entity() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_entity(Parameters(EntityParams {
            id: "specs--entity-a".to_string(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        let text = extract_text(&result);
        assert!(
            text.contains("_mem_schema: default@1.0.0"),
            "anchor missing from entity frontmatter:\n{text}"
        );
    }

    /// Stub reads carry the anchor too — the entity is a placeholder
    /// but the mem is real and known. Pinned so a future regression
    /// stripping anchors from "incomplete" entities is caught.
    #[test]
    fn memstead_entity_frontmatter_carries_anchor_for_stub() {
        let (server, _tmp) = setup_dual_test_engine();
        // Create a stub by relating to a non-existent target.
        let _ = server.memstead_relate(Parameters(RelateParams {
            relations: vec![RelateOpInput {
                from: "specs--entity-a".to_string(),
                to: "specs--stub-target".to_string(),
                rel_type: "USES".to_string(),
                remove: None,
                description: None,
            }],
            note: None,
            role: None,
            identity: None,
            dry_run: None,
        }));

        let result = server.memstead_entity(Parameters(EntityParams {
            id: "specs--stub-target".to_string(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        let text = extract_text(&result);
        assert!(
            text.contains("_mem_schema: default@1.0.0"),
            "stub frontmatter must still carry the anchor:\n{text}"
        );
    }

    /// `memstead_overview` scoped to a single mem carries the anchor in
    /// its frontmatter.
    #[test]
    fn memstead_overview_single_mem_carries_anchor() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_overview(Parameters(OverviewParams {
            mem: Some("specs".to_string()),
            rebuild: Some(true),
            chunk: None,
            token_budget: None,
            include: None,
        }));
        let text = extract_text(&result);
        assert!(
            text.contains("_mem_schema: default@1.0.0"),
            "single-mem overview must carry the anchor:\n{text}"
        );
    }

    /// Multi-mem `memstead_overview` (no `mem` filter) skips the
    /// anchor — there's no single mem to point at.
    #[test]
    fn memstead_overview_multi_mem_omits_anchor() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_overview(Parameters(OverviewParams {
            mem: None,
            rebuild: Some(true),
            chunk: None,
            token_budget: None,
            include: None,
        }));
        let text = extract_text(&result);
        assert!(
            !text.contains("_mem_schema:"),
            "multi-mem overview must NOT carry an anchor:\n{text}"
        );
    }

    /// `memstead_health` inherits the same single-vs-multi rule: scoped
    /// to one mem → anchor; global → no anchor.
    #[test]
    fn memstead_health_single_mem_carries_anchor() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_health(Parameters(HealthParams {
            include: None,
            limit: None,
            mem: Some("specs".to_string()),
            include_config: false,
            token_budget: None,
            chunk: None,
            target_schema: None,
        }));
        assert_eq!(
            payload(&result)["_mem_schema"].as_str(),
            Some("default@1.0.0"),
        );
    }

    #[test]
    fn memstead_health_multi_mem_omits_anchor() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_health(Parameters(HealthParams {
            include: None,
            limit: None,
            mem: None,
            include_config: false,
            token_budget: None,
            chunk: None,
            target_schema: None,
        }));
        assert!(
            payload(&result).get("_mem_schema").is_none(),
            "global health must not carry an anchor: {:?}",
            payload(&result)
        );
    }

    /// Pre-resolve errors carry no anchor — the engine never resolved
    /// a mem to point at. Pinning this so a future "always inject
    /// even on errors" change does not silently anchor to the wrong
    /// mem.
    #[test]
    fn pre_resolve_unknown_mem_carries_no_anchor() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_entity(Parameters(EntityParams {
            id: "ghost-mem--missing-entity".to_string(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        // The lookup fails (NotFound). The response shape varies
        // (sometimes a markdown body, sometimes an error envelope on
        // structured_content) but never carries an anchor — both
        // surfaces are checked.
        assert_eq!(result.is_error, Some(true));
        let text = extract_text(&result);
        assert!(
            !text.contains("_mem_schema:"),
            "pre-resolve error markdown must not anchor: {text}"
        );
        if let Some(sc) = result.structured_content.as_ref() {
            assert!(
                sc.get("_mem_schema").is_none(),
                "pre-resolve error envelope must not anchor: {sc:?}"
            );
        }
    }
}

/// Agent-schema-priming contract — `memstead_mem_create` returns the
/// full per-type catalogue under `schema`, byte-identical to what
/// `memstead_schema(name=<resolved-schema>)` would ship for the same mem.
/// Primes the agent that just created the mem with everything they
/// need to write into it.
mod schema_payload {
    use super::*;

    /// Set up a permissive lifecycle server (sibling to
    /// `setup_lifecycle_server` further down — duplicated here so this
    /// module can run independently if the upstream helper moves).
    fn setup() -> (McpServer, TempDir) {
        let tmp = TempDir::new().unwrap();
        let settings = memstead_git_branch::test_support::auto_seed_with_settings(
            tmp.path(),
            WorkspaceSettings {
                mem_create_rules: vec![memstead_base::CreateRuleSetting {
                    pattern: "*".to_string(),
                    schemas: vec!["default@1.0.0".to_string()],
                    default_cross_links: None,
                }],
                mem_delete_rules: vec![],
                ..Default::default()
            },
        );
        let unified_settings = settings.clone();
        let _ = settings;
        // Install the create-rule policy on the unified engine
        // so memstead_mem_create_unified's gating sees it.
        // Canonicalise tmp.path() because TempDir returns a
        // symlink (e.g. /var/.../) on macOS while canonical
        // paths resolve to /private/var/...; the
        // `canonical.strip_prefix(workspace_root)` check needs
        // the two to match.
        let mut unified = setup_unified_test_engine(tmp.path());
        unified.set_settings(unified_settings);
        let canonical_root =
            std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
        unified.set_workspace_root(canonical_root);
        let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);
        (server, tmp)
    }

    /// Create a mem and return the response's structured payload.
    /// `include_schema:
    /// true` is required to surface the schema body — these tests
    /// exist *to verify* the inlined body, so the opt-in fires.
    fn create_and_get_payload(server: &McpServer, tmp: &TempDir, name: &str) -> serde_json::Value {
        let target = tmp.path().join(name);
        let result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
            title: None,
            description: None,
            subject: None,
            // These payload tests pin the FULL catalogue content;
            // the omitted-param default is lite (tested separately).
            schema_verbosity: Some("full".to_string()),
            write_guidance: Default::default(),
            name: name.to_string(),
            location: target.to_string_lossy().into_owned(),
            schema: "default@1.0.0".to_string(),

            vcs: None,
            note: Some("schema payload".to_string()),
            recovery: None,
            include_schema: true,
        }));
        assert!(
            !result.is_error.unwrap_or(false),
            "create must succeed: {result:?}"
        );
        result
            .structured_content
            .expect("structured_content present")
    }

    /// Create half: curation params applied
    /// at creation are visible on the loaded config and the
    /// configure tool's stable response.
    #[test]
    fn mem_create_with_curation_carries_all_three() {
        let (server, tmp) = setup();
        let target = tmp.path().join("curated");
        let result = server.memstead_mem_create(Parameters(TlsMemCreateParams {
            title: Some("Curated Library".to_string()),
            description: Some("One-line card text.".to_string()),
            subject: Some(crate::lifecycle::MemSubjectInput {
                scope: "Everything about curation.".to_string(),
                method: Some("Hand-written fixtures.".to_string()),
                exclusions: Some(vec!["History.".to_string()]),
            }),
            schema_verbosity: None,
            write_guidance: Default::default(),
            name: "curated".to_string(),
            location: target.to_string_lossy().into_owned(),
            schema: "default@1.0.0".to_string(),
            vcs: None,
            note: Some("curation at create".to_string()),
            recovery: None,
            include_schema: false,
        }));
        assert!(
            !result.is_error.unwrap_or(false),
            "create must succeed: {result:?}"
        );

        // The stable read-back: a no-field configure call returns
        // the post-create state.
        let state =
            server.memstead_mem_configure(Parameters(crate::lifecycle::MemConfigureParams {
                name: "curated".to_string(),
                title: None,
                description: None,
                subject: None,
                clear_subject: false,
                note: None,
            }));
        assert!(!state.is_error.unwrap_or(false), "{state:?}");
        let body = state.structured_content.expect("structured body");
        assert_eq!(body["title"], "Curated Library");
        assert_eq!(body["description"], "One-line card text.");
        assert_eq!(body["subject"]["scope"], "Everything about curation.");
        assert_eq!(body["subject"]["method"], "Hand-written fixtures.");
        assert_eq!(body["subject"]["exclusions"][0], "History.");
    }

    /// Configure half: set, overwrite, and
    /// clear each field on an existing mem; unknown mem refuses.
    #[test]
    fn mem_configure_sets_overwrites_and_clears() {
        let (server, tmp) = setup();
        let target = tmp.path().join("plain");
        let create = server.memstead_mem_create(Parameters(TlsMemCreateParams {
            title: None,
            description: None,
            subject: None,
            schema_verbosity: None,
            write_guidance: Default::default(),
            name: "plain".to_string(),
            location: target.to_string_lossy().into_owned(),
            schema: "default@1.0.0".to_string(),
            vcs: None,
            note: Some("plain create".to_string()),
            recovery: None,
            include_schema: false,
        }));
        assert!(!create.is_error.unwrap_or(false), "{create:?}");

        let configure = |title: Option<&str>,
                         description: Option<&str>,
                         subject: Option<crate::lifecycle::MemSubjectInput>,
                         clear_subject: bool| {
            server.memstead_mem_configure(Parameters(crate::lifecycle::MemConfigureParams {
                name: "plain".to_string(),
                title: title.map(String::from),
                description: description.map(String::from),
                subject,
                clear_subject,
                note: Some("configure test".to_string()),
            }))
        };

        // Set.
        let set = configure(
            Some("First Title"),
            Some("First description."),
            Some(crate::lifecycle::MemSubjectInput {
                scope: "Scope one.".to_string(),
                method: None,
                exclusions: None,
            }),
            false,
        );
        assert!(!set.is_error.unwrap_or(false), "{set:?}");
        let body = set.structured_content.unwrap();
        assert_eq!(body["title"], "First Title");
        assert_eq!(body["subject"]["scope"], "Scope one.");

        // Overwrite one field, leave the rest untouched.
        let over = configure(Some("Second Title"), None, None, false);
        let body = over.structured_content.unwrap();
        assert_eq!(body["title"], "Second Title");
        assert_eq!(body["description"], "First description.");
        assert_eq!(body["subject"]["scope"], "Scope one.");

        // Clear: empty string for the strings, clear_subject for
        // the block.
        let cleared = configure(Some(""), Some(""), None, true);
        let body = cleared.structured_content.unwrap();
        assert!(
            body["title"].is_null(),
            "cleared title must be null: {body}"
        );
        assert!(body["description"].is_null());
        assert!(body["subject"].is_null());

        // Unknown mem refuses with the typed code.
        let unknown =
            server.memstead_mem_configure(Parameters(crate::lifecycle::MemConfigureParams {
                name: "no-such-mem".to_string(),
                title: Some("X".to_string()),
                description: None,
                subject: None,
                clear_subject: false,
                note: None,
            }));
        assert!(unknown.is_error.unwrap_or(false));
        let err = unknown.structured_content.unwrap();
        assert_eq!(err["code"], "UNKNOWN_MEM", "{err}");
    }

    /// The operator's permission complement:
    /// curation params change nothing about the create allowlist
    /// gate — a rejected name refuses with the same typed code and
    /// detail shape; and configure against a read-only mount
    /// refuses READ_ONLY_MOUNT.
    #[test]
    fn mem_curation_changes_no_permission_gate() {
        // Restrictive create allowlist: only `allowed-*` passes.
        let tmp = TempDir::new().unwrap();
        let settings = memstead_git_branch::test_support::auto_seed_with_settings(
            tmp.path(),
            WorkspaceSettings {
                mem_create_rules: vec![memstead_base::CreateRuleSetting {
                    pattern: "allowed-*".to_string(),
                    schemas: vec!["default@1.0.0".to_string()],
                    default_cross_links: None,
                }],
                mem_delete_rules: vec![],
                ..Default::default()
            },
        );
        let mut unified = setup_unified_test_engine(tmp.path());
        unified.set_settings(settings);
        let canonical_root =
            std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
        unified.set_workspace_root(canonical_root);
        let server = McpServer::new(unified, crate::config::DEFAULT_TOKEN_BUDGET);

        let target = tmp.path().join("denied-x");
        let refused = server.memstead_mem_create(Parameters(TlsMemCreateParams {
            title: Some("Sneaky Title".to_string()),
            description: Some("Sneaky description.".to_string()),
            subject: None,
            schema_verbosity: None,
            write_guidance: Default::default(),
            name: "denied-x".to_string(),
            location: target.to_string_lossy().into_owned(),
            schema: "default@1.0.0".to_string(),
            vcs: None,
            note: None,
            recovery: None,
            include_schema: false,
        }));
        assert!(refused.is_error.unwrap_or(false), "{refused:?}");
        let err = refused.structured_content.unwrap();
        assert_eq!(err["code"], "MEM_PATH_NOT_ALLOWED", "{err}");
        assert!(
            err["details"]["candidate"].is_string()
                && err["details"]["patterns"].is_array()
                && err["details"]["reason"].is_string(),
            "detail shape must match the param-less refusal: {err}"
        );

        // Read-only mount: build a real sealed archive, register
        // it read-only on the live engine (the exact mount shape
        // `memstead install` produces), and configure against it.
        {
            let src = tmp.path().join("frozen-src").join("frozen");
            std::fs::create_dir_all(src.join(".memstead")).unwrap();
            std::fs::write(
                src.join(".memstead/config.json"),
                r#"{"version":"1.0.0","schema":"default@1.0.0"}"#,
            )
            .unwrap();
            std::fs::write(
                    src.join("solo.md"),
                    "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-01-15\nlevel: M0\n---\n# Solo\n\n## Identity\n\nA.\n\n## Purpose\n\nB.\n",
                ).unwrap();
            let archive = tmp.path().join("frozen.mem");
            let config = memstead_schema::load_and_validate(&src).unwrap();
            memstead_git_branch::ops::export::export_mem(&src, &config, &archive, None, None, None)
                .unwrap();

            let unified = server.unified_engine();
            let mut engine = unified.lock().unwrap_or_else(|e| e.into_inner());
            let mount = memstead_base::Mount {
                mem: "frozen".to_string(),
                schema: Some("default@1.0.0".parse().unwrap()),
                storage: memstead_base::MountStorage::Archive {
                    path: archive.clone(),
                },
                capability: memstead_base::MountCapability::ReadOnly,
                lifecycle: memstead_base::MountLifecycle::Eager,
                cross_linkable: false,
                migration_target: None,
            };
            let backend: Box<dyn memstead_base::MemBackend> =
                Box::new(memstead_base::storage::ArchiveBackend::new(archive));
            engine
                .register_read_mount(
                    mount,
                    backend,
                    memstead_base::MemOrigin::RuntimeCreated {
                        at: std::time::SystemTime::now(),
                        by_tool: "test",
                    },
                )
                .unwrap();
        }
        let ro = server.memstead_mem_configure(Parameters(crate::lifecycle::MemConfigureParams {
            name: "frozen".to_string(),
            title: Some("New Title".to_string()),
            description: None,
            subject: None,
            clear_subject: false,
            note: None,
        }));
        assert!(ro.is_error.unwrap_or(false), "{ro:?}");
        let err = ro.structured_content.unwrap();
        assert_eq!(err["code"], "READ_ONLY_MOUNT", "{err}");
    }

    /// The schema payload exists, names the pinned schema, and ships
    /// the full type catalogue (not the lite `types_summary`).
    #[test]
    fn mem_create_response_carries_full_schema_catalogue() {
        let (server, tmp) = setup();
        let payload = create_and_get_payload(&server, &tmp, "fresh-payload");
        let schema = payload
            .get("schema")
            .expect("schema block present in mem_create response");
        assert_eq!(schema["ref"].as_str(), Some("default@1.0.0"));
        assert!(
            schema.get("types").is_some(),
            "full per-type catalogue must ship (types[]), not types_summary"
        );
        assert!(
            schema.get("types_summary").is_none(),
            "lite summary must not ship when types[] is requested"
        );
        assert!(
            schema["types"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "types[] must be non-empty"
        );
    }

    /// Schema-level
    /// `default_writing_guidance` is surfaced at the top level of the
    /// schema payload so plugin-side resolvers (`writing-guidance.mjs`)
    /// can concatenate the schema-generic prose with per-mem
    /// additions in one place. Authored as block scalars in YAML;
    /// payload value is the raw `String` (chomp behaviour follows the
    /// loader's serde rules).
    #[test]
    fn schema_payload_surfaces_default_writing_guidance() {
        // Build a minimal schema fixture in-memory carrying both
        // `avoid` and `goal`. Direct call to `build_schema_payload`
        // — the field flows through every consumer (memstead_overview,
        // memstead_mem_create) via the same helper.
        let manifest_yaml = r#"name: tests-dwg
version: 0.1.0
description: dwg test schema
when_to_use: tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: REFERENCES
      description: ref
      default_weight: 0.5
    - name: _default
      description: Fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
default_writing_guidance:
  avoid: |
    First avoid line.

    - bullet
  goal: |
    Goal prose.
"#;
        let type_yaml = r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        let schema = Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest_yaml,
                &[("sample".to_string(), type_yaml.to_string())],
            )
            .expect("dwg fixture must parse"),
        );
        let payload = render::build_schema_payload(
            &schema,
            vec!["v".to_string()],
            render::SchemaVerbosity::Full,
            render::OriginClass::FirstParty,
        );
        let dwg = payload
            .get("default_writing_guidance")
            .expect("default_writing_guidance must surface at top level");
        assert!(
            dwg["avoid"]
                .as_str()
                .map(|s| s.contains("First avoid line") && s.contains("- bullet"))
                .unwrap_or(false),
            "avoid block scalar must reach the wire as a string; got {dwg}"
        );
        assert!(
            dwg["goal"]
                .as_str()
                .map(|s| s.contains("Goal prose"))
                .unwrap_or(false),
            "goal block scalar must reach the wire; got {dwg}"
        );
    }

    /// Every
    /// field carries its `filterable` posture so an agent reads
    /// `filters` / `range_filters` eligibility straight from the
    /// schema body — `"equality"`, `"range"`, or `null`. Also pins
    /// that the engine-stamped `created_date` is range-
    /// filterable.
    #[test]
    fn schema_payload_surfaces_filterable_posture_per_field() {
        let manifest_yaml = r#"name: tests-filterable
version: 0.1.0
description: filterable test schema
when_to_use: tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: Fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let type_yaml = r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: state
    field_type: string
    filterable: equality
  - key: due_on
    description: a date
    field_type: date
    filterable: range
  - key: note
    description: freeform
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        let schema = Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest_yaml,
                &[("sample".to_string(), type_yaml.to_string())],
            )
            .expect("filterable fixture must parse"),
        );
        let payload = render::build_schema_payload(
            &schema,
            vec!["v".to_string()],
            render::SchemaVerbosity::Full,
            render::OriginClass::FirstParty,
        );
        let fields = payload["types"][0]["fields"]
            .as_array()
            .expect("fields array present");
        let field = |name: &str| {
            fields
                .iter()
                .find(|f| f["name"] == name)
                .unwrap_or_else(|| panic!("field {name} present; got {payload}"))
        };
        assert_eq!(field("status")["filterable"], serde_json::json!("equality"));
        assert_eq!(field("due_on")["filterable"], serde_json::json!("range"));
        assert!(
            field("note")["filterable"].is_null(),
            "non-filterable field surfaces null, not an absent key"
        );
        // The engine-stamped `created_date` base field is
        // range-filterable (auto-prepended by the loader).
        assert_eq!(
            field("created_date")["filterable"],
            serde_json::json!("range"),
            "created_date must be range-filterable; got {payload}"
        );
    }

    /// The schema-level `alias_target_rel_type` pointer surfaces at
    /// the top level of the schema payload so agents can predict
    /// from one introspection call which rel-type body wiki-links
    /// auto-emit. Schemas without the pointer ship no key at all.
    #[test]
    fn schema_payload_surfaces_alias_target_rel_type() {
        let manifest_yaml = r#"name: tests-alias
version: 0.1.0
description: alias test schema
when_to_use: tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: REFERENCES
      description: ref
      default_weight: 0.5
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
        let type_yaml = r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        let schema = Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest_yaml,
                &[("sample".to_string(), type_yaml.to_string())],
            )
            .expect("alias fixture must parse"),
        );
        let payload = render::build_schema_payload(
            &schema,
            vec!["v".to_string()],
            render::SchemaVerbosity::Full,
            render::OriginClass::FirstParty,
        );
        assert_eq!(
            payload["alias_target_rel_type"].as_str(),
            Some("REFERENCES"),
            "alias_target_rel_type must surface at top level; got {payload}"
        );
    }

    /// A schema omitting `alias_target_rel_type:` must not ship the
    /// key at all — keeps the wire envelope minimal for schemas
    /// that opt out of alias synthesis.
    #[test]
    fn schema_payload_omits_alias_target_rel_type_when_absent() {
        let manifest_yaml = r#"name: tests-alias-absent
version: 0.1.0
description: schema without alias pointer
when_to_use: tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: Fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let type_yaml = r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        let schema = Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest_yaml,
                &[("sample".to_string(), type_yaml.to_string())],
            )
            .expect("opt-out fixture must parse"),
        );
        let payload = render::build_schema_payload(
            &schema,
            vec!["v".to_string()],
            render::SchemaVerbosity::Full,
            render::OriginClass::FirstParty,
        );
        assert!(
            payload.get("alias_target_rel_type").is_none(),
            "key must be absent for schemas without the pointer; got {payload}"
        );
    }

    /// A schema with a `cross_mem_relationships:`
    /// section surfaces it on the response as a top-level field
    /// with the same shape as the YAML (array of
    /// `{ to_schema, definitions }`). The intra-mem
    /// `relationships` block continues to round-trip.
    #[test]
    fn schema_payload_surfaces_cross_mem_relationships() {
        let manifest_yaml = r#"name: tests-cv
version: 0.1.0
description: cv test schema
when_to_use: tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: other
    definitions:
      - name: ADDRESSES
        description: outbound
        default_weight: 1.0
        source_types: [sample]
        target_types: [foreign_type]
community:
  resolution: 1.0
  seed: 42
"#;
        let type_yaml = r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        let schema = Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest_yaml,
                &[("sample".to_string(), type_yaml.to_string())],
            )
            .expect("cv fixture must parse"),
        );
        let payload = render::build_schema_payload(
            &schema,
            vec!["v".to_string()],
            render::SchemaVerbosity::Full,
            render::OriginClass::FirstParty,
        );
        let cv = payload
            .get("cross_mem_relationships")
            .expect("cross_mem_relationships must surface at top level")
            .as_array()
            .expect("array");
        assert_eq!(cv.len(), 1);
        assert_eq!(cv[0]["to_schema"].as_str(), Some("other"));
        let defs = cv[0]["definitions"].as_array().expect("definitions array");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["name"].as_str(), Some("ADDRESSES"));
        assert_eq!(
            defs[0]["source_types"].as_array().unwrap()[0].as_str(),
            Some("sample")
        );
        assert_eq!(
            defs[0]["target_types"].as_array().unwrap()[0].as_str(),
            Some("foreign_type")
        );
        // Intra-mem relationships block continues to round-trip.
        let intra = payload["relationships"].as_array().unwrap();
        assert!(intra.iter().any(|r| r["name"] == "PART_OF"));
    }

    /// A schema with no `cross_mem_relationships:` declarations
    /// produces no top-level key — presence-of-key is the
    /// consumer's signal that the schema speaks cross-mem.
    #[test]
    fn schema_payload_includes_cross_mem_relationships_for_default() {
        // `default@1.0.0` declares a cross-mem REFERENCES relationship into
        // the `software` schema (the "knowledge mem → code mem" pairing), so
        // its schema payload must surface `cross_mem_relationships` naming
        // that entry. The omit-when-absent serialization contract (the key is
        // absent, never null/empty, when a schema declares no cross-mem) stays
        // covered by the sibling `schema_payload_omits_default_writing_guidance_when_absent`.
        let (server, tmp) = setup();
        let payload = create_and_get_payload(&server, &tmp, "cv");
        let schema = &payload["schema"];
        let cmr = schema.get("cross_mem_relationships").unwrap_or_else(|| {
            panic!(
                "default now declares cross-mem into software → key must be present; got {schema}"
            )
        });
        let s = cmr.to_string();
        assert!(
            s.contains("software") && s.contains("REFERENCES"),
            "the cross-mem payload names REFERENCES into software; got {cmr}"
        );
    }

    /// A schema without `default_writing_guidance` produces no key at
    /// all (presence-of-key is the consumer's signal — never a null
    /// value or empty object).
    #[test]
    fn schema_payload_omits_default_writing_guidance_when_absent() {
        // Default schema carries no DWG; reuse the existing test
        // fixture's `default@1.0.0` payload.
        let (server, tmp) = setup();
        let payload = create_and_get_payload(&server, &tmp, "no-dwg");
        let schema = &payload["schema"];
        assert!(
            schema.get("default_writing_guidance").is_none(),
            "default schema has no DWG → key must be absent, not null/empty; got {schema}"
        );
    }

    /// Item 05: the schema's `_default` rel-type is the internal
    /// weight-fallback knob (sets the edge weight every
    /// `_default`-less rel-type inherits) and is *not* a usable
    /// rel-type on `memstead_relate` (the relate path rejects it with
    /// `INVALID_REL_TYPE`). The schema response must not advertise
    /// it in `relationships[]` — pre-fix every agent reading the
    /// schema paid one round-trip per session learning the
    /// asymmetry by trial.
    #[test]
    fn schema_payload_omits_internal_default_rel_type() {
        // In-memory schema fixture — needs `_default` in the
        // relationship list so the suppression has something to
        // strip. Direct call to `build_schema_payload` so the
        // assertion runs against the helper that every consumer
        // (memstead_schema, memstead_overview, memstead_mem_create) uses.
        let manifest_yaml = r#"name: tests-no-default
version: 0.1.0
description: t
when_to_use: tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: REFERENCES
      description: ref
      default_weight: 0.5
    - name: _default
      description: Fallback weight for any relationship not otherwise specified.
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let type_yaml = r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        let schema = Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest_yaml,
                &[("sample".to_string(), type_yaml.to_string())],
            )
            .expect("fixture must parse"),
        );
        let payload = render::build_schema_payload(
            &schema,
            vec!["v".to_string()],
            render::SchemaVerbosity::Full,
            render::OriginClass::FirstParty,
        );
        let rels = payload["relationships"]
            .as_array()
            .expect("relationships array present");
        // The user-facing rel-types survive; `_default` is filtered.
        let names: Vec<&str> = rels.iter().filter_map(|r| r["name"].as_str()).collect();
        assert!(
            names.contains(&"PART_OF"),
            "user-facing rel-types must survive; got {names:?}",
        );
        assert!(
            names.contains(&"REFERENCES"),
            "user-facing rel-types must survive; got {names:?}",
        );
        assert!(
            !names.contains(&"_default"),
            "internal `_default` weight fallback must not surface on the agent-facing relationship list; got {names:?}",
        );
    }

    /// Schema response
    /// surfaces `acyclic` per rel-type and `no_self_loop_relationships`
    /// per type. Agents predict cycle-check + self-loop refusal from
    /// introspection without trial-and-error.
    #[test]
    fn schema_payload_carries_acyclic_and_no_self_loop_relationships() {
        let (server, tmp) = setup();
        let payload = create_and_get_payload(&server, &tmp, "introspection-cycle");
        // The default schema declares DEPENDS_ON as acyclic.
        let rels = payload["schema"]["relationships"]
            .as_array()
            .expect("relationships array present");
        let depends_on = rels
            .iter()
            .find(|r| r["name"] == "DEPENDS_ON")
            .expect("DEPENDS_ON rel-type present in default schema");
        assert_eq!(
            depends_on["acyclic"].as_bool(),
            Some(true),
            "DEPENDS_ON must carry acyclic=true: {depends_on}"
        );
        // The default schema's spec type lists [DEPENDS_ON, USES]
        // in no_self_loop_relationships.
        let types = payload["schema"]["types"]
            .as_array()
            .expect("types array present");
        let spec = types
            .iter()
            .find(|t| t["name"] == "spec")
            .expect("spec type present in default schema");
        let no_self_loop = spec["no_self_loop_relationships"]
            .as_array()
            .expect("no_self_loop_relationships array present");
        let names: Vec<&str> = no_self_loop.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            names.contains(&"DEPENDS_ON") && names.contains(&"USES"),
            "spec's no_self_loop_relationships must list DEPENDS_ON and USES: {names:?}"
        );
    }

    /// Sections in the payload carry their `write_rules` array.
    #[test]
    fn schema_payload_sections_carry_write_rules() {
        let (server, tmp) = setup();
        let payload = create_and_get_payload(&server, &tmp, "section-rules");
        let types = payload["schema"]["types"]
            .as_array()
            .expect("types array present");

        // At least one type must have at least one section with a
        // populated write_rules array. Built-in `default@1.0.0` ships
        // genuine per-section rules for spec, decision, etc.
        let any_with_rules = types.iter().any(|t| {
            t["sections"]
                .as_array()
                .map(|sections| {
                    sections.iter().any(|s| {
                        s["write_rules"]
                            .as_array()
                            .map(|rules| !rules.is_empty())
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        });
        assert!(
            any_with_rules,
            "at least one section must ship a non-empty write_rules array; types: {types:#?}"
        );

        // Every section must have a `write_rules` field (even when
        // empty) so consumers can branch on its presence as
        // schema-grade metadata, not best-effort drift.
        for t in types {
            for section in t["sections"].as_array().unwrap_or(&Vec::new()) {
                assert!(
                    section.get("write_rules").is_some(),
                    "every section must carry a write_rules field; type: {t:?}"
                );
            }
        }
    }

    /// Enum-typed metadata fields ship their `enum` value list — the
    /// agent reads the allowed values without a follow-up call.
    #[test]
    fn schema_payload_enum_fields_carry_allowed_values() {
        let (server, tmp) = setup();
        let payload = create_and_get_payload(&server, &tmp, "enum-fields");
        let types = payload["schema"]["types"].as_array().expect("types array");

        // Built-in `default@1.0.0` has at least one enum-typed field
        // (e.g. `status` on spec). Pin that the enum surfaces.
        let any_with_enum = types.iter().any(|t| {
            t["fields"]
                .as_array()
                .map(|fields| fields.iter().any(|f| f.get("enum").is_some()))
                .unwrap_or(false)
        });
        assert!(
            any_with_enum,
            "at least one metadata field must ship an `enum` array; types: {types:#?}"
        );
    }

    /// Type-level `writing_guidance` and the relationship-vocabulary
    /// `when_to_use` strings must ship — these are the cure for the
    /// agent walking blind through the schema.
    #[test]
    fn schema_payload_carries_writing_guidance_and_when_to_use() {
        let (server, tmp) = setup();
        let payload = create_and_get_payload(&server, &tmp, "guidance-when-to-use");
        let schema = &payload["schema"];

        let any_writing_guidance = schema["types"]
            .as_array()
            .map(|types| {
                types.iter().any(|t| {
                    t["writing_guidance"]
                        .as_array()
                        .map(|g| !g.is_empty())
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        assert!(
            any_writing_guidance,
            "type-level writing_guidance must ship for at least one type"
        );

        let any_when_to_use = schema["relationships"]
            .as_array()
            .map(|rels| rels.iter().any(|r| r.get("when_to_use").is_some()))
            .unwrap_or(false);
        assert!(
            any_when_to_use,
            "relationships[].when_to_use must ship in the catalogue"
        );
    }

    /// The schema payload from `memstead_mem_create` matches the schema
    /// payload from `memstead_schema(name=<resolved-schema>)` — pins that
    /// both surfaces share one helper (`build_schema_payload`) and do
    /// not drift. The priming contract anchors on `memstead_schema`
    /// rather than on overview.
    #[test]
    fn mem_create_payload_matches_memstead_schema() {
        let (server, tmp) = setup();
        let payload = create_and_get_payload(&server, &tmp, "byte-identical");
        let create_schema = payload["schema"].clone();
        let schema_ref = create_schema["ref"].as_str().unwrap().to_string();

        // memstead_schema returns the schema body as a JSON
        // structured-content response — byte-equality with the
        // priming payload from memstead_mem_create is the contract,
        // at matching verbosity (both full here; the shared
        // omitted-param default is lite on both surfaces).
        let schema_result = server.memstead_schema(Parameters(SchemaParams {
            verbosity: Some("full".to_string()),
            name: Some(schema_ref.clone()),
            mem: None,
            types: None,
            token_budget: None,
        }));
        assert!(
            !schema_result.is_error.unwrap_or(false),
            "memstead_schema must succeed: {schema_result:?}"
        );
        let schema_payload = schema_result
            .structured_content
            .clone()
            .expect("memstead_schema returns structured_content");

        // The two payloads share one helper and must agree
        // field-by-field except for `used_by`, which is the mem
        // list at the moment of the call (mem_create's response
        // captures it during creation; the engine's reverse-index
        // computes the same set from the registered mems).
        let mut a = create_schema.clone();
        let mut b = schema_payload.clone();
        // Both should now list ["byte-identical"]. Strip the field
        // before comparison to keep the assertion explicit.
        if let Some(obj) = a.as_object_mut() {
            obj.remove("used_by");
        }
        if let Some(obj) = b.as_object_mut() {
            obj.remove("used_by");
        }
        assert_eq!(
            a, b,
            "mem_create.schema and memstead_schema must return identical payloads (modulo used_by)"
        );

        // `used_by` itself must list the just-created mem on
        // both surfaces.
        for (label, payload) in [
            ("mem_create", &create_schema),
            ("memstead_schema", &schema_payload),
        ] {
            let used_by = payload["used_by"]
                .as_array()
                .unwrap_or_else(|| panic!("{label}.used_by must be an array; got {payload}"));
            let names: Vec<&str> = used_by.iter().filter_map(|v| v.as_str()).collect();
            assert!(
                names.contains(&"byte-identical"),
                "{label}.used_by must list the just-created mem; got {names:?}"
            );
        }

        // Sanity guard against the legacy overview path: legacy
        // include=["schema_types"] no longer works, and full
        // schema bodies must not surface in overview anymore.
        let overview = server.memstead_overview(Parameters(OverviewParams {
            rebuild: Some(true),
            chunk: None,
            mem: Some("byte-identical".to_string()),
            include: None,
            token_budget: Some(32_000),
        }));
        let overview_text = extract_text(&overview);
        assert!(
            overview_text.contains("default@1.0.0"),
            "overview must reference the same schema ref"
        );
        assert!(
            !overview_text.contains("**Types:**"),
            "Types must NOT render under overview — moved to memstead_schema"
        );
        // Relationships and types no longer render under overview —
        // they live on memstead_schema's body, which we already
        // byte-equality-compared above.
        assert!(
            !overview_text.contains("**Relationships:**"),
            "Relationship vocabulary must NOT render under overview"
        );
    }

    /// Shared read-envelope contract — the MCP `memstead_schema` path and a
    /// direct, rmcp-free call to the relocated
    /// `render::build_schema_payload` builder emit identical bytes.
    /// Proves schema-read is produced by one shared, transport-neutral
    /// builder reachable with no rmcp type in the path, so a future
    /// `/api/schema` is not a hand-mirrored third copy.
    #[test]
    fn memstead_schema_mcp_path_matches_direct_builder_bytes() {
        let (server, tmp) = setup();
        // Register a mem so `used_by` resolves to a known set.
        let _ = create_and_get_payload(&server, &tmp, "byte-identical-direct");

        // MCP path: structured_content of the memstead_schema tool at
        // its omitted-param default (lite).
        let schema_result = server.memstead_schema(Parameters(SchemaParams {
            verbosity: None,
            name: Some("default@1.0.0".to_string()),
            mem: None,
            types: None,
            token_budget: None,
        }));
        assert!(
            !schema_result.is_error.unwrap_or(false),
            "memstead_schema must succeed: {schema_result:?}"
        );
        let mcp_payload = schema_result
            .structured_content
            .clone()
            .expect("memstead_schema returns structured_content");

        // Direct rmcp-free path: resolve the same schema Arc and the
        // same `used_by` the handler computes, then call the shared
        // builder in memstead-base::render with no rmcp type involved.
        let direct_payload = {
            let engine = server.unified_engine().lock().unwrap();
            let parsed: memstead_schema::SchemaRef = "default@1.0.0".parse().expect("ref parses");
            let schema = find_schema_unified(&engine, &parsed)
                .cloned()
                .expect("default schema resolves");
            let canon = format!("{}@{}", schema.manifest.name, schema.version);
            let mut used_by: Vec<String> = engine
                .mounts()
                .iter()
                .filter(|m| {
                    m.schema.as_ref().map(|s| s.to_string()).as_deref() == Some(canon.as_str())
                })
                .map(|m| m.mem.clone())
                .collect();
            used_by.sort();
            render::build_schema_payload(
                &schema,
                used_by,
                render::SchemaVerbosity::Lite,
                render::OriginClass::FirstParty,
            )
        };

        assert_eq!(
            mcp_payload, direct_payload,
            "MCP schema-read structured_content must equal the direct builder payload"
        );
        // Byte-level identity, not just structural equality.
        assert_eq!(
            serde_json::to_string(&mcp_payload).unwrap(),
            serde_json::to_string(&direct_payload).unwrap(),
            "serialized bytes must be identical across MCP and direct call"
        );
    }
}

/// Every schema-bound failure carries recovery payload. Tests pin the per-code shape so
/// the contract the agent reads on stumbling does not silently drift.
mod recovery_payload {
    use super::*;

    /// Pull the typed `{code, message, details}` envelope from an
    /// error-response's structured_content.
    fn envelope_payload(result: &CallToolResult) -> serde_json::Value {
        assert_eq!(result.is_error, Some(true), "expected error response");
        result
            .structured_content
            .as_ref()
            .cloned()
            .expect("structured_content present on errors")
    }

    /// `MISSING_REQUIRED_SECTION` fires as a typed *error*
    /// envelope on the create path, not a warning. Pre-fix the
    /// section omission surfaced as a warning while the entity
    /// landed with empty placeholders — the resulting on-disk
    /// state then failed the install-time strict validator, so
    /// the export-then-install round-trip broke silently. The
    /// refusal carries the same `details` shape the warning
    /// historically shipped (per-section entries with
    /// `write_rules`, plus the top-level `type_guidance` map
    /// keyed by `entity_type`).
    #[test]
    fn missing_required_section_carries_type_guidance_top_level() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Bare Spec".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            // Omit identity/purpose so MISSING_REQUIRED_SECTION fires.
            sections: None,
            metadata: None,
            relations: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        // create now refuses with a typed envelope; fish the
        // structured_content for the recovery payload.
        assert_eq!(
            result.is_error,
            Some(true),
            "missing required sections must refuse the create",
        );
        let payload = result
            .structured_content
            .as_ref()
            .cloned()
            .expect("error response present");
        assert_eq!(payload["code"], "MISSING_REQUIRED_SECTION");
        // Per-section payload mirrors the pre-fix warning shape.
        let sections = payload["details"]["sections"]
            .as_array()
            .cloned()
            .expect("details.sections present");
        assert!(
            !sections.is_empty(),
            "details.sections must list at least one missing key: {payload}",
        );
        let first = &sections[0];
        assert_eq!(first["entity_type"], "spec");
        assert!(
            first["write_rules"].is_array(),
            "per-section write_rules array must ship: {first:?}"
        );

        // Top-level type_guidance map carries the entity type's
        // write_rules exactly once, keyed by `entity_type`.
        let guidance = payload["details"]["type_guidance"]
            .as_object()
            .expect("details.type_guidance map present");
        let spec_rules = guidance
            .get("spec")
            .and_then(|v| v.as_array())
            .expect("details.type_guidance.spec array present");
        assert!(
            !spec_rules.is_empty(),
            "type-level write_rules must be non-empty for `spec`"
        );
    }

    /// Cross-gate pre-announcement through the
    /// MCP envelope: a create failing BOTH the section gate and the
    /// metadata gate carries
    /// `details.pre_announced.required_field_unset.missing[]` in
    /// `REQUIRED_FIELD_UNSET`'s element shape — and announces only
    /// what that gate would actually demand (a required field WITH
    /// a default is not announced). The same refusal against a
    /// metadata-complete body omits the key entirely, keeping the
    /// single-gate refusal byte-compatible.
    #[test]
    fn missing_required_section_pre_announces_unset_required_metadata() {
        // `obligation@0.1.0` decision: required section `decision`,
        // required-no-default field `decided_on`, and `standing`
        // (required WITH default — must not be announced).
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("duties");
        fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        fs::write(
            mem_dir.join(".memstead/config.json"),
            r#"{"version":"0.1.0","schema":"obligation@0.1.0","mediums":{},"projections":{}}"#,
        )
        .unwrap();
        memstead_git_branch::test_support::auto_seeded_settings(tmp.path());
        let engine = setup_unified_test_engine(tmp.path());
        let server = McpServer::new(engine, crate::config::DEFAULT_TOKEN_BUDGET);

        let base_params = |metadata: Option<indexmap::IndexMap<String, String>>| CreateParams {
            anchors: None,
            title: "Cold Duty".to_string(),
            entity_type: "decision".to_string(),
            mem: Some("duties".to_string()),
            sections: None,
            metadata,
            relations: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        };

        // Both gates unmet: sections absent, metadata absent.
        let result = server.memstead_create(Parameters(base_params(None)));
        let payload = envelope_payload(&result);
        assert_eq!(payload["code"], "MISSING_REQUIRED_SECTION");
        let missing = payload["details"]["pre_announced"]["required_field_unset"]["missing"]
            .as_array()
            .cloned()
            .expect("details.pre_announced.required_field_unset.missing[] present");
        assert_eq!(missing[0]["field"], "decided_on");
        assert!(
            missing[0].get("description").is_some() && missing[0].get("enum_values").is_some(),
            "element shape mirrors REQUIRED_FIELD_UNSET missing[]: {missing:?}"
        );
        assert!(
            !missing.iter().any(|m| m["field"] == "standing"),
            "a required field WITH a default is auto-filled — announcing it would be false"
        );
        // The text channel carries the pre-announcement too.
        let text = extract_text(&result);
        assert!(
            text.contains("decided_on"),
            "prose names the pre-announced field: {text}"
        );

        // Metadata complete: single-gate refusal, no pre_announced
        // key — byte-compatible with the established shape.
        let result = server.memstead_create(Parameters(base_params(Some(
            indexmap::IndexMap::from_iter([("decided_on".to_string(), "2026-08-19".to_string())]),
        ))));
        let payload = envelope_payload(&result);
        assert_eq!(payload["code"], "MISSING_REQUIRED_SECTION");
        assert!(
            payload["details"].get("pre_announced").is_none(),
            "single-gate refusal must omit pre_announced: {payload}"
        );
    }

    /// F9 stable empty shape: `type_guidance` ships as `{}` even
    /// when no MissingRequiredSection / MissingRequiredField
    /// warnings fire, so consumers don't branch on field presence.
    #[test]
    fn type_guidance_ships_empty_shape_when_no_warnings() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Complete Spec".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(indexmap::IndexMap::from_iter([
                ("identity".to_string(), "what it is".to_string()),
                ("purpose".to_string(), "why it exists".to_string()),
            ])),
            metadata: None,
            relations: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        let payload = result
            .structured_content
            .as_ref()
            .cloned()
            .expect("dry-run response present");
        let guidance = payload["type_guidance"]
            .as_object()
            .expect("type_guidance map present even on warning-free create");
        assert!(
            guidance.is_empty(),
            "no warnings → empty type_guidance, got {guidance:?}"
        );
    }

    /// `INVALID_REL_TYPE` (error) carries the full schema relationship
    /// vocabulary plus a nearest-match suggestion when the typo is
    /// close to a declared rel.
    #[test]
    fn invalid_rel_type_carries_allowed_and_suggestion() {
        let (server, _tmp) = setup_dual_test_engine();
        // Typo: PART_O instead of PART_OF
        let result = server.memstead_relate(Parameters(RelateParams {
            relations: vec![RelateOpInput {
                from: "specs--entity-a".to_string(),
                to: "specs--entity-b".to_string(),
                rel_type: "PART_O".to_string(),
                remove: None,
                description: None,
            }],
            note: None,
            role: None,
            identity: None,
            dry_run: None,
        }));
        let env = envelope_payload(&result);
        assert_eq!(env["code"].as_str(), Some("INVALID_REL_TYPE"));
        let allowed = env["details"]["allowed"]
            .as_array()
            .expect("allowed[] present");
        assert!(
            !allowed.is_empty(),
            "allowed[] must list the schema vocabulary"
        );
        // Each entry has `name` and `when_to_use` (Option may be None).
        assert!(allowed.iter().all(|h| h["name"].is_string()));
        // Strsim hits PART_OF for PART_O.
        assert_eq!(
            env["details"]["suggestion"].as_str(),
            Some("PART_OF"),
            "nearest-match suggestion should point at PART_OF"
        );
    }

    /// `INVALID_REL_TYPE` also fires on syntactic violations — and
    /// even there the recovery payload ships the allowed vocabulary.
    #[test]
    fn invalid_rel_type_syntactic_still_carries_allowed() {
        let (server, _tmp) = setup_dual_test_engine();
        // Spaces are syntactically illegal.
        let result = server.memstead_relate(Parameters(RelateParams {
            relations: vec![RelateOpInput {
                from: "specs--entity-a".to_string(),
                to: "specs--entity-b".to_string(),
                rel_type: "alt rel type".to_string(),
                remove: None,
                description: None,
            }],
            note: None,
            role: None,
            identity: None,
            dry_run: None,
        }));
        let env = envelope_payload(&result);
        assert_eq!(env["code"].as_str(), Some("INVALID_REL_TYPE"));
        assert!(
            env["details"]["allowed"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "syntactic-error path must still ship allowed[]"
        );
    }

    /// `INVALID_ENUM_VALUE` (error) carries `allowed`, the field's
    /// `field_description`, a nearest-match `suggestion`, and the
    /// type's `type_write_rules`.
    #[test]
    fn invalid_enum_value_carries_full_recovery_payload() {
        let (server, _tmp) = setup_dual_test_engine();
        let mut metadata = IndexMap::new();
        metadata.insert("level".to_string(), "M99".to_string());
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Bad Level".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(IndexMap::from_iter([
                ("identity".to_string(), "x".to_string()),
                ("purpose".to_string(), "x".to_string()),
            ])),
            metadata: Some(metadata),
            relations: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        let env = envelope_payload(&result);
        assert_eq!(env["code"].as_str(), Some("INVALID_ENUM_VALUE"));
        assert_eq!(env["details"]["field"].as_str(), Some("level"));
        let allowed = env["details"]["allowed"]
            .as_array()
            .expect("allowed[] present");
        assert!(allowed.iter().any(|v| v == "M0"));
        assert!(
            env["details"]["field_description"].is_string(),
            "field_description must ship as String (may be empty); got: {env:?}"
        );
        assert_eq!(env["details"]["entity_type"].as_str(), Some("spec"));
        assert!(
            env["details"]["type_write_rules"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "type-level write_rules must ship for `spec`"
        );
    }

    /// `REQUIRED_FIELD_UNSET` (error) — try to drop a field with a
    /// `default_value` (i.e. effectively required). Carries
    /// `field_description`, `enum_values` (when applicable), and
    /// `type_write_rules`.
    #[test]
    fn required_field_unset_carries_full_recovery_payload() {
        // `EngineError::RequiredFieldUnset` ships `field`,
        // `entity_type`, `field_description`, `enum_values`,
        // `type_write_rules` in the recovery envelope.
        let (server, _tmp) = setup_dual_test_engine();
        // Read the current hash for entity-a.
        let read = server.memstead_entity(Parameters(EntityParams {
            id: "specs--entity-a".to_string(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }));
        let read_text = extract_text(&read);
        let hash = read_text
            .lines()
            .find(|l| l.starts_with("_hash:"))
            .map(|l| l.trim_start_matches("_hash:").trim().to_string())
            .expect("_hash present");

        // Try to unset `level` — required (default_value = M0, not optional).
        let result = server.memstead_update(Parameters(UpdateParams {
            anchors: None,
            relations_unset: None,
            anchors_unset: None,
            id: "specs--entity-a".to_string(),
            expected_hash: Some(hash),
            sections: None,
            append_sections: None,
            patch_sections: None,
            sections_unset: None,
            metadata: None,
            metadata_unset: Some(vec!["level".to_string()]),
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
            declare_relations: None,
        }));
        let env = envelope_payload(&result);
        assert_eq!(env["code"].as_str(), Some("REQUIRED_FIELD_UNSET"));
        assert_eq!(env["details"]["field"].as_str(), Some("level"));
        assert_eq!(env["details"]["entity_type"].as_str(), Some("spec"));
        assert!(env["details"]["field_description"].is_string());
        let enums = env["details"]["enum_values"]
            .as_array()
            .expect("enum_values present");
        assert!(enums.iter().any(|v| v == "M0"));
        assert!(
            env["details"]["type_write_rules"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false)
        );
    }

    /// Regression — `UNKNOWN_SECTION` keeps its existing
    /// `details.declared` + `suggestion` shape so additive changes
    /// do not accidentally rewrite a code that
    /// already followed the recovery-payload pattern.
    #[test]
    fn unknown_section_regression() {
        let (server, _tmp) = setup_dual_test_engine();
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Probe".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(IndexMap::from_iter([(
                "idntity".to_string(),
                "typo".to_string(),
            )])),
            metadata: None,
            relations: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        let env = envelope_payload(&result);
        assert_eq!(env["code"].as_str(), Some("UNKNOWN_SECTION"));
        assert!(env["details"]["declared"].is_array());
        assert_eq!(
            env["details"]["suggestion"].as_str(),
            Some("identity"),
            "strsim must point at the closest declared key"
        );
    }

    /// Regression — `UNKNOWN_METADATA_FIELD` keeps its shape.
    #[test]
    fn unknown_metadata_field_regression() {
        let (server, _tmp) = setup_dual_test_engine();
        let mut metadata = IndexMap::new();
        metadata.insert("levle".to_string(), "M0".to_string());
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Probe".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(IndexMap::from_iter([
                ("identity".to_string(), "x".to_string()),
                ("purpose".to_string(), "x".to_string()),
            ])),
            metadata: Some(metadata),
            relations: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        let env = envelope_payload(&result);
        assert_eq!(env["code"].as_str(), Some("UNKNOWN_METADATA_FIELD"));
        assert!(env["details"]["declared"].is_array());
        assert_eq!(env["details"]["suggestion"].as_str(), Some("level"));
    }
}

/// `[[wiki-link]]` patterns in section content silently created
/// stubs and a REFERENCES edge with no warning. Now surfaces as
/// `INLINE_WIKI_LINK_AUTO_STUBBED` so an agent illustrating link
/// syntax in prose notices the side-effect immediately.
mod inline_wiki_link_warning {
    use super::*;

    #[test]
    fn create_with_inline_link_to_unresolved_target_emits_warning() {
        // Under the alias model body wiki-links must be backed by
        // an atomic relation declaration; that relation auto-stubs
        // the absent target via the relate path and surfaces
        // `AUTO_STUB_CREATED` for agent review (the parser-side
        // inline-link auto-stub warning is structurally
        // unreachable now — the filter that drops aliased targets
        // empties `inline_links` before the scan runs).
        let (server, _tmp) = setup_dual_test_engine();
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "Probe.".to_string());
        sections.insert(
            "purpose".to_string(),
            "Example link form: [[ghost-target]] for documentation.".to_string(),
        );
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Inline Demo".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(sections),
            metadata: None,
            relations: Some(vec![crate::tools::mutation::RelationInput {
                rel_type: "USES".to_string(),
                target: "specs--ghost-target".to_string(),
                description: None,
            }]),
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        let payload = result
            .structured_content
            .as_ref()
            .cloned()
            .expect("dry-run response present");
        let relations_declared = payload["relations_declared"]
            .as_array()
            .cloned()
            .expect("relations_declared echoed on response");
        assert_eq!(relations_declared.len(), 1);
        assert_eq!(
            relations_declared[0]["target_was_stubbed"].as_bool(),
            Some(true),
            "absent target must be flagged as stubbed in relations_declared"
        );
    }

    /// Symmetry between `memstead_create` and `memstead_update`: both
    /// surface absent declared targets as auto-stubbed via the
    /// `relations_declared` echo (under the alias model the
    /// only auto-stub path is the explicit relation, so this is
    /// the surface to lock).
    #[test]
    fn create_and_update_emit_matching_inline_wiki_link_warnings() {
        let (server, _tmp) = setup_dual_test_engine();

        let mut create_sections = IndexMap::new();
        create_sections.insert("identity".to_string(), "Probe.".to_string());
        create_sections.insert(
            "purpose".to_string(),
            "documents [[update-ghost]] usage".to_string(),
        );
        let create_res = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Symmetry Create".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(create_sections),
            metadata: None,
            relations: Some(vec![crate::tools::mutation::RelationInput {
                rel_type: "USES".to_string(),
                target: "specs--update-ghost".to_string(),
                description: None,
            }]),
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        let create_payload = create_res
            .structured_content
            .as_ref()
            .cloned()
            .expect("create dry-run response present");
        let create_declared = create_payload["relations_declared"]
            .as_array()
            .cloned()
            .expect("create must echo relations_declared");
        assert_eq!(create_declared.len(), 1);
        assert_eq!(
            create_declared[0]["target_was_stubbed"].as_bool(),
            Some(true)
        );

        let mut update_sections = IndexMap::new();
        update_sections.insert(
            "purpose".to_string(),
            "documents [[update-ghost]] usage".to_string(),
        );
        let update_res = server.memstead_update(Parameters(UpdateParams {
            anchors: None,
            relations_unset: None,
            anchors_unset: None,
            id: "specs--entity-a".to_string(),
            expected_hash: Some(String::new()),
            sections: Some(update_sections),
            append_sections: None,
            patch_sections: None,
            sections_unset: None,
            metadata: None,
            metadata_unset: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
            declare_relations: Some(vec![crate::tools::mutation::RelationInput {
                rel_type: "USES".to_string(),
                target: "specs--update-ghost".to_string(),
                description: None,
            }]),
        }));
        assert!(
            !update_res.is_error.unwrap_or(false),
            "update dry-run must succeed: {}",
            extract_text(&update_res),
        );
        let update_payload = update_res
            .structured_content
            .as_ref()
            .cloned()
            .expect("update dry-run response present");
        let update_declared = update_payload["relations_declared"]
            .as_array()
            .cloned()
            .expect("update must echo relations_declared");
        assert_eq!(update_declared.len(), 1);
        assert_eq!(
            update_declared[0]["target_was_stubbed"].as_bool(),
            Some(true)
        );

        assert_eq!(
            create_declared[0]["target"], update_declared[0]["target"],
            "same target on both sides",
        );
    }

    /// No warning when the inline link points at an entity that
    /// already exists — only auto-CREATED stubs surface.
    #[test]
    fn create_with_inline_link_to_existing_target_omits_warning() {
        let (server, _tmp) = setup_dual_test_engine();
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "Probe.".to_string());
        sections.insert(
            "purpose".to_string(),
            "Real link: [[entity-a]] (already in store).".to_string(),
        );
        let result = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: "Real Link Demo".to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: Some(true),
            note: None,
            role: None,
            identity: None,
        }));
        let payload = result
            .structured_content
            .as_ref()
            .cloned()
            .expect("dry-run response present");
        let warnings = payload["warnings"].as_array().cloned().unwrap_or_default();
        assert!(
            !warnings
                .iter()
                .any(|w| w["code"] == "INLINE_WIKI_LINK_AUTO_STUBBED"),
            "no warning expected when the inline link resolves to an existing entity"
        );
    }
}

/// F1 (coprobe-coherence-notice-fixes): reload drift must reach the
/// agent on *error* responses too, on the same channel split a
/// success carries — the full `mem_changed` notice on
/// `structured_content`, the `MEM_RELOADED` admonition on the text
/// channel. Pre-fix the read-path 404 early-return consumed the
/// drained notice and surfaced it on neither channel, silently
/// swallowing the whole reload window; the mutation error arms
/// carried the notice on `structured_content` but never the text
/// warning line.
mod f1_drift_on_error_paths {
    use super::*;
    use memstead_base::vcs::{Actor, ClientId};
    use memstead_git_branch::test_support::init_real_mem_repo;
    use memstead_git_branch::workspace_store::engine_from_workspace_root;

    fn client() -> ClientId {
        ClientId {
            name: "sibling".to_string(),
            version: "0".to_string(),
        }
    }

    /// Create a `spec` (identity + purpose seeded) through the
    /// server, asserting success; returns the response `_hash`.
    fn create_spec(server: &McpServer, title: &str) -> String {
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "identity body".to_string());
        sections.insert("purpose".to_string(), "purpose body".to_string());
        let r = server.memstead_create(Parameters(CreateParams {
            anchors: None,
            title: title.to_string(),
            entity_type: "spec".to_string(),
            mem: Some("specs".to_string()),
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
            role: None,
            identity: None,
        }));
        assert!(
            !r.is_error.unwrap_or(false),
            "create {title}: {}",
            extract_text(&r)
        );
        let sc = r.structured_content.as_ref().expect("create envelope");
        sc["_hash"].as_str().expect("create _hash").to_string()
    }

    fn read_entity(server: &McpServer, id: &str) -> CallToolResult {
        server.memstead_entity(Parameters(EntityParams {
            id: id.to_string(),
            include_relations: None,
            include_context: None,
            sections: None,
            token_budget: None,
            chunk: None,
            include_provenance: None,
        }))
    }

    /// Wholesale-replace the `purpose` section on a sibling engine,
    /// gated on `expected_hash`.
    fn sibling_update_purpose(
        b: &mut memstead_base::Engine,
        id: &EntityId,
        expected_hash: String,
        body: &str,
    ) {
        let mut sections = indexmap::IndexMap::new();
        sections.insert("purpose".to_string(), body.to_string());
        b.update_entity(
            memstead_base::UpdateEntityArgs {
                anchors: Vec::new(),
                id: id.clone(),
                expected_hash: Some(expected_hash),
                sections,
                append_sections: indexmap::IndexMap::new(),
                patch_sections: indexmap::IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: indexmap::IndexMap::new(),
                metadata_unset: Vec::new(),
                dry_run: false,
                declare_relations: Vec::new(),
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            Actor::Cli,
            Some(&client()),
            None,
        )
        .expect("sibling update commits");
    }

    /// The notices array a response carries on `structured_content`,
    /// or `None` when the `mem_changed` key is absent.
    fn mem_changed(res: &CallToolResult) -> Option<serde_json::Value> {
        res.structured_content
            .as_ref()
            .and_then(|sc| sc.get("mem_changed").cloned())
    }

    #[test]
    fn entity_not_found_after_sibling_delete_carries_window_notice_and_warning_line() {
        let tmp = TempDir::new().unwrap();
        init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
        let engine_a = engine_from_workspace_root(tmp.path()).expect("engine A boots");
        let server = McpServer::new(engine_a, crate::config::DEFAULT_TOKEN_BUDGET);

        // A creates X, Y (touched in the window) and Z (the
        // untouched bystander for the no-leak follow-on check).
        let x_hash = create_spec(&server, "Entity X");
        let y_hash = create_spec(&server, "Entity Y");
        create_spec(&server, "Entity Z");

        // Sibling B boots at A's current head — sees X, Y, Z. In one
        // window (two commits) B modifies Y and deletes X.
        let mut b = engine_from_workspace_root(tmp.path()).expect("engine B boots");
        let x = EntityId::new("specs", "entity-x");
        let y = EntityId::new("specs", "entity-y");
        sibling_update_purpose(&mut b, &y, y_hash, "sibling-edited Y");
        b.delete_entity(
            memstead_base::DeleteEntityArgs {
                id: x.clone(),
                expected_hash: Some(x_hash),
            },
            Actor::Cli,
            Some(&client()),
            None,
        )
        .expect("sibling delete X commits");

        // A reads the now-deleted X: reloads across the whole window,
        // returns ENTITY_NOT_FOUND, and the drift must ride both
        // channels.
        let r = read_entity(&server, "specs--entity-x");
        assert_eq!(r.is_error, Some(true), "deleted entity reads as error");
        let text = extract_text(&r);
        assert!(
            text.contains("ENTITY_NOT_FOUND"),
            "text carries the code: {text}"
        );
        assert!(
            text.contains("Engine snapshot reloaded"),
            "404 text carries the MEM_RELOADED admonition: {text}",
        );

        let vc = mem_changed(&r).expect("404 carries mem_changed on structured_content");
        let notices = vc.as_array().expect("notices is an array");
        assert_eq!(notices.len(), 1, "one notice for the one reloaded mem");
        let entries = notices[0]["changes"]["entries"]
            .as_array()
            .expect("detailed notice carries entries");
        let removed_x = entries
            .iter()
            .any(|e| e["id"] == "specs--entity-x" && e["action"] == "removed");
        let modified_y = entries
            .iter()
            .any(|e| e["id"] == "specs--entity-y" && e["action"] == "updated");
        assert!(removed_x, "window notice lists X as removed: {entries:?}");
        assert!(
            modified_y,
            "window notice lists the sibling Y edit in the same window: {entries:?}",
        );

        // No-leak + quiescence: the drain on the 404 path must not
        // leak into the next op, and A's head is now current, so a
        // read of the untouched Z carries no notice.
        let z = read_entity(&server, "specs--entity-z");
        assert!(
            !z.is_error.unwrap_or(false),
            "Z reads cleanly: {}",
            extract_text(&z)
        );
        assert!(
            mem_changed(&z).is_none(),
            "quiescent follow-on read carries no notice (no leak): {:?}",
            z.structured_content,
        );
        assert!(
            !extract_text(&z).contains("Engine snapshot reloaded"),
            "quiescent follow-on read carries no drift admonition",
        );
    }

    #[test]
    fn update_hash_mismatch_carries_notice_and_warning_line() {
        let tmp = TempDir::new().unwrap();
        init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
        let engine_a = engine_from_workspace_root(tmp.path()).expect("engine A boots");
        let server = McpServer::new(engine_a, crate::config::DEFAULT_TOKEN_BUDGET);

        // A creates X and remembers its hash.
        let stale_hash = create_spec(&server, "Entity X");

        // Sibling B modifies X, advancing the head and invalidating
        // A's remembered hash.
        let mut b = engine_from_workspace_root(tmp.path()).expect("engine B boots");
        let x = EntityId::new("specs", "entity-x");
        let live_hash = b.get_entity(&x).expect("B sees X").content_hash.clone();
        sibling_update_purpose(&mut b, &x, live_hash, "sibling-edited X");

        // A updates X with the now-stale hash: the engine reloads
        // before the write (stashing the notice), then the CAS fails
        // → HASH_MISMATCH. The notice already rode structured_content
        // pre-fix; the text channel must now carry the warning line.
        let mut sections = indexmap::IndexMap::new();
        sections.insert("purpose".to_string(), "A's racing edit".to_string());
        let r = server.memstead_update(Parameters(UpdateParams {
            anchors: None,
            relations_unset: None,
            anchors_unset: None,
            id: "specs--entity-x".to_string(),
            expected_hash: Some(stale_hash),
            sections: Some(sections),
            append_sections: None,
            patch_sections: None,
            sections_unset: None,
            metadata: None,
            metadata_unset: None,
            dry_run: None,
            declare_relations: None,
            note: None,
            role: None,
            identity: None,
        }));
        assert_eq!(r.is_error, Some(true), "stale-hash write refuses");
        let text = extract_text(&r);
        assert!(
            text.contains("HASH_MISMATCH"),
            "text carries the refusal code: {text}"
        );
        assert!(
            text.contains("Engine snapshot reloaded"),
            "HASH_MISMATCH text now carries the MEM_RELOADED warning line: {text}",
        );
        let vc = mem_changed(&r).expect("HASH_MISMATCH carries mem_changed (regression guard)");
        let notices = vc.as_array().expect("notices array");
        assert_eq!(notices.len(), 1, "one notice for the collision reload");
        let entries = notices[0]["changes"]["entries"]
            .as_array()
            .expect("entries");
        assert!(
            entries
                .iter()
                .any(|e| e["id"] == "specs--entity-x" && e["action"] == "updated"),
            "notice lists the sibling X edit that caused the collision: {entries:?}",
        );
    }
}
