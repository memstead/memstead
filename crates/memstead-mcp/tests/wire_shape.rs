#![cfg(feature = "mem-repo")]
//! Wire-shape characterization for the MCP tool surface.
//!
//! This suite pins the bytes the server emits in `result.content[]` and
//! `result.structured_content` for representative tool calls. Both server
//! implementations live in this crate, gated by `mem-repo`: each
//! flavour's pin runs against its own (`FilesystemMcpServer` for the
//! lean build, `McpServer` for the full build).
//!
//! Harness drives the real `memstead-mcp` binary over stdio — same path agents
//! exercise — so the bytes captured here are the agent-visible contract.
//! Per-test spawn cost is acceptable (boot is <500ms); the harness sends
//! the full MCP handshake then multiple `tools/call` requests down one
//! pipe before tearing the child down.
//!
//! Adding a new pin:
//!   1. Pick a tool + path (success or specific error variant).
//!   2. Seed the workspace with enough state to reach that path (or
//!      reuse the empty-mounts fixture below for pure error paths).
//!   3. Call `harness.call_tool(...)`, assert on `code`, `message`
//!      contents, and `structured_content` shape.
//!   4. If the path is flavor-specific, gate on `mem-repo`.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

const WORKSPACE_TOML_BODY: &str = "format = \"memstead-git-branch-2\"\n\n\
[persistence_adapter]\nname = \"file-two-layer\"\n";

const MOUNTS_JSON_BODY_EMPTY: &str = r#"{ "format": "memstead-mounts-3", "mounts": [] }"#;

fn memstead_mcp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_memstead-mcp")
}

/// Seed a minimal workspace at `root`. No mounts — sufficient for any
/// pure-error path that doesn't depend on graph state.
fn seed_empty_workspace(root: &Path) {
    let memstead = root.join(".memstead");
    std::fs::create_dir_all(memstead.join("state")).unwrap();
    std::fs::write(memstead.join("workspace.toml"), WORKSPACE_TOML_BODY).unwrap();
    std::fs::write(
        memstead.join("state").join("mounts.json"),
        MOUNTS_JSON_BODY_EMPTY,
    )
    .unwrap();
}

/// Seed a full-flavor workspace at `root` with git-branch backed mems.
/// Each `(mem_name, schema_pin)` produces:
/// - a branch `refs/heads/<name>` and a config blob on `__SYSTEM` (via
///   `init_real_mem_repo`)
/// - a corresponding `MountStorage::GitBranch` entry in `mounts.json`
///   so the full boot path's persistence adapter sees the mem as a
///   writable mount.
///
/// Without the mounts.json entries the engine boots with zero mounts
/// even when git-branch refs exist on disk — boot doesn't auto-discover
/// mem branches; the materialisation runs out-of-band via
/// `memstead mem-repo init`. The seed shortcuts that by writing the
/// state file directly.
fn seed_full_workspace(root: &Path, mems: &[(&str, &str)]) {
    seed_full_workspace_with_toml(root, mems, WORKSPACE_TOML_BODY);
}

/// Variant of [`seed_full_workspace`] that accepts a custom
/// `workspace.toml` body. Used by tests that need `[[mem_management.*]]`
/// rules (those rules live in workspace.toml and are not state-managed).
fn seed_full_workspace_with_toml(root: &Path, mems: &[(&str, &str)], workspace_toml: &str) {
    use memstead_base::WorkspaceStoreAdapter;
    use memstead_schema::SchemaRef;

    memstead_git_branch::test_support::init_real_mem_repo(root, mems);

    let memstead = root.join(".memstead");
    std::fs::create_dir_all(memstead.join("state")).unwrap();
    std::fs::write(memstead.join("workspace.toml"), workspace_toml).unwrap();

    let gitdir = root.join("mem-repo").join(".git");
    let mounts: Vec<memstead_base::Mount> = mems
        .iter()
        .map(|(name, schema)| {
            let pin: SchemaRef = schema.parse().unwrap();
            memstead_base::Mount {
                mem: (*name).to_string(),
                schema: Some(pin),
                storage: memstead_base::MountStorage::GitBranch {
                    gitdir: gitdir.clone(),
                    branch: (*name).to_string(),
                },
                capability: memstead_base::MountCapability::Write,
                lifecycle: memstead_base::MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            }
        })
        .collect();

    let workspace = memstead_base::Workspace {
        mounts,
        settings: memstead_base::WorkspaceSettings::default(),
    };
    memstead_base::FileWorkspaceStore::new()
        .save_state(root, &workspace)
        .unwrap();
}

/// JSON-RPC harness over a spawned `memstead-mcp` child. Construct with
/// [`WireHarness::start`], drive with [`WireHarness::call_tool`], drop
/// to tear the child down.
struct WireHarness {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: i64,
}

impl WireHarness {
    /// Spawn the binary in `cwd`, send `initialize` + `notifications/initialized`.
    /// Panics on any handshake failure — these tests assume the binary
    /// boots; a regression there belongs in [`boot.rs`], not here.
    fn start(cwd: &Path) -> Self {
        Self::start_with_args(cwd, &[])
    }

    /// Spawn `memstead-mcp` with caller-supplied CLI args (e.g.
    /// `--operator-mode`) before the standard handshake.
    fn start_with_args(cwd: &Path, args: &[&str]) -> Self {
        let mut cmd = Command::new(memstead_mcp_bin());
        cmd.current_dir(cwd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .expect("spawn memstead-mcp — confirm the binary built before running tests");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let mut harness = Self {
            child: Some(child),
            stdin: Some(stdin),
            reader: BufReader::new(stdout),
            next_id: 0,
        };
        harness.handshake();
        harness
    }

    fn handshake(&mut self) {
        let id = self.send_request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "wire-shape-test", "version": "0" }
            }),
        );
        let _ = self.read_response(id, Duration::from_secs(10));
        // Spec: the client signals it's ready with this notification.
        // The server's tool surface is only legally callable after.
        self.send_notification("notifications/initialized", json!({}));
    }

    fn send_request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&body).unwrap();
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{line}").expect("write request");
        stdin.flush().expect("flush");
        id
    }

    fn send_notification(&mut self, method: &str, params: Value) {
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&body).unwrap();
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{line}").expect("write notification");
        stdin.flush().expect("flush");
    }

    fn read_response(&mut self, want_id: i64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        let mut line = String::new();
        loop {
            if Instant::now() >= deadline {
                panic!("no JSON-RPC response with id={want_id} within {timeout:?}");
            }
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => panic!("stdout EOF before id={want_id} reply"),
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let value: Value = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(_) => continue, // skip non-JSON lines (server logs leaking, etc.)
                    };
                    if value.get("id").and_then(|v| v.as_i64()) == Some(want_id) {
                        return value;
                    }
                    // Different id (e.g. server-initiated notification or
                    // out-of-order reply) — keep reading.
                }
                Err(_) => panic!("stdout read error before id={want_id} reply"),
            }
        }
    }

    /// Send `tools/call` and return the JSON-RPC `result` value (the
    /// `CallToolResult` envelope from rmcp). On JSON-RPC error replies
    /// the `error` field is returned wrapped under `_jsonrpc_error` so
    /// the caller can branch.
    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        let id = self.send_request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let response = self.read_response(id, Duration::from_secs(15));
        if let Some(err) = response.get("error") {
            return json!({ "_jsonrpc_error": err });
        }
        response
            .get("result")
            .cloned()
            .expect("tools/call response must carry `result`")
    }
}

impl Drop for WireHarness {
    fn drop(&mut self) {
        drop(self.stdin.take());
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// Lean-flavor pins (FilesystemMcpServer)
// ---------------------------------------------------------------------------
//
// Run with `cargo nextest run --no-default-features -p memstead-mcp wire_shape`.

/// Shared assertion shape: every error envelope must carry `isError=true`,
/// the expected typed `code`, and a `message` matching the per-flavor
/// pinned text. Pre-extraction the two server files own independent
/// mappers (`FilesystemMcpServer::engine_op_error` vs
/// `McpServer::engine_err_unified`) — message text DRIFTS between them
/// today (see `lean_memstead_entity_*` vs `full_memstead_entity_*`). The
/// wire-byte-identity contract is *per-flavor*, not inter-flavor, so
/// each pin records its own server's current bytes.
fn assert_error_envelope(result: &Value, expected_code: &str, expected_message: &str) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(is_error, "expected isError=true on error path: {result}");

    let structured = result
        .get("structuredContent")
        .expect("structuredContent missing — wire envelope drifted");
    let code = structured
        .get("code")
        .and_then(Value::as_str)
        .expect("structured.code missing");
    assert_eq!(
        code, expected_code,
        "code drifted; structured payload = {structured}"
    );
    let msg = structured
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        msg, expected_message,
        "message bytes drifted from pinned shape"
    );
}

// ---------------------------------------------------------------------------
// Full-flavor pins (McpServer)
// ---------------------------------------------------------------------------

/// Full pin: same input as the lean test, intentionally separate
/// assertion because the full mapper (`engine_err_unified` in
/// `server.rs`) emits a different message string than the lean mapper
/// for `ENTITY_NOT_FOUND`. These strings DIVERGE — the snapshot suite
/// captures both as today's truth until the casing is reconciled.
#[test]
fn full_memstead_entity_emits_typed_envelope_for_missing_id() {
    let tmp = TempDir::new().unwrap();
    seed_empty_workspace(tmp.path());
    // Full boot checks `<workspace>/mem-repo/.git` shape on startup —
    // seed a real bare repo with `main` + `__MEMSTEAD` refs.
    memstead_git_branch::test_support::init_real_mem_repo(tmp.path(), &[]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_entity", json!({ "id": "specs--does-not-exist" }));
    // Full mapper formats with capital "Entity not found" — diverges
    // from lean's "entity not found" (engine Display verbatim).
    // Recorded as inter-flavor drift; not fixed here.
    assert_error_envelope(
        &result,
        "ENTITY_NOT_FOUND",
        "Entity not found: specs--does-not-exist",
    );
}

// ---------------------------------------------------------------------------
// Success-path pins — pin envelope SHAPE, not exact content
// ---------------------------------------------------------------------------
//
// Success responses carry markdown content (often dependent on dynamic
// state like mem counts or schema version names). Pinning every byte
// would couple the suite to schema metadata. Instead these pins fix the
// envelope shape — `isError` absent or false, `content[0].type == text`,
// `text` carries the expected anchor sections — so a contract-shape
// regression (wrong content type, missing isError flag, structured_content
// in the wrong place) trips loudly; cosmetic prose changes do not.

fn assert_success_envelope(result: &Value) -> String {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(!is_error, "expected success but got isError=true: {result}");
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .expect("content[] missing — wire envelope drifted");
    assert!(
        !content.is_empty(),
        "content[] empty — wire envelope drifted"
    );
    let first = &content[0];
    let kind = first
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(kind, "text", "content[0].type drifted: {first}");
    first
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Full pin: same input through the full server. Full discovers mems
/// via the git-branch refs in `mem-repo/.git/`, so the seed seeds a
/// `demo` branch with the default schema pinned in `__SYSTEM`.
#[test]
fn full_memstead_search_succeeds_on_empty_seeded_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_search", json!({}));
    let text = assert_success_envelope(&result);
    for marker in ["_total: 0", "_returned: 0", "_offset: 0"] {
        assert!(
            text.contains(marker),
            "search response missing {marker:?}: {text:?}"
        );
    }
}

/// Full pin: full flavor's `memstead_overview` against the proper full seed
/// (git-branch refs + matching `mounts.json` entries) emits the
/// canonical anchors AND lists the seeded mem. Adding the full-only
/// `## Lifecycle Namespaces` anchor (the lean overview omits it
/// entirely — lean has no mem-creation rules) is part of the pin so
/// the test trips if full accidentally drops that section.
#[test]
fn full_memstead_overview_succeeds_on_empty_seeded_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_overview", json!({}));
    let text = assert_success_envelope(&result);
    for anchor in [
        "## Mems",
        "## Schemas",
        "## Communities",
        "## Lifecycle Namespaces",
    ] {
        assert!(
            text.contains(anchor),
            "full overview missing {anchor:?}: {text:?}"
        );
    }
    assert!(
        text.contains("demo"),
        "full overview missing mem name: {text:?}"
    );
}

// ---------------------------------------------------------------------------
// `memstead_schema` error pin — both flavors emit `ENTITY_NOT_FOUND` for
// names that don't match the workspace's pinned schema. Helps confirm
// the pre-extraction message divergence story applies symmetrically
// across tools, not just `memstead_entity`.
// ---------------------------------------------------------------------------

/// Full pin: same input on a full-seeded single-mem workspace. Per-flavor
/// message bytes are recorded independently; the lean flavor appends
/// `" — workspace pins default@1.0.0"` to the message, the full flavor
/// emits only `"schema not found: \"<name>\""`. Recorded drift, pending
/// reconciliation.
#[test]
fn full_memstead_schema_unknown_name_emits_entity_not_found() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_schema", json!({ "name": "not-a-schema" }));
    assert_error_envelope(
        &result,
        "ENTITY_NOT_FOUND",
        "schema not found: \"not-a-schema\"",
    );
}

/// Plan 06a criterion 1: a full-verbosity request scoped to a small
/// type selection on the measured large schema (`software@0.4.0` — the
/// 60.2 KB field spill, 2026-08-18 WOENENN ingest) returns the named
/// types' complete prose in one under-budget reply, with every
/// unserved type named in `types_omitted`. Complement: an unknown type
/// name in the selection refuses `UNKNOWN_ENTITY_TYPE` naming the
/// valid types — never a silent empty section.
#[test]
fn full_schema_type_selection_serves_named_prose_under_budget() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);
    let mut harness = WireHarness::start(tmp.path());

    let result = harness.call_tool(
        "memstead_schema",
        json!({
            "name": "software@0.4.0",
            "verbosity": "full",
            "types": ["actor", "incident"],
        }),
    );
    assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing");
    let types = body["types"].as_array().expect("scoped full carries types");
    let names: Vec<&str> = types.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, vec!["actor", "incident"], "exactly the selection");
    for t in types {
        assert!(
            t.get("writing_guidance").is_some() && t.get("system_context").is_some(),
            "selected types carry the complete prose: {t}"
        );
    }
    let omitted = body["types_omitted"]
        .as_array()
        .expect("unserved types are named, never silently dropped");
    assert_eq!(omitted.len(), 9 - 2, "the other seven types are listed");
    assert!(
        body.get("_schema_mode").is_none(),
        "a scoped reply is the steered-to shape — never re-degraded"
    );
    // The whole point: the scoped reply fits the pipe.
    let estimated = serde_json::to_string(body).unwrap().len() / 4;
    assert!(
        estimated < 15_000,
        "scoped reply must sit under the schema budget, got ~{estimated} tokens"
    );

    // Complement: unknown type name refuses typed with the roster.
    let result = harness.call_tool(
        "memstead_schema",
        json!({
            "name": "software@0.4.0",
            "verbosity": "full",
            "types": ["galaxy-brain"],
        }),
    );
    assert!(
        result["isError"].as_bool().unwrap_or(false),
        "unknown type must refuse: {result}"
    );
    let structured = &result["structuredContent"];
    assert_eq!(structured["code"], "UNKNOWN_ENTITY_TYPE");
    assert_eq!(structured["details"]["unknown"], json!(["galaxy-brain"]));
    assert!(
        structured["details"]["known_types"]
            .as_array()
            .is_some_and(|k| k.iter().any(|v| v == "actor")),
        "refusal names the valid types: {structured}"
    );
}

/// Plan 06a criterion 2: the unscoped full reply on the measured large
/// schema degrades VISIBLY per the budget pattern — reduced mode
/// stamped, hint steering to per-type retrieval, full roster in
/// `types_omitted` — no silent truncation and no reliance on harness
/// file-spill. Complements: an unscoped full on a schema that fits
/// (`default@1.0.0`, ~52 KB — today's working size) still serves the
/// whole prose untouched, and the default lite reply carries none of
/// the new keys (byte-compatible for existing consumers).
#[test]
fn full_schema_unscoped_over_budget_degrades_visibly() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);
    let mut harness = WireHarness::start(tmp.path());

    let result = harness.call_tool(
        "memstead_schema",
        json!({ "name": "software@0.4.0", "verbosity": "full" }),
    );
    assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing");
    assert_eq!(body["_schema_mode"], "reduced", "degrade is stamped");
    assert!(
        body["_hint"].as_str().is_some_and(|h| h.contains("types")),
        "hint steers to per-type retrieval: {}",
        body["_hint"]
    );
    assert!(body.get("types").is_none(), "over-budget prose not shipped");
    assert!(
        body["types_summary"]
            .as_array()
            .is_some_and(|t| t.len() == 9),
        "the lite skeleton still covers the whole roster"
    );
    assert_eq!(
        body["types_omitted"].as_array().map(|v| v.len()),
        Some(9),
        "everything unserved is named"
    );

    // Complement 1: a fitting schema's unscoped full is untouched.
    let result = harness.call_tool(
        "memstead_schema",
        json!({ "name": "default@1.0.0", "verbosity": "full" }),
    );
    assert_success_envelope(&result);
    let body = &result["structuredContent"];
    assert!(body["types"].is_array(), "fitting full stays full");
    assert!(body.get("_schema_mode").is_none());
    assert!(body.get("types_omitted").is_none());

    // Complement 2: the default lite reply carries none of the new keys.
    let result = harness.call_tool("memstead_schema", json!({ "name": "default@1.0.0" }));
    assert_success_envelope(&result);
    let body = &result["structuredContent"];
    assert!(body["types_summary"].is_array());
    for k in ["types", "types_omitted", "_schema_mode", "_hint"] {
        assert!(
            body.get(k).is_none(),
            "lite reply must stay byte-compatible — unexpected key {k:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Mutation pins — `memstead_create` success + UNKNOWN_ENTITY_TYPE error
// ---------------------------------------------------------------------------
//
// Success path: the create response carries a JSON body on
// `structured_content` whose `id` field is the slugified id, plus
// `title`, `mem`, `content_hash`, `commit_sha`, and `warnings`. The
// pins assert on field PRESENCE + the deterministic `id` slug; the
// hashes / commit shas are content-derived and pinning them would
// couple the suite to the markdown render exactly.

fn assert_create_success_shape(result: &Value, expected_id: &str, expected_mem: &str) {
    let _text = assert_success_envelope(result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on create success");
    for field in ["id", "title", "mem", "_hash", "warnings"] {
        assert!(
            body.get(field).is_some(),
            "create response missing {field:?}: {body}"
        );
    }
    assert_eq!(
        body.get("id").and_then(Value::as_str),
        Some(expected_id),
        "create id drifted from slug rule: {body}"
    );
    assert_eq!(
        body.get("mem").and_then(Value::as_str),
        Some(expected_mem),
        "create response mem drifted: {body}"
    );
}

/// Full pin: same as lean. The slug rule (`<mem>--<lower-kebab>`) is
/// engine-internal so the expected id matches the lean pin.
#[test]
fn full_memstead_create_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_create",
        json!({
            "title": "First",
            "entity_type": "spec",
            "sections": { "identity": "the identity", "purpose": "the purpose" },
        }),
    );
    assert_create_success_shape(&result, "demo--first", "demo");
}

/// Full pin: same input. Pre-extraction the full mapper
/// (`engine_err_unified`) also wraps `UNKNOWN_ENTITY_TYPE`; this pin
/// trips if full drops the recovery payload during the lift.
#[test]
fn full_memstead_create_unknown_type_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_create",
        json!({ "title": "X", "entity_type": "totally-not-a-type" }),
    );

    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(is_error, "expected isError on unknown type: {result}");
    let structured = result
        .get("structuredContent")
        .expect("structuredContent missing");
    assert_eq!(
        structured.get("code").and_then(Value::as_str),
        Some("UNKNOWN_ENTITY_TYPE"),
        "code drifted: {structured}"
    );
    let msg = structured
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        msg.contains("totally-not-a-type"),
        "message missing rejected type name: {msg:?}"
    );
    assert!(
        msg.contains("Declared types:") || msg.contains("declared types:"),
        "message missing declared-types prefix: {msg:?}"
    );
}

// ---------------------------------------------------------------------------
// `memstead_health` success pins
// ---------------------------------------------------------------------------

/// Full pin: full `memstead_health` returns a richer envelope with
/// `writable_mems` populated when the engine sees writable mounts.
#[test]
fn full_memstead_health_succeeds_on_seeded_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_health", json!({}));
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on health success");
    assert!(
        body.get("writable_mems").is_some(),
        "full health response missing writable_mems: {body}"
    );
}

/// A bad
/// `since` cursor on `memstead_changes_since` returns the typed `INVALID_CURSOR`
/// — not the `MEM_ERROR` catch-all — with the offending SHA untruncated
/// in `details.since`, so a sync loop branches cleanly (typed → re-seed).
#[test]
fn full_memstead_changes_since_bad_cursor_returns_invalid_cursor() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);
    let mut harness = WireHarness::start(tmp.path());

    let bad = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let result = harness.call_tool(
        "memstead_changes_since",
        json!({ "mem": "demo", "since": bad }),
    );
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(is_error, "a bad since cursor must error: {result}");
    let sc = result
        .get("structuredContent")
        .expect("structuredContent missing on error envelope");
    assert_eq!(
        sc.get("code").and_then(Value::as_str),
        Some("INVALID_CURSOR"),
        "bad since must carry the typed INVALID_CURSOR code, not MEM_ERROR: {sc}",
    );
    assert_eq!(
        sc.get("details")
            .and_then(|d| d.get("since"))
            .and_then(Value::as_str),
        Some(bad),
        "the offending SHA must ride untruncated in details.since: {sc}",
    );
}

/// The default writable
/// mem is stable. After the seed mem `demo`, creating a second
/// writable mem `aaa` (which sorts ahead alphabetically) must NOT
/// retarget omitted-`mem` writes — a subsequent `memstead_create` with
/// `mem` omitted still lands in `demo`. The default is discoverable
/// on `memstead_health.default_writable_mem`, and an explicit `mem`
/// always wins. Pre-fix the resolver read `writable_mems().iter().next()`
/// off an unordered `HashSet`, so the second mem silently retargeted
/// the default.
#[test]
fn full_default_writable_mem_is_stable_after_second_mem() {
    const TOML: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
\n\
[[mem_management.create]]\n\
pattern = \"*\"\n\
schemas = [\"default@1.0.0\"]\n\
";
    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(tmp.path(), &[("demo", "default@1.0.0")], TOML);
    let mut harness = WireHarness::start(tmp.path());

    let sections = json!({ "identity": "the identity", "purpose": "the purpose" });

    // Baseline: an omitted-`mem` create lands in the seed `demo`.
    let c1 = harness.call_tool(
        "memstead_create",
        json!({ "title": "First", "entity_type": "spec", "sections": sections }),
    );
    assert_create_success_shape(&c1, "demo--first", "demo");

    // Bring up a second writable mem whose name sorts ahead of `demo`.
    let cv = harness.call_tool(
        "memstead_mem_create",
        json!({ "name": "aaa", "location": "mems/aaa", "schema": "default@1.0.0" }),
    );
    let _ = assert_success_envelope(&cv);

    // The omitted-`mem` create STILL lands in `demo`, not `aaa` —
    // adding a mem did not move the default.
    let c2 = harness.call_tool(
        "memstead_create",
        json!({ "title": "Second", "entity_type": "spec", "sections": sections }),
    );
    assert_create_success_shape(&c2, "demo--second", "demo");

    // The default is discoverable on the read surface.
    let health = harness.call_tool("memstead_health", json!({}));
    let hbody = health
        .get("structuredContent")
        .expect("structuredContent missing on health success");
    assert_eq!(
        hbody.get("default_writable_mem").and_then(Value::as_str),
        Some("demo"),
        "memstead_health must name the stable default: {hbody}",
    );

    // Explicit `mem` always wins, regardless of the default.
    let c3 = harness.call_tool(
        "memstead_create",
        json!({ "mem": "aaa", "title": "Third", "entity_type": "spec", "sections": sections }),
    );
    assert_create_success_shape(&c3, "aaa--third", "aaa");
}

// ---------------------------------------------------------------------------
// Multi-step mutation pins — exercise the read-then-write contract
// ---------------------------------------------------------------------------
//
// The optimistic-locking contract is central to safe mutations: every
// `memstead_update` / `memstead_delete` / `memstead_rename` requires `expected_hash`
// from a prior read, and a stale hash trips `HASH_MISMATCH` with the
// current on-disk hash on `details.current`. These pins exercise the
// full read-then-write loop through the wire.

/// Issue an `memstead_create` call and return `(id, content_hash)` so a
/// subsequent mutation can target it with the right `expected_hash`.
/// Panics on any create failure — used as a fixture by mutation tests.
///
/// The engine
/// refuses on missing required sections, so the helper seeds the
/// `spec` type's required `identity` + `purpose` sections.
fn create_and_get_id_hash(harness: &mut WireHarness, title: &str) -> (String, String) {
    let result = harness.call_tool(
        "memstead_create",
        json!({
            "title": title,
            "entity_type": "spec",
            "sections": {
                "identity": "seed identity",
                "purpose": "seed purpose",
            },
        }),
    );
    let body = result
        .get("structuredContent")
        .expect("create response missing structuredContent");
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .expect("create response missing id")
        .to_string();
    let hash = body
        .get("_hash")
        .and_then(Value::as_str)
        .expect("create response missing content_hash")
        .to_string();
    (id, hash)
}

/// Shared assertion for HASH_MISMATCH envelopes. `details.current` must
/// carry the actual on-disk hash; `details.id` must echo the rejected id.
/// `details.is_stub` indicates whether the entity is a stub (no body) —
/// pinned so callers know to branch on it for stub-aware recovery.
fn assert_hash_mismatch_envelope(result: &Value, expected_id: &str, expected_current: &str) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(is_error, "expected isError=true on stale hash: {result}");
    let structured = result
        .get("structuredContent")
        .expect("structuredContent missing");
    assert_eq!(
        structured.get("code").and_then(Value::as_str),
        Some("HASH_MISMATCH"),
        "code drifted: {structured}"
    );
    let details = structured
        .get("details")
        .expect("HASH_MISMATCH must carry details");
    assert_eq!(
        details.get("id").and_then(Value::as_str),
        Some(expected_id),
        "details.id drifted: {details}"
    );
    assert_eq!(
        details.get("current").and_then(Value::as_str),
        Some(expected_current),
        "details.current drifted: {details}"
    );
    assert!(
        details.get("is_stub").is_some(),
        "details.is_stub missing — recovery payload contract drifted: {details}"
    );
}

/// Full pin: same multi-step flow exercises full's mapper.
#[test]
fn full_memstead_update_stale_hash_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (id, real_hash) = create_and_get_id_hash(&mut harness, "Locked");

    let stale_hash = "0".repeat(64);
    let result = harness.call_tool(
        "memstead_update",
        json!({
            "id": id,
            "expected_hash": stale_hash,
            "sections": { "identity": "new body" },
        }),
    );
    assert_hash_mismatch_envelope(&result, &id, &real_hash);
}

/// Full pin: same. Full response shape may differ subtly (extra fields
/// like commit_sha) — the pin only requires the rotated hash.
#[test]
fn full_memstead_update_succeeds_and_rotates_hash() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (id, original_hash) = create_and_get_id_hash(&mut harness, "Updatable");

    let result = harness.call_tool(
        "memstead_update",
        json!({
            "id": id,
            "expected_hash": original_hash,
            "sections": { "identity": "rewritten body" },
        }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on update success");
    let new_hash = body
        .get("_hash")
        .and_then(Value::as_str)
        .expect("update response missing content_hash");
    assert_ne!(
        new_hash, original_hash,
        "content_hash did not rotate after section rewrite: {body}"
    );
}

/// Full pin: same flow; full's ENTITY_NOT_FOUND message text uses
/// capital "Entity" per the previously-recorded inter-flavor drift.
#[test]
fn full_memstead_delete_succeeds_and_entity_becomes_unreadable() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (id, hash) = create_and_get_id_hash(&mut harness, "Doomed");

    let del = harness.call_tool(
        "memstead_delete",
        json!({ "id": id, "expected_hash": hash }),
    );
    let _ = assert_success_envelope(&del);

    let read = harness.call_tool("memstead_entity", json!({ "id": id }));
    assert_error_envelope(
        &read,
        "ENTITY_NOT_FOUND",
        &format!("Entity not found: {id}"),
    );
}

// ---------------------------------------------------------------------------
// `memstead_relate` success pins
// ---------------------------------------------------------------------------

/// Full pin: same flow, but the response field names differ from lean:
/// full emits `rel_type` (not `type`), `source: "explicit"` (carries the
/// edge source), `_mem_schema`, and `commit_sha` — but **omits**
/// `action`. The lean surface has `type` and `action` instead. Both
/// shapes are pinned per-flavor, pending reconciliation of which schema
/// wins.
#[test]
fn full_memstead_relate_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (from, _) = create_and_get_id_hash(&mut harness, "Source");
    let (to, _) = create_and_get_id_hash(&mut harness, "Target");

    let result = harness.call_tool(
        "memstead_relate",
        json!({ "relations": [{ "from": from, "to": to, "type": "USES" }] }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on relate success");
    let entry = body
        .get("results")
        .and_then(|r| r.get(0))
        .expect("plural envelope carries results[0]");
    assert_eq!(
        entry.get("from").and_then(Value::as_str),
        Some(from.as_str()),
        "relate `from` drifted: {body}"
    );
    assert_eq!(
        entry.get("to").and_then(Value::as_str),
        Some(to.as_str()),
        "relate `to` drifted: {body}"
    );
    // Full uses `rel_type` (not `type`). USES (not REFERENCES) — explicit
    // author of REFERENCES is refused under the default schema's
    // `alias_target_rel_type` pointer; this test pins the envelope
    // shape, not the rel-type specifically.
    assert_eq!(
        entry.get("rel_type").and_then(Value::as_str),
        Some("USES"),
        "full relate `rel_type` drifted: {body}"
    );
    assert!(
        body.get("type").is_none(),
        "full must not carry `type` (lean field name): {body}"
    );
    // `action` rides the per-entry result, not the top level — the same
    // place the lean surface puts it. (This assertion once recorded
    // "full omits `action`, lean carries it"; that drift closed with the
    // plural relate envelope, and the twin pin in `wire_shape_lean.rs`
    // now asserts the matching per-entry shape.)
    assert_eq!(
        entry.get("action").and_then(Value::as_str),
        Some("added"),
        "full relate `action` drifted: {body}"
    );
    assert!(
        body.get("action").is_none(),
        "`action` belongs inside results[], never at the top level: {body}"
    );
}

// ---------------------------------------------------------------------------
// `memstead_rename` pins — success + RENAME_NO_OP
// ---------------------------------------------------------------------------

/// Full pin: same flow.
#[test]
fn full_memstead_rename_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (id, hash) = create_and_get_id_hash(&mut harness, "Old Title");

    let result = harness.call_tool(
        "memstead_rename",
        json!({ "id": id, "new_title": "New Title", "expected_hash": hash }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on rename success");
    assert_eq!(
        body.get("old_id").and_then(Value::as_str),
        Some(id.as_str()),
        "old_id drifted: {body}"
    );
    assert_eq!(
        body.get("new_id").and_then(Value::as_str),
        Some("demo--new-title"),
        "new_id drifted from slug rule: {body}"
    );
}

/// Full pin: full renames-to-same-slug succeed but ride a typed
/// `TITLE_NORMALIZED_TO_SLUG_NOOP` warning on the response so an agent
/// can detect the degenerate case from `details.warnings[]`. The lean
/// surface omits the warning entirely (see the lean pin above).
#[test]
fn full_memstead_rename_same_slug_emits_typed_warning() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (id, hash) = create_and_get_id_hash(&mut harness, "First");

    let result = harness.call_tool(
        "memstead_rename",
        json!({ "id": id, "new_title": "First", "expected_hash": hash }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing");
    assert_eq!(
        body.get("old_id").and_then(Value::as_str),
        Some(id.as_str()),
    );
    assert_eq!(
        body.get("new_id").and_then(Value::as_str),
        Some(id.as_str()),
    );
    let warnings = body
        .get("warnings")
        .and_then(Value::as_array)
        .expect("full rename success must carry warnings[]");
    let codes: Vec<&str> = warnings
        .iter()
        .filter_map(|w| w.get("code").and_then(Value::as_str))
        .collect();
    assert!(
        codes.contains(&"TITLE_NORMALIZED_TO_SLUG_NOOP"),
        "expected TITLE_NORMALIZED_TO_SLUG_NOOP warning, got codes={codes:?}: {body}"
    );
}

// ---------------------------------------------------------------------------
// `memstead_reload` (full-only) success pin
// ---------------------------------------------------------------------------
//
// The lean filesystem-mem server doesn't expose memstead_reload —
// drift-reload is a mem-repo concept (sibling writer commits a new
// HEAD; engine re-derives memo state). Pinning is full-only.

/// Full pin: `memstead_reload` on a quiescent workspace returns a success
/// envelope. The detailed report shape (changes count, etc.) is
/// engine-state-dependent; the pin is on the envelope's success flag
/// and presence of the report on `structured_content`.
#[test]
fn full_memstead_reload_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_reload", json!({}));
    let _ = assert_success_envelope(&result);
    assert!(
        result.get("structuredContent").is_some(),
        "reload response missing structuredContent: {result}"
    );
}

// ---------------------------------------------------------------------------
// `memstead_delete` HAS_INCOMING_REFS pin — multi-step (create×2 → relate → delete)
// ---------------------------------------------------------------------------
//
// The recovery payload contract: `details.referrers[]` carries
// `{from_id, rel_type, mem, capability: "write"}` for each Write-Mem
// referrer so the agent can rewrite the offending references without a
// follow-up `memstead_entity` call. Both flavors emit this shape today.

fn assert_has_incoming_refs_envelope(result: &Value, expected_target: &str, expected_source: &str) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(
        is_error,
        "expected isError on delete with referrers: {result}"
    );
    let structured = result
        .get("structuredContent")
        .expect("structuredContent missing");
    assert_eq!(
        structured.get("code").and_then(Value::as_str),
        Some("HAS_INCOMING_REFS"),
        "code drifted: {structured}"
    );
    let details = structured
        .get("details")
        .expect("details missing on HAS_INCOMING_REFS");
    assert_eq!(
        details.get("id").and_then(Value::as_str),
        Some(expected_target),
        "details.id drifted: {details}"
    );
    let referrers = details
        .get("referrers")
        .and_then(Value::as_array)
        .expect("details.referrers[] missing");
    assert!(
        !referrers.is_empty(),
        "details.referrers[] is empty: {details}"
    );
    let first = &referrers[0];
    assert_eq!(
        first.get("from_id").and_then(Value::as_str),
        Some(expected_source),
        "referrer.from_id drifted: {first}"
    );
    assert_eq!(
        first.get("capability").and_then(Value::as_str),
        Some("write"),
        "referrer.capability drifted: {first}"
    );
    let rel_types = first
        .get("rel_types")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("referrer.rel_types missing: {first}"));
    assert!(
        !rel_types.is_empty(),
        "referrer.rel_types must carry ≥1 entry: {first}"
    );
    assert!(
        first.get("mem").and_then(Value::as_str).is_some(),
        "referrer.mem missing: {first}"
    );
}

/// Full pin: same multi-step flow.
#[test]
fn full_memstead_delete_with_incoming_refs_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, _) = create_and_get_id_hash(&mut harness, "Referrer");
    let (target, target_hash) = create_and_get_id_hash(&mut harness, "Referenced");

    let relate = harness.call_tool(
        "memstead_relate",
        json!({ "relations": [{ "from": source, "to": target, "type": "USES" }] }),
    );
    let _ = assert_success_envelope(&relate);

    let del = harness.call_tool(
        "memstead_delete",
        json!({ "id": target, "expected_hash": target_hash }),
    );
    assert_has_incoming_refs_envelope(&del, &target, &source);
}

// ---------------------------------------------------------------------------
// `memstead_changes_since` success pins
// ---------------------------------------------------------------------------
//
// Lean and full use STRUCTURALLY different change-feeds: lean reads
// timestamp-keyed entries from `.memstead/changes.jsonl`; full reads git
// commits between `since` and HEAD. The two response envelopes
// diverge — each pin records its flavor's shape per-flavor.

/// Full pin: `memstead_changes_since` reads git history. Passing the
/// canonical empty-tree SHA returns every entity as `added`. The
/// response carries a richer envelope (`changes[]`, head_sha,
/// changed_files counts) compared to lean's flat `{since, count,
/// entries}` shape. **Drift recorded** — neither shape is canonical
/// yet.
#[test]
fn full_memstead_changes_since_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let _ = create_and_get_id_hash(&mut harness, "First");

    // Canonical git empty-tree SHA → "give me every entity as added".
    let empty_tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let result = harness.call_tool(
        "memstead_changes_since",
        json!({ "mem": "demo", "since": empty_tree }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on changes_since success");
    // Full's response shape is distinct from lean — pin presence of
    // `changes` (the per-entity event list on full) rather than lean's
    // `entries`. The exact richer fields (head_sha, etc.) are not
    // pinned here so the envelope can evolve under non-extraction
    // plans without tripping this test; the lift cannot drop
    // `changes[]` though.
    assert!(
        body.get("changes").is_some(),
        "full changes_since response missing `changes[]`: {body}"
    );
    // Lean-style `entries[]` must NOT appear on full — these are
    // distinct envelopes today.
    assert!(
        body.get("entries").is_none(),
        "full response unexpectedly carries lean's `entries[]`: {body}"
    );
}

/// Engine-tier rename
/// detection via commit notes. Relying on
/// gix's content-similarity scorer alone, over wide cursor
/// windows, pairs unrelated entities X↔Y if their content happens to
/// be more similar than the actual rename pair X↔Z — a memo rename
/// followed by adjacent unrelated commits reproduces this.
///
/// Instead the engine walks `agent_notes_since` first and uses
/// the authoritative `memstead: rename A → B` map to override gix's
/// pairing. Reproducer: rename one entity, make several unrelated
/// commits, poll `changes_since` over the wide cursor window
/// (empty-tree → HEAD). Exactly one `renamed` event with the
/// correct from/to pair must surface, regardless of any
/// content-similarity coincidences across the other commits.
#[test]
fn full_memstead_changes_since_wide_window_uses_authoritative_rename_map() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());

    // Step 1: seed the workspace with all the entities that exist
    // BEFORE the cursor — pre-rename adjacent entities and the
    // rename target. Then capture the cursor SHA. Anything that
    // happens after this is "inside the polling window".
    let (rename_id, _) = create_and_get_id_hash(&mut harness, "Leading And Trailing Whitespace");
    // Adjacent unrelated entities — bodies share enough lexical
    // mass with the rename target that gix's similarity scorer can
    // mispair them over the wide window (the F16 trip).
    let (other_a, _) = create_and_get_id_hash(&mut harness, "Adjacent Memo Alpha");
    let (other_b, _) = create_and_get_id_hash(&mut harness, "Adjacent Memo Beta");

    // Re-read the rename target so we have a fresh hash for the
    // rename call (the post-create hash, which is still current
    // because nothing has touched the target since).
    let entity_read = harness.call_tool("memstead_entity", json!({ "id": rename_id }));
    let entity_text = assert_success_envelope(&entity_read);
    // Extract `_hash` from the markdown frontmatter — wire-shape
    // helper isn't worth threading; a substring sniff is enough.
    let pre_hash = entity_text
        .lines()
        .find_map(|l| l.strip_prefix("_hash: "))
        .map(|s| s.trim_matches('"').to_string())
        .expect("entity text must carry _hash");

    // Step 2: capture cursor SHA by recording the most recent
    // create's commit_sha — that's the workspace head right after
    // the last seed entity landed, so it's the boundary between
    // "pre-window" and "in-window" commits. The agent contract
    // is to keep `commit_sha` from every mutation response and pass
    // it back as `since` for the next poll.
    let last_seed_create = harness.call_tool("memstead_entity", json!({ "id": other_b }));
    let _ = assert_success_envelope(&last_seed_create);
    // Use memstead_changes_since with empty-tree to find the latest
    // commit's SHA at the current HEAD — the response carries `head`.
    let cursor_capture = harness.call_tool(
        "memstead_changes_since",
        json!({
            "mem": "demo",
            "since": "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
        }),
    );
    let _ = assert_success_envelope(&cursor_capture);
    let head_sha = cursor_capture
        .get("structuredContent")
        .and_then(|c| c.get("head"))
        .and_then(Value::as_str)
        .expect("changes_since must echo head for cursor capture")
        .to_string();

    // Step 3: inside the polling window — touch unrelated entities
    // (so the diff has Update events) and rename the target.
    let entity_a_read = harness.call_tool("memstead_entity", json!({ "id": other_a }));
    let a_text = assert_success_envelope(&entity_a_read);
    let other_a_hash = a_text
        .lines()
        .find_map(|l| l.strip_prefix("_hash: "))
        .map(|s| s.trim_matches('"').to_string())
        .expect("entity_a missing _hash");
    let update_a = harness.call_tool(
        "memstead_update",
        json!({
            "id": other_a,
            "expected_hash": other_a_hash,
            "sections": {
                "identity": "Some adjacent content overlapping with the rename target.",
            },
        }),
    );
    let _ = assert_success_envelope(&update_a);

    // Now rename the target.
    let renamed = harness.call_tool(
        "memstead_rename",
        json!({
            "id": rename_id,
            "new_title": "Whitespace Memo Renamed",
            "expected_hash": pre_hash,
        }),
    );
    let renamed_body = renamed
        .get("structuredContent")
        .expect("rename response missing body");
    let new_id = renamed_body
        .get("new_id")
        .and_then(Value::as_str)
        .expect("rename response missing new_id")
        .to_string();

    // Step 4: changes_since from the captured cursor.
    let feed = harness.call_tool(
        "memstead_changes_since",
        json!({ "mem": "demo", "since": head_sha }),
    );
    let _ = assert_success_envelope(&feed);
    let body = feed
        .get("structuredContent")
        .expect("changes_since missing structuredContent");
    let changes = body
        .get("changes")
        .and_then(Value::as_array)
        .expect("changes_since missing changes[]");

    // Exactly one Renamed event with the right pair. Other actions
    // (`updated` on adjacents) may also surface — the pin is "no
    // false-positive renames coming from gix-similarity scoring".
    let renames: Vec<&Value> = changes
        .iter()
        .filter(|ev| ev.get("action").and_then(Value::as_str) == Some("renamed"))
        .collect();
    assert_eq!(
        renames.len(),
        1,
        "wide-window changes_since must surface exactly one renamed event; \
         got {}. changes={:#?}",
        renames.len(),
        changes,
    );
    let only_rename = renames[0];
    assert_eq!(
        only_rename.get("from_id").and_then(Value::as_str),
        Some(rename_id.as_str()),
        "renamed.from_id drifted: {only_rename}",
    );
    assert_eq!(
        only_rename.get("to_id").and_then(Value::as_str),
        Some(new_id.as_str()),
        "renamed.to_id drifted: {only_rename}",
    );

    // Unrelated entities must NOT surface as `renamed` (the F16
    // class of false-positive). Their action should be `updated`
    // (other_a was updated, other_b was untouched and so doesn't
    // appear at all).
    for ev in changes {
        let action = ev.get("action").and_then(Value::as_str).unwrap_or_default();
        if action == "renamed" {
            continue;
        }
        let id = ev.get("id").and_then(Value::as_str).unwrap_or_default();
        let from_id = ev
            .get("from_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let to_id = ev.get("to_id").and_then(Value::as_str).unwrap_or_default();
        assert_ne!(id, other_b.as_str(), "other_b mispaired: {ev}");
        assert_ne!(from_id, other_b.as_str(), "other_b as rename source: {ev}");
        assert_ne!(to_id, other_b.as_str(), "other_b as rename target: {ev}");
    }
}

/// `include_notes: false` strips notes + memstead_ref from the
/// wire response even though the engine populates them
/// unconditionally — the parameter is renderer-side filtering, not
/// an engine-side trigger.
#[test]
fn full_memstead_changes_since_include_notes_false_strips_notes_and_memstead_ref() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let _ = create_and_get_id_hash(&mut harness, "Noteless");

    let empty_tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let result = harness.call_tool(
        "memstead_changes_since",
        json!({ "mem": "demo", "since": empty_tree, "include_notes": false }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing");
    assert!(
        body.get("notes").is_none(),
        "include_notes: false must strip notes[] from the wire: {body}",
    );
    assert!(
        body.get("memstead_ref").is_none(),
        "include_notes: false must strip memstead_ref from the wire: {body}",
    );
}

/// `memstead_entity` ships
/// rendered markdown on the text channel and the structured
/// envelope on `structured_content`. With an empty structured
/// channel, agents wanting `_hash`, sections, or
/// relations would parse the text-channel markdown by string-scraping.
#[test]
fn full_memstead_entity_returns_structured_envelope_alongside_markdown() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (id, hash) = create_and_get_id_hash(&mut harness, "Structured Subject");

    let result = harness.call_tool("memstead_entity", json!({ "id": id }));
    let _ = assert_success_envelope(&result);

    // Text channel: rendered markdown — preserved for terminal /
    // prose consumers.
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .expect("entity response missing text-channel markdown");
    assert!(
        text.contains("# Structured Subject"),
        "text channel must carry rendered markdown: {text}",
    );

    // Structured channel: typed envelope — agents branch on fields
    // without parsing the text channel.
    let body = result
        .get("structuredContent")
        .expect("memstead_entity must populate structured_content");
    assert_eq!(
        body.get("_hash").and_then(Value::as_str),
        Some(hash.as_str()),
        "structured._hash must match the create response's content_hash: {body}",
    );
    assert_eq!(body.get("id").and_then(Value::as_str), Some(id.as_str()),);
    assert_eq!(body.get("mem").and_then(Value::as_str), Some("demo"),);
    assert_eq!(
        body.get("type").and_then(Value::as_str),
        Some("spec"),
        "structured.type drifted: {body}",
    );
    assert!(
        body.get("sections").and_then(Value::as_object).is_some(),
        "structured.sections must be a JSON object: {body}",
    );
    assert!(
        body.get("relationships")
            .and_then(Value::as_array)
            .is_some(),
        "structured.relationships must be a JSON array: {body}",
    );
    assert!(
        body.get("_tokens").and_then(Value::as_u64).is_some(),
        "structured._tokens must be a non-negative integer: {body}",
    );
}

/// `memstead_search` ships
/// rendered markdown on the text channel and the structured
/// `SearchResultEnvelope` on `structured_content`. Without it,
/// agents would have to parse the markdown prose to recover scores,
/// score breakdowns, or facet counts.
#[test]
fn full_memstead_search_returns_structured_envelope_alongside_markdown() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let _ = create_and_get_id_hash(&mut harness, "Authorization Flow");
    let _ = create_and_get_id_hash(&mut harness, "Anchor Memo");

    let result = harness.call_tool("memstead_search", json!({ "query": { "any": ["Anchor"] } }));
    let _ = assert_success_envelope(&result);

    // Text channel — rendered markdown (rendered prose with scores,
    // headings, etc.).
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .expect("search response missing text-channel markdown");
    assert!(
        text.contains("_total:"),
        "text channel must carry rendered markdown frontmatter: {text}",
    );

    // Structured channel — _-prefixed counters at the top level,
    // hits[] with the per-hit shape (score and friends).
    let body = result
        .get("structuredContent")
        .expect("memstead_search must populate structured_content");
    assert!(
        body.get("_total").and_then(Value::as_u64).is_some(),
        "structured._total must be present: {body}",
    );
    assert!(
        body.get("_returned").and_then(Value::as_u64).is_some(),
        "structured._returned must be present: {body}",
    );
    assert!(
        body.get("_offset").and_then(Value::as_u64).is_some(),
        "structured._offset must be present: {body}",
    );
    assert!(
        body.get("_total_tokens").and_then(Value::as_u64).is_some(),
        "structured._total_tokens must be present: {body}",
    );
    let hits = body
        .get("hits")
        .and_then(Value::as_array)
        .expect("structured.hits must be an array");
    assert!(!hits.is_empty(), "expected ≥1 hit: {body}");
    let hit = &hits[0];
    assert!(
        hit.get("id").and_then(Value::as_str).is_some(),
        "hit.id missing: {hit}",
    );
    assert!(
        hit.get("score").and_then(Value::as_f64).is_some(),
        "hit.score must be a float (no precision loss vs engine f32): {hit}",
    );
}

/// `relationships` carry typed shape — `rel_type`, `target`,
/// `source: explicit`, plus optional `description` per posture.
#[test]
fn full_memstead_entity_structured_relationships_carry_typed_shape() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (from, _) = create_and_get_id_hash(&mut harness, "Rel Source");
    let (to, _) = create_and_get_id_hash(&mut harness, "Rel Target");
    let _ = harness.call_tool(
        "memstead_relate",
        json!({ "relations": [{ "from": from, "to": to, "type": "PART_OF" }] }),
    );

    let result = harness.call_tool("memstead_entity", json!({ "id": from }));
    let body = result
        .get("structuredContent")
        .expect("missing structured_content");
    let relationships = body
        .get("relationships")
        .and_then(Value::as_array)
        .expect("structured.relationships must be an array");
    assert!(
        !relationships.is_empty(),
        "expected ≥1 relationship after relate: {body}",
    );
    let rel = &relationships[0];
    assert_eq!(rel.get("rel_type").and_then(Value::as_str), Some("PART_OF"),);
    assert_eq!(rel.get("target").and_then(Value::as_str), Some(to.as_str()),);
    assert_eq!(
        rel.get("source").and_then(Value::as_str),
        Some("explicit"),
        "structured.relationships[].source pinned to `explicit`: {rel}",
    );
}

/// `include_notes: true` carries the per-commit feed. The
/// rename note must surface alongside the renamed change event,
/// proving the engine populates both from the same walk.
#[test]
fn full_memstead_changes_since_include_notes_true_carries_notes_and_rename_note() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (id, hash) = create_and_get_id_hash(&mut harness, "Renaming Subject");
    let renamed = harness.call_tool(
        "memstead_rename",
        json!({
            "id": id,
            "new_title": "After Rename",
            "expected_hash": hash,
        }),
    );
    let _ = assert_success_envelope(&renamed);

    let empty_tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let feed = harness.call_tool(
        "memstead_changes_since",
        json!({ "mem": "demo", "since": empty_tree, "include_notes": true }),
    );
    let _ = assert_success_envelope(&feed);
    let body = feed
        .get("structuredContent")
        .expect("structuredContent missing");
    let notes = body
        .get("notes")
        .and_then(Value::as_array)
        .expect("include_notes: true must surface notes[]");
    assert!(
        notes
            .iter()
            .any(|n| { n.get("tool_verb").and_then(Value::as_str) == Some("rename") }),
        "rename note missing from notes[]: {body}",
    );
}

// ---------------------------------------------------------------------------
// Stub-family pins — auto-stub create + STUB_NOT_UPDATABLE / STUB_NOT_RENAMABLE
// ---------------------------------------------------------------------------
//
// Stubs are entities present in the store but with no body/type — they
// surface when `memstead_relate` targets an absent id (auto-stub) or when
// a delete demotes an entity with read-only referrers. The typed-stub
// error variants (`STUB_NOT_UPDATABLE`, `STUB_NOT_RENAMABLE`,
// `STUB_CANNOT_RELATE`) tell the agent to promote the stub via
// `memstead_create` before mutating. The auto-stub side rides a
// `AUTO_STUB_CREATED` warning on the relate response.

/// Full pin: same multi-step flow.
#[test]
fn full_auto_stub_then_update_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, _) = create_and_get_id_hash(&mut harness, "Source");
    let stub_id = "demo--ghost";

    let relate = harness.call_tool(
        "memstead_relate",
        json!({ "relations": [{ "from": source, "to": stub_id, "type": "USES" }] }),
    );
    let _ = assert_success_envelope(&relate);
    let body = relate
        .get("structuredContent")
        .expect("structuredContent missing on relate");
    let warnings = body
        .get("warnings")
        .and_then(Value::as_array)
        .expect("relate-to-absent-target must carry warnings[]");
    let codes: Vec<&str> = warnings
        .iter()
        .filter_map(|w| w.get("code").and_then(Value::as_str))
        .collect();
    assert!(
        codes.contains(&"AUTO_STUB_CREATED"),
        "expected AUTO_STUB_CREATED warning, got codes={codes:?}: {body}"
    );

    let update = harness.call_tool(
        "memstead_update",
        json!({
            "id": stub_id,
            "expected_hash": "",
            "sections": { "identity": "promotion-attempt" },
        }),
    );
    let is_error = update
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(is_error, "expected isError on stub update: {update}");
    let structured = update
        .get("structuredContent")
        .expect("structuredContent missing");
    assert_eq!(
        structured.get("code").and_then(Value::as_str),
        Some("STUB_NOT_UPDATABLE"),
        "code drifted: {structured}"
    );
}

/// Full pin: same.
#[test]
fn full_rename_stub_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, _) = create_and_get_id_hash(&mut harness, "Source");
    let stub_id = "demo--ghost";

    let _ = harness.call_tool(
        "memstead_relate",
        json!({ "relations": [{ "from": source, "to": stub_id, "type": "USES" }] }),
    );

    let rename = harness.call_tool(
        "memstead_rename",
        json!({ "id": stub_id, "new_title": "Promoted", "expected_hash": "" }),
    );
    let is_error = rename
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(is_error, "expected isError on stub rename: {rename}");
    let structured = rename
        .get("structuredContent")
        .expect("structuredContent missing");
    assert_eq!(
        structured.get("code").and_then(Value::as_str),
        Some("STUB_NOT_RENAMABLE"),
        "code drifted: {structured}"
    );
}

// ---------------------------------------------------------------------------
// `memstead_relate` STUB_CANNOT_RELATE — relate FROM an auto-stub source
// ---------------------------------------------------------------------------
//
// Stubs have no entity_type and cannot author edges. Bootstrap: relate
// to an absent target → engine auto-stubs that target. Then try to
// relate FROM the stub → STUB_CANNOT_RELATE. The agent's recovery is
// `memstead_create` to promote the stub.

/// Full pin.
#[test]
fn full_relate_from_stub_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, _) = create_and_get_id_hash(&mut harness, "Real");
    let stub_id = "demo--ghost";

    let _ = harness.call_tool(
        "memstead_relate",
        json!({ "relations": [{ "from": source, "to": stub_id, "type": "USES" }] }),
    );

    let result = harness.call_tool(
        "memstead_relate",
        json!({ "relations": [{ "from": stub_id, "to": source, "type": "USES" }] }),
    );
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(is_error, "expected isError on relate-from-stub: {result}");
    let structured = result
        .get("structuredContent")
        .expect("structuredContent missing");
    assert_eq!(
        structured.get("code").and_then(Value::as_str),
        Some("STUB_CANNOT_RELATE"),
        "code drifted: {structured}"
    );
}

// ---------------------------------------------------------------------------
// Full-only mem-lifecycle pin — `memstead_mem_create` success
// ---------------------------------------------------------------------------

/// Full pin: with a permissive `[[mem_management.create]]` rule in
/// `workspace.toml`, `memstead_mem_create` succeeds and registers a new
/// mem. Response shape carries the new mem's identity so the agent
/// can chain follow-up mutations.
#[test]
fn full_memstead_mem_create_returns_typed_success_envelope() {
    // The mem-management matcher tests the candidate against the
    // pattern. The candidate is the mem NAME (not the location
    // path) so a wildcard pattern admits any name. The location lives
    // on disk at the operator's discretion.
    const WORKSPACE_TOML_WITH_CREATE_RULE: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
\n\
[[mem_management.create]]\n\
pattern = \"*\"\n\
schemas = [\"default@1.0.0\"]\n\
";

    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("demo", "default@1.0.0")],
        WORKSPACE_TOML_WITH_CREATE_RULE,
    );

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_mem_create",
        json!({
            "name": "fresh",
            "location": "mems/fresh",
            "schema": "default@1.0.0",
        }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on mem_create success");
    // The exact response field set is engine-derived; the pin
    // checks the bare minimum: the new mem's name is echoed back so
    // the agent can chain follow-up mutations against it.
    assert!(
        body.get("name").is_some() || body.get("mem").is_some(),
        "mem_create response missing name/mem: {body}"
    );
}

/// Full pin: with permissive `[[mem_management.create]]` and `.delete]]`
/// rules, `memstead_mem_delete` against an existing mem returns a success
/// envelope. The pin checks the success flag and presence of
/// `structured_content` — exact response fields are engine-derived.
#[test]
fn full_memstead_mem_delete_returns_typed_success_envelope() {
    const WORKSPACE_TOML_WITH_LIFECYCLE_RULES: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
\n\
[[mem_management.create]]\n\
pattern = \"*\"\n\
schemas = [\"default@1.0.0\"]\n\
\n\
[[mem_management.delete]]\n\
pattern = \"*\"\n\
";

    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("demo", "default@1.0.0")],
        WORKSPACE_TOML_WITH_LIFECYCLE_RULES,
    );

    let mut harness = WireHarness::start(tmp.path());

    // Create a fresh mem first so we have something to delete that
    // is not the seeded `demo` (which has a real git-branch ref).
    let create = harness.call_tool(
        "memstead_mem_create",
        json!({
            "name": "ephemeral",
            "location": "mems/ephemeral",
            "schema": "default@1.0.0",
        }),
    );
    let _ = assert_success_envelope(&create);

    // Now delete it. The MCP wrapper hardcodes `delete_files: true`,
    // so this is always destructive.
    let del = harness.call_tool("memstead_mem_delete", json!({ "name": "ephemeral" }));
    let _ = assert_success_envelope(&del);
    assert!(
        del.get("structuredContent").is_some(),
        "mem_delete response missing structuredContent: {del}"
    );
}

/// MCP parity for the CLI
/// F7 regression. `memstead_mem_delete` (always destructive) scrubs the
/// deleted mem's dangling `[cross_mem_links]` grant but PRESERVES
/// the exact-name `[[mem_management.create]]` /
/// `[[mem_management.delete]]` allowlist rules — they are
/// forward-looking permissions for the name. So a follow-up
/// `memstead_mem_create` of the same name succeeds without re-granting.
/// The cross-link grant points OUT of the deleted mem
/// (`ephemeral → demo`) so the delete's own `MEM_REFERENCED_BY_POLICY`
/// gate (which fires only when another mem grants the target) stays
/// clear.
#[test]
fn full_mem_delete_preserves_allowlist_rules_so_recreate_succeeds() {
    const WORKSPACE_TOML: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
\n\
[cross_mem_links]\n\
ephemeral = [\"demo\"]\n\
\n\
[[mem_management.create]]\n\
pattern = \"ephemeral\"\n\
schemas = [\"default@1.0.0\"]\n\
\n\
[[mem_management.delete]]\n\
pattern = \"ephemeral\"\n\
";

    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(tmp.path(), &[("demo", "default@1.0.0")], WORKSPACE_TOML);

    let mut harness = WireHarness::start(tmp.path());

    // Create `ephemeral` — admitted by the exact-name create rule.
    let create = harness.call_tool(
        "memstead_mem_create",
        json!({
            "name": "ephemeral",
            "location": "mems/ephemeral",
            "schema": "default@1.0.0",
        }),
    );
    let _ = assert_success_envelope(&create);

    // Destructive delete — admitted by the exact-name delete rule.
    let del = harness.call_tool("memstead_mem_delete", json!({ "name": "ephemeral" }));
    let _ = assert_success_envelope(&del);

    // The exact-name create + delete allowlist rules survive the delete.
    let after =
        std::fs::read_to_string(tmp.path().join(".memstead").join("workspace.toml")).unwrap();
    assert_eq!(
        after.matches("pattern = \"ephemeral\"").count(),
        2,
        "delete must preserve the create+delete allowlist rules; got:\n{after}",
    );
    // The deleted mem's own dangling cross-link grant is scrubbed.
    assert!(
        !after.contains("ephemeral = [\"demo\"]"),
        "delete must scrub the deleted mem's dangling cross-link grant; got:\n{after}",
    );

    // Re-create the same name — succeeds with no fresh allow-create.
    let recreate = harness.call_tool(
        "memstead_mem_create",
        json!({
            "name": "ephemeral",
            "location": "mems/ephemeral",
            "schema": "default@1.0.0",
        }),
    );
    let _ = assert_success_envelope(&recreate);
}

/// Item 01 pin: `memstead-mcp --operator-mode` plumbs the bypass through
/// the MCP boundary. With zero `[[mem_management.create]]` /
/// `[[mem_management.delete]]` rules, an operator-mode server can
/// still `memstead_mem_create` and `memstead_mem_delete` a fresh mem;
/// a server booted without the flag against the same workspace
/// returns `MEM_PATH_NOT_ALLOWED` reason=`no_allowlist_configured`.
#[test]
fn full_operator_mode_bypasses_empty_allowlist_via_mcp() {
    // Workspace.toml carries no `[mem_management]` section at all —
    // every agent-mode lifecycle call rejects with the
    // `no_allowlist_configured` envelope. Operator-mode admits the
    // call regardless.
    const WORKSPACE_TOML_NO_RULES: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
";

    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("demo", "default@1.0.0")],
        WORKSPACE_TOML_NO_RULES,
    );

    // Agent-mode: rejected.
    {
        let mut harness = WireHarness::start(tmp.path());
        let agent_attempt = harness.call_tool(
            "memstead_mem_create",
            json!({
                "name": "fresh",
                "location": "mems/fresh",
                "schema": "default@1.0.0",
            }),
        );
        let is_error = agent_attempt
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert!(
            is_error,
            "agent-mode create against empty allowlist must error: {agent_attempt}"
        );
        let structured = agent_attempt
            .get("structuredContent")
            .expect("structuredContent missing on agent-mode envelope");
        assert_eq!(
            structured.get("code").and_then(Value::as_str),
            Some("MEM_PATH_NOT_ALLOWED"),
            "agent-mode rejection must carry MEM_PATH_NOT_ALLOWED: {structured}"
        );
        assert_eq!(
            structured
                .get("details")
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str),
            Some("no_allowlist_configured"),
            "details.reason drifted: {structured}"
        );
    }

    // Operator-mode: same call succeeds.
    {
        let mut harness = WireHarness::start_with_args(tmp.path(), &["--operator-mode"]);
        let create = harness.call_tool(
            "memstead_mem_create",
            json!({
                "name": "fresh",
                "location": "mems/fresh",
                "schema": "default@1.0.0",
            }),
        );
        let _ = assert_success_envelope(&create);

        // And the matching delete also succeeds — both gates are bypassed.
        let del = harness.call_tool("memstead_mem_delete", json!({ "name": "fresh" }));
        let _ = assert_success_envelope(&del);
    }
}

/// Item 01 pin: `memstead_overview` surfaces the operator-mode posture so
/// anyone reading the engine's output can confirm the bypass is in
/// force. The disclosure lives under `## Lifecycle Namespaces`, where
/// the allowlist policy itself is rendered — colocating the policy
/// and its bypass posture keeps the surface coherent.
#[test]
fn full_memstead_overview_surfaces_operator_mode_bypass() {
    const WORKSPACE_TOML: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
";

    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(tmp.path(), &[("demo", "default@1.0.0")], WORKSPACE_TOML);

    // Agent-mode overview omits the bypass disclosure.
    {
        let mut harness = WireHarness::start(tmp.path());
        let overview = harness.call_tool("memstead_overview", json!({}));
        let text = assert_success_envelope(&overview);
        assert!(
            !text.contains("--operator-mode"),
            "agent-mode overview must NOT mention operator-mode: {text}"
        );
    }

    // Operator-mode overview names the bypass and the gates it
    // shorts.
    {
        let mut harness = WireHarness::start_with_args(tmp.path(), &["--operator-mode"]);
        let overview = harness.call_tool("memstead_overview", json!({}));
        let text = assert_success_envelope(&overview);
        assert!(
            text.contains("--operator-mode"),
            "operator-mode overview must mention the flag: {text}"
        );
        assert!(
            text.contains("MEM_REFERENCED_BY_POLICY"),
            "operator-mode overview must name the bypassed safeguard: {text}"
        );
    }
}

/// Item 03 pin: `memstead_mem_create` against a mem-repo workspace
/// produces a `mounts.json` whose new git-branch entry carries the
/// fully-qualified `refs/heads/<leaf>` form for the `branch` field.
/// Pre-fix the writer already produced the long form; this pin guards
/// against a regression that re-introduces the short-form drift the
/// older committed `mounts.json` files used to carry (and which made
/// every fresh-workspace rebuild produce noise-only diffs against the
/// legacy shape).
#[test]
fn full_memstead_mem_create_writes_refs_heads_branch_in_mounts_json() {
    const WORKSPACE_TOML_WITH_CREATE_RULE: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
\n\
[[mem_management.create]]\n\
pattern = \"*\"\n\
schemas = [\"default@1.0.0\"]\n\
\n\
[[mem_management.create]]\n\
pattern = \"namespace/*\"\n\
schemas = [\"default@1.0.0\"]\n\
";

    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("demo", "default@1.0.0")],
        WORKSPACE_TOML_WITH_CREATE_RULE,
    );

    let mut harness = WireHarness::start(tmp.path());

    // Flat-layout create — branch_leaf is the bare name.
    let flat = harness.call_tool(
        "memstead_mem_create",
        json!({
            "name": "fresh",
            "location": "mems/fresh",
            "schema": "default@1.0.0",
        }),
    );
    let _ = assert_success_envelope(&flat);

    // Hierarchical paths are first-class. `name = "namespace/scoped"`
    // IS the full identifier — there is no separate `path` wire field.
    let hier = harness.call_tool(
        "memstead_mem_create",
        json!({
            "name": "namespace/scoped",
            "location": "mems/scoped",
            "schema": "default@1.0.0",
        }),
    );
    let _ = assert_success_envelope(&hier);

    let mounts_json_path = tmp
        .path()
        .join(".memstead")
        .join("state")
        .join("mounts.json");
    let on_disk = std::fs::read_to_string(&mounts_json_path)
        .expect("mounts.json must exist after mem_create");
    assert!(
        on_disk.contains("\"branch\": \"refs/heads/fresh\""),
        "flat-layout mem must persist refs/heads/<name>, got: {on_disk}"
    );
    assert!(
        on_disk.contains("\"branch\": \"refs/heads/namespace/scoped\""),
        "hierarchical mem must persist refs/heads/<full-name>, got: {on_disk}"
    );
    // `mounts.json` carries the full hierarchical name as the mem
    // identifier (not the bare leaf).
    assert!(
        on_disk.contains("\"mem\": \"namespace/scoped\""),
        "hierarchical mem identity is the full path in mounts.json, got: {on_disk}"
    );
}

// ---------------------------------------------------------------------------
// Typed envelope coverage for description-posture + wikilink-without-
// relation errors. Both used to fall through to the wildcard
// `_ => INTERNAL` arm in `engine_err_unified`; the match is now
// exhaustive, and these reproducers pin the typed wire shape.
// ---------------------------------------------------------------------------

/// `memstead_relate` on a rel-type whose schema declares
/// `per_edge_description: forbidden` (REFERENCES in default@1.0.0) with a
/// description ships `code: DESCRIPTION_NOT_PERMITTED` + structured
/// `details.{rel_type,from_id,to_id}` — not a bare `INTERNAL`.
#[test]
fn full_memstead_relate_with_forbidden_description_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (from, _) = create_and_get_id_hash(&mut harness, "Forbid Source");
    let (to, _) = create_and_get_id_hash(&mut harness, "Forbid Target");

    let result = harness.call_tool(
        "memstead_relate",
        json!({ "relations": [{ "from": from,
            "to": to,
            "type": "REFERENCES",
            "description": "should be refused" }] }),
    );
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(
        is_error,
        "expected isError=true on description-forbidden relate: {result}",
    );
    let structured = result
        .get("structuredContent")
        .expect("structuredContent missing on description-forbidden relate");
    assert_eq!(
        structured.get("code").and_then(Value::as_str),
        Some("DESCRIPTION_NOT_PERMITTED"),
        "wire code regressed to non-typed: {structured}",
    );
    let details = structured
        .get("details")
        .expect("DESCRIPTION_NOT_PERMITTED must carry details");
    assert_eq!(
        details.get("rel_type").and_then(Value::as_str),
        Some("REFERENCES"),
        "details.rel_type drifted: {details}",
    );
    assert_eq!(
        details.get("from_id").and_then(Value::as_str),
        Some(from.as_str()),
        "details.from_id drifted: {details}",
    );
    assert_eq!(
        details.get("to_id").and_then(Value::as_str),
        Some(to.as_str()),
        "details.to_id drifted: {details}",
    );
}

/// `memstead_update` that introduces a body wiki-link without a backing relation
/// ships `code: WIKILINK_WITHOUT_RELATION` + structured `details.{from_id,
/// missing[]}` listing each unbacked link's `section_key` and `target_id`.
/// A bare `INTERNAL` here would train agents to treat the
/// recoverable input error as an engine bug.
#[test]
fn full_memstead_update_body_wikilink_auto_synthesises_alias_relation() {
    // Under the default schema's `alias_target_rel_type: REFERENCES`
    // pointer, a body wiki-link no longer trips `WIKILINK_WITHOUT_RELATION`:
    // the alias-synthesis pass emits the REFERENCES relation first,
    // the mutation succeeds, and the relation is observable on the
    // entity afterward. Schemas without the pointer continue to surface
    // the typed `WIKILINK_WITHOUT_RELATION` envelope — that path is
    // covered by a fixture-schema test in the engine crate.
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, source_hash) = create_and_get_id_hash(&mut harness, "WikiSource");
    let (target, _) = create_and_get_id_hash(&mut harness, "WikiTarget");

    let result = harness.call_tool(
        "memstead_update",
        json!({
            "id": source,
            "expected_hash": source_hash,
            "sections": {
                "identity": format!("see [[{target}]] for context"),
            },
        }),
    );
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(
        !is_error,
        "alias-synthesis must satisfy the validator and let the body land: {result}",
    );

    // The entity now carries the auto-emitted REFERENCES relation.
    let entity = harness.call_tool("memstead_entity", json!({ "id": source }));
    let relationships = entity
        .get("structuredContent")
        .and_then(|sc| sc.get("relationships"))
        .and_then(Value::as_array)
        .expect("relationships[] missing from structured envelope");
    let has_ref = relationships.iter().any(|r| {
        r.get("rel_type").and_then(Value::as_str) == Some("REFERENCES")
            && r.get("target").and_then(Value::as_str) == Some(target.as_str())
    });
    assert!(
        has_ref,
        "REFERENCES → target must surface in relationships[]; got {relationships:?}",
    );
}

// ---------------------------------------------------------------------------
// Friction ledger (agent-trust plan 08) — the dual-surface fixture.
// ---------------------------------------------------------------------------

/// One fixture drives both surfaces: a refused MCP call (through the
/// REAL dispatch seam — the spawned binary's `call_tool`) and a
/// refused CLI call each append one ledger entry (values from closed
/// engine-defined vocabularies only — the module's privacy rule);
/// successful calls on both surfaces append nothing; the wire-served
/// `include: ["friction"]` axis reports the combined counts.
///
/// The CLI binary is resolved from the mcp binary's target directory —
/// both are built by the canonical workspace test surface
/// (`run-tests.sh`, workspace-wide nextest).
#[test]
fn friction_ledger_records_both_surfaces_and_serves_the_axis() {
    let tmp = TempDir::new().unwrap();
    seed_empty_workspace(tmp.path());
    let ledger_path = tmp
        .path()
        .join(".memstead")
        .join("state")
        .join("friction")
        .join("refusals.jsonl");
    let entries = |path: &Path| -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("every ledger line parses"))
            .collect()
    };

    let cli_bin = Path::new(memstead_mcp_bin())
        .parent()
        .expect("binary has a parent dir")
        .join("memstead");
    assert!(
        cli_bin.exists(),
        "memstead CLI binary not built — run the workspace test surface (run-tests.sh)"
    );

    // Refused MCP call through the real wire: unknown mem.
    let mut harness = WireHarness::start(tmp.path());
    let refused = harness.call_tool(
        "memstead_entity",
        json!({ "id": "ghost--entity", "sections": [] }),
    );
    assert_eq!(refused["isError"], true, "{refused}");
    let after_mcp = entries(&ledger_path);
    assert_eq!(after_mcp.len(), 1, "one entry per refused MCP call");
    assert_eq!(after_mcp[0]["surface"], "mcp");
    assert_eq!(after_mcp[0]["verb"], "memstead_entity");
    assert_eq!(
        after_mcp[0]["code"], refused["structuredContent"]["code"],
        "ledger code matches the served refusal"
    );
    assert!(after_mcp[0]["ts"].as_u64().unwrap() > 0);

    // Refused CLI call against the SAME workspace/ledger.
    let out = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args(["--json", "entity", "ghost--entity"])
        .output()
        .expect("run memstead CLI");
    assert!(!out.status.success(), "CLI fixture call must refuse");
    let after_cli = entries(&ledger_path);
    assert_eq!(after_cli.len(), 2, "one entry per refused CLI call");
    assert_eq!(after_cli[1]["surface"], "cli");
    assert_eq!(after_cli[1]["verb"], "entity");

    // Successful calls on both surfaces append nothing.
    let ok = harness.call_tool("memstead_health", json!({}));
    assert!(ok["isError"] != true, "{ok}");
    let ok_cli = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args(["--json", "health"])
        .output()
        .expect("run memstead CLI");
    assert!(ok_cli.status.success());
    assert_eq!(
        entries(&ledger_path).len(),
        2,
        "successful calls append nothing"
    );

    // The wire-served include axis reports the combined counts.
    let served = harness.call_tool("memstead_health", json!({ "include": ["friction"] }));
    assert!(served["isError"] != true, "{served}");
    let axis = &served["structuredContent"]["friction"];
    assert_eq!(axis["total"], 2, "{served}");
    assert_eq!(axis["by_verb"]["mcp:memstead_entity"], 1);
    assert_eq!(axis["by_verb"]["cli:entity"], 1);
    assert_eq!(axis["recent_24h"]["total"], 2);
}

// ---------------------------------------------------------------------------
// Negative findings (agent-trust plan 10) — the fourth ingest type.
// ---------------------------------------------------------------------------

/// ingest@0.5.0's `negative_finding`: a conformant entity writes via
/// MCP and via the CLI against the same process mem; malformed
/// variants refuse with the standard typed conformance errors; and
/// the type's leaf declaration keeps edge-less findings out of the
/// orphan axis (they surface as a leaf population instead).
#[test]
fn negative_finding_writes_on_both_surfaces_and_is_leaf_exempt() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("proc", "ingest@0.5.0")]);
    let mut harness = WireHarness::start(tmp.path());

    // Legal via MCP: all three required sections.
    let ok = harness.call_tool(
        "memstead_create",
        json!({
            "title": "No rollback runbook in the source tree",
            "entity_type": "negative_finding",
            "mem": "proc",
            "sections": {
                "sought": "A rollback runbook for failed deploys.",
                "search_path": "Full read of docs/ops; grep for rollback and revert across docs/.",
                "finding": "Nothing — deploys are documented forward-only."
            }
        }),
    );
    assert!(
        ok["isError"] != true,
        "legal negative_finding must land: {ok}"
    );
    assert_eq!(
        ok["structuredContent"]["id"], "proc--no-rollback-runbook-in-the-source-tree",
        "{ok}"
    );

    // Illegal via MCP: missing required sections → typed refusal.
    let missing = harness.call_tool(
        "memstead_create",
        json!({
            "title": "Half a finding",
            "entity_type": "negative_finding",
            "mem": "proc",
            "sections": { "sought": "Something." }
        }),
    );
    assert_eq!(missing["isError"], true, "{missing}");
    assert_eq!(
        missing["structuredContent"]["code"], "MISSING_REQUIRED_SECTION",
        "{missing}"
    );

    // CLI against the SAME workspace: legal write.
    let cli_bin = Path::new(memstead_mcp_bin())
        .parent()
        .expect("binary has a parent dir")
        .join("memstead");
    assert!(cli_bin.exists(), "memstead CLI binary not built");
    let out = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args([
            "--json",
            "create",
            "--mem",
            "proc",
            "--title",
            "No SLA stated for the batch queue",
            "--type",
            "negative_finding",
            "--section",
            "sought=A latency or delivery SLA for the batch queue.",
            "--section",
            "search_path=Skim of the queue chapter; grep for SLA and latency across docs/.",
            "--section",
            "finding=Nothing — the queue is documented without service guarantees.",
        ])
        .output()
        .expect("run memstead CLI");
    assert!(
        out.status.success(),
        "legal CLI negative_finding must land: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // CLI illegal variant: unknown section → typed refusal.
    let bad = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args([
            "--json",
            "create",
            "--mem",
            "proc",
            "--title",
            "Bad finding",
            "--type",
            "negative_finding",
            "--section",
            "sought=X.",
            "--section",
            "search_path=Y.",
            "--section",
            "finding=Z.",
            "--section",
            "bogus_section=nope",
        ])
        .output()
        .expect("run memstead CLI");
    assert!(!bad.status.success());
    let body: Value = serde_json::from_slice(&bad.stdout).expect("CLI --json refusal parses");
    assert_eq!(body["code"], "UNKNOWN_SECTION", "{body}");

    // Leaf exemption: both findings are edge-less, yet the orphan
    // axis lists neither — they surface as the leaf population.
    let health = harness.call_tool("memstead_health", json!({ "include": ["orphans"] }));
    assert!(health["isError"] != true, "{health}");
    let orphans = serde_json::to_string(&health["structuredContent"]["orphans"]).unwrap();
    assert!(
        !orphans.contains("no-rollback-runbook") && !orphans.contains("no-sla-stated"),
        "leaf-typed negative findings must not appear as orphans: {orphans}"
    );
    let leaf = &health["structuredContent"]["leaf_entities_by_type"];
    assert_eq!(leaf["ingest@0.5.0:negative_finding"], 2, "{health}");
}

/// The `open_questions` axis over the wire (agent-trust plan 11):
/// include-gated (absent without the include), an empty workspace
/// serves an empty axis rather than an error, no leaf is `INTERNAL`,
/// and an unknown `mem` scope refuses typed.
#[test]
fn open_questions_axis_is_include_gated_and_refuses_unknown_mem_typed() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("specs", "default@1.3.0")]);
    let mut harness = WireHarness::start(tmp.path());

    // Without the include: no axis key.
    let plain = harness.call_tool("memstead_health", json!({}));
    assert!(plain["isError"] != true, "{plain}");
    assert!(
        plain["structuredContent"].get("open_questions").is_none(),
        "axis must be include-gated: {plain}"
    );

    // With the include on a hole-free mem: an empty axis, not an error.
    let served = harness.call_tool("memstead_health", json!({ "include": ["open_questions"] }));
    assert!(served["isError"] != true, "{served}");
    let axis = &served["structuredContent"]["open_questions"];
    assert_eq!(axis["_item_cap"], 20, "{served}");
    assert_eq!(axis["specs"]["total_open"], 0, "{served}");
    assert_eq!(axis["specs"]["stubs"]["count"], 0);
    assert!(
        !serde_json::to_string(&served)
            .unwrap()
            .contains("\"INTERNAL\""),
        "no leaf of the axis is INTERNAL: {served}"
    );

    // Unknown mem scope refuses typed.
    let ghost = harness.call_tool(
        "memstead_health",
        json!({ "include": ["open_questions"], "mem": "ghost" }),
    );
    assert_eq!(ghost["isError"], true, "{ghost}");
    assert_eq!(ghost["structuredContent"]["code"], "UNKNOWN_MEM", "{ghost}");
}

/// The `stale_derivations` axis over the wire (agent-trust plan 12):
/// include-gated (absent without the include), a mem with no declared
/// derivation rel-types serves an empty list rather than an error, no
/// leaf is `INTERNAL`, and an unknown `mem` scope refuses typed.
#[test]
fn stale_derivations_axis_is_include_gated_and_refuses_unknown_mem_typed() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("specs", "default@1.3.0")]);
    let mut harness = WireHarness::start(tmp.path());

    let plain = harness.call_tool("memstead_health", json!({}));
    assert!(plain["isError"] != true, "{plain}");
    assert!(
        plain["structuredContent"]
            .get("stale_derivations")
            .is_none(),
        "axis must be include-gated: {plain}"
    );

    let served = harness.call_tool(
        "memstead_health",
        json!({ "include": ["stale_derivations"] }),
    );
    assert!(served["isError"] != true, "{served}");
    let axis = &served["structuredContent"]["stale_derivations"];
    assert_eq!(
        axis["specs"],
        json!([]),
        "undeclared schema → empty list: {served}"
    );
    assert!(
        !serde_json::to_string(&served)
            .unwrap()
            .contains("\"INTERNAL\""),
        "no leaf is INTERNAL: {served}"
    );

    let ghost = harness.call_tool(
        "memstead_health",
        json!({ "include": ["stale_derivations"], "mem": "ghost" }),
    );
    assert_eq!(ghost["isError"], true, "{ghost}");
    assert_eq!(ghost["structuredContent"]["code"], "UNKNOWN_MEM", "{ghost}");
}

// ---------------------------------------------------------------------------
// Provenance at mutation (agent-trust plan 13) — the record half.
// ---------------------------------------------------------------------------

/// The checks axis serves the four derived states, and the
/// independence gate refuses to manufacture identity from transport:
/// the recorded `(actor, client)` pair names the surface a record
/// arrived through, not who acted, so until a caller-declared
/// identity exists (caller-identity follow-up) every ok-checked
/// entity is `unconfirmable` — same-surface author+check is never
/// `self_checked`, cross-surface author/check is never
/// `confirmed_independent`. Both categories stay in the wire shape
/// as explicit empties.
#[test]
fn checks_health_axis_serves_unconfirmable_without_caller_identity() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("specs", "default@1.3.0")]);
    let mut harness = WireHarness::start(tmp.path());
    let cli_bin = Path::new(memstead_mcp_bin())
        .parent()
        .expect("binary has a parent dir")
        .join("memstead");

    // Four entities, all authored via MCP (same recorded author
    // identity: agent + the harness client).
    for title in [
        "Alpha Claim",
        "Beta Claim",
        "Gamma Claim",
        "Delta Claim",
        "Epsilon Claim",
        "Zeta Claim",
    ] {
        let created = harness.call_tool(
            "memstead_create",
            json!({
                "title": title,
                "entity_type": "spec",
                "mem": "specs",
                "sections": { "identity": "I.", "purpose": "P." },
                "role": "author"
            }),
        );
        assert!(created["isError"] != true, "{created}");
    }

    // Alpha: ok-checked via MCP as checker — same transport as the
    // author, but transport is not identity: without a
    // caller-declared identity (caller-identity follow-up) the gate
    // cannot establish sameness → unconfirmable.
    let r = harness.call_tool(
        "memstead_check",
        json!({ "entity": "specs--alpha-claim", "verdict": "ok", "role": "checker" }),
    );
    assert!(r["isError"] != true, "{r}");

    // Beta: ok-checked via the CLI as checker — a different recorded
    // transport pair (cli + memstead-cli client), which does NOT
    // establish a different actor → unconfirmable, never a false
    // acquittal via transport.
    let out = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args([
            "--json",
            "--role",
            "checker",
            "check",
            "specs--beta-claim",
            "--verdict",
            "ok",
        ])
        .output()
        .expect("run memstead CLI check");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Gamma: ok-checked with NO role — records honestly, but an
    // unspecified-role check cannot confirm independence.
    let r = harness.call_tool(
        "memstead_check",
        json!({ "entity": "specs--gamma-claim", "verdict": "ok" }),
    );
    assert!(r["isError"] != true, "{r}");

    // Epsilon: ok-checked, then edited — the axis serves check_stale.
    let r = harness.call_tool(
        "memstead_check",
        json!({ "entity": "specs--epsilon-claim", "verdict": "ok", "role": "checker" }),
    );
    assert!(r["isError"] != true, "{r}");
    let read = harness.call_tool("memstead_entity", json!({ "id": "specs--epsilon-claim" }));
    let eps_hash = read["structuredContent"]["_hash"]
        .as_str()
        .unwrap()
        .to_string();
    let r = harness.call_tool(
        "memstead_update",
        json!({
            "id": "specs--epsilon-claim",
            "expected_hash": eps_hash,
            "sections": { "purpose": "P2." },
            "role": "author"
        }),
    );
    assert!(r["isError"] != true, "{r}");

    // Zeta: failed check — the axis serves check_failed.
    let r = harness.call_tool(
        "memstead_check",
        json!({ "entity": "specs--zeta-claim", "verdict": "failed", "role": "checker" }),
    );
    assert!(r["isError"] != true, "{r}");

    // Delta stays never-checked.
    let health = harness.call_tool("memstead_health", json!({ "include": ["checks"] }));
    assert!(health["isError"] != true, "{health}");
    let axis = &health["structuredContent"]["checks"]["specs"];
    assert_eq!(axis["checked_ok"], 3, "{axis}");
    assert_eq!(axis["check_stale"], 1, "{axis}");
    assert_eq!(axis["check_failed"], 1, "{axis}");
    assert!(axis["never_checked"].as_u64().unwrap() >= 1, "{axis}");
    let gate = &axis["independence"];
    // Transport is not identity: until a caller-declared identity
    // exists (caller-identity follow-up) every ok-checked entity is
    // unconfirmable; self_checked / confirmed_independent stay as
    // categories whose empty lists are a statement.
    assert_eq!(gate["self_checked"]["items"], json!([]), "{gate}");
    assert_eq!(gate["confirmed_independent"]["items"], json!([]), "{gate}");
    assert_eq!(
        gate["unconfirmable"]["items"],
        json!([
            "specs--alpha-claim",
            "specs--beta-claim",
            "specs--gamma-claim"
        ]),
        "{gate}"
    );

    // CLI parity: the same axis through `memstead health`.
    let out = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args(["--json", "health", "--include", "checks"])
        .output()
        .expect("run memstead CLI health");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["checks"]["specs"]["independence"]["unconfirmable"]["items"],
        json!([
            "specs--alpha-claim",
            "specs--beta-claim",
            "specs--gamma-claim"
        ]),
        "{v}"
    );
}

#[test]
fn check_operation_records_derives_state_and_mutates_nothing() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("specs", "default@1.3.0")]);
    let mut harness = WireHarness::start(tmp.path());
    let cli_bin = Path::new(memstead_mcp_bin())
        .parent()
        .expect("binary has a parent dir")
        .join("memstead");
    assert!(cli_bin.exists(), "memstead CLI binary not built");

    // Author an entity. It starts never-checked.
    let created = harness.call_tool(
        "memstead_create",
        json!({
            "title": "Checked Claim",
            "entity_type": "spec",
            "mem": "specs",
            "sections": { "identity": "I.", "purpose": "P." },
            "role": "author"
        }),
    );
    assert!(created["isError"] != true, "{created}");
    let hash = created["structuredContent"]["_hash"]
        .as_str()
        .unwrap()
        .to_string();
    let commit_before = created["structuredContent"]["commit_sha"]
        .as_str()
        .unwrap()
        .to_string();

    let read = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--checked-claim", "include_provenance": true }),
    );
    assert_eq!(
        read["structuredContent"]["mutation_provenance"]["check_state"], "never_checked",
        "{read}"
    );

    // Refusal complements before any check lands: illegal verdict
    // (vocabulary named), unknown entity.
    let bad = harness.call_tool(
        "memstead_check",
        json!({ "entity": "specs--checked-claim", "verdict": "passed" }),
    );
    assert_eq!(bad["isError"], true, "{bad}");
    assert_eq!(bad["structuredContent"]["code"], "INVALID_VERDICT", "{bad}");
    assert!(
        serde_json::to_string(&bad["structuredContent"]["details"]["allowed"])
            .unwrap()
            .contains("failed"),
        "vocabulary named: {bad}"
    );
    let missing = harness.call_tool(
        "memstead_check",
        json!({ "entity": "specs--no-such-entity", "verdict": "ok" }),
    );
    assert_eq!(missing["isError"], true, "{missing}");
    assert_eq!(
        missing["structuredContent"]["code"], "ENTITY_NOT_FOUND",
        "{missing}"
    );

    // Check as checker, verdict ok → checked_ok.
    let checked = harness.call_tool(
        "memstead_check",
        json!({
            "entity": "specs--checked-claim",
            "verdict": "ok",
            "method": "diffed against source spec",
            "role": "checker"
        }),
    );
    assert!(checked["isError"] != true, "{checked}");
    assert_eq!(checked["structuredContent"]["check_state"], "checked_ok");
    assert_eq!(checked["structuredContent"]["role"], "checker");

    // Checking mutates nothing: entity `_hash` unchanged, mem history
    // gained no commit (the create's commit is still HEAD), markdown
    // untouched.
    let read = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--checked-claim", "include_provenance": true }),
    );
    let sc = &read["structuredContent"];
    assert_eq!(
        sc["_hash"].as_str().unwrap(),
        hash,
        "check must not touch _hash"
    );
    assert_eq!(
        sc["mutation_provenance"]["check_state"], "checked_ok",
        "{sc}"
    );
    let last = &sc["mutation_provenance"]["last_check"];
    assert_eq!(last["verdict"], "ok");
    assert_eq!(last["role"], "checker");
    assert_eq!(last["method"], "diffed against source spec");
    let gitdir = tmp.path().join("mem-repo").join(".git");
    let head = Command::new("git")
        .args([
            "--git-dir",
            gitdir.to_str().unwrap(),
            "rev-parse",
            "refs/heads/specs",
        ])
        .output()
        .expect("git rev-parse");
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        commit_before,
        "a check must not produce a mem commit"
    );

    // Entity edit → check_stale (computed by hash comparison, never
    // stamped).
    let updated = harness.call_tool(
        "memstead_update",
        json!({
            "id": "specs--checked-claim",
            "expected_hash": hash,
            "sections": { "purpose": "P2." },
            "role": "author"
        }),
    );
    assert!(updated["isError"] != true, "{updated}");
    let read = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--checked-claim", "include_provenance": true }),
    );
    assert_eq!(
        read["structuredContent"]["mutation_provenance"]["check_state"], "check_stale",
        "{read}"
    );

    // Re-check via the CLI (verb parity, session --role) → checked_ok.
    let out = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args([
            "--json",
            "--role",
            "verifier",
            "check",
            "specs--checked-claim",
            "--verdict",
            "ok",
        ])
        .output()
        .expect("run memstead CLI check");
    assert!(
        out.status.success(),
        "CLI check must land: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["check_state"], "checked_ok", "{v}");
    assert_eq!(v["role"], "verifier");
    let read = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--checked-claim", "include_provenance": true }),
    );
    assert_eq!(
        read["structuredContent"]["mutation_provenance"]["check_state"], "checked_ok",
        "{read}"
    );

    // A failed verdict serves check_failed — and supersession never
    // erases: the ledger keeps every record.
    let failed = harness.call_tool(
        "memstead_check",
        json!({ "entity": "specs--checked-claim", "verdict": "failed", "role": "checker" }),
    );
    assert!(failed["isError"] != true, "{failed}");
    assert_eq!(failed["structuredContent"]["check_state"], "check_failed");
    let ledger = std::fs::read_to_string(
        tmp.path()
            .join(".memstead")
            .join("state")
            .join("checks")
            .join("checks.jsonl"),
    )
    .expect("check ledger exists");
    assert_eq!(
        ledger.lines().count(),
        3,
        "append-only: every check kept: {ledger}"
    );

    // CLI illegal-verdict refusal parity.
    let out = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args([
            "--json",
            "check",
            "specs--checked-claim",
            "--verdict",
            "maybe",
        ])
        .output()
        .expect("run memstead CLI check");
    assert!(!out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["code"], "INVALID_VERDICT", "{v}");
}

#[test]
fn declared_roles_are_recorded_in_append_only_history_on_both_backends() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("specs", "default@1.3.0")]);
    let mut harness = WireHarness::start(tmp.path());

    // MCP create-as-author.
    let created = harness.call_tool(
        "memstead_create",
        json!({
            "title": "Derived Conclusion",
            "entity_type": "spec",
            "mem": "specs",
            "sections": { "identity": "I.", "purpose": "P." },
            "role": "author"
        }),
    );
    assert!(created["isError"] != true, "{created}");
    let hash = created["structuredContent"]["_hash"]
        .as_str()
        .unwrap()
        .to_string();

    // MCP illegal role → typed refusal naming the vocabulary.
    let bad = harness.call_tool(
        "memstead_create",
        json!({
            "title": "Nope",
            "entity_type": "spec",
            "mem": "specs",
            "sections": { "identity": "I.", "purpose": "P." },
            "role": "reviewer"
        }),
    );
    assert_eq!(bad["isError"], true, "{bad}");
    assert_eq!(bad["structuredContent"]["code"], "INVALID_ROLE", "{bad}");
    assert!(
        serde_json::to_string(&bad["structuredContent"]["details"]["allowed"])
            .unwrap()
            .contains("checker"),
        "vocabulary named: {bad}"
    );

    // CLI update-as-checker against the SAME workspace.
    let cli_bin = Path::new(memstead_mcp_bin())
        .parent()
        .expect("binary has a parent dir")
        .join("memstead");
    assert!(cli_bin.exists(), "memstead CLI binary not built");
    let out = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args([
            "--json",
            "--role",
            "checker",
            "update",
            "specs--derived-conclusion",
            "--expected-hash",
            &hash,
            "--append",
            "purpose= Checked.",
        ])
        .output()
        .expect("run memstead CLI");
    assert!(
        out.status.success(),
        "checker update must land: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // A mutation WITHOUT a role — no trailer, never defaulted.
    let plain = harness.call_tool(
        "memstead_create",
        json!({
            "title": "Plain Entity",
            "entity_type": "spec",
            "mem": "specs",
            "sections": { "identity": "I.", "purpose": "P." }
        }),
    );
    assert!(plain["isError"] != true, "{plain}");

    // CLI illegal role → typed refusal.
    let bad_cli = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args(["--json", "--role", "boss", "entity", "specs--plain-entity"])
        .output()
        .expect("run memstead CLI");
    assert!(!bad_cli.status.success());
    let v: Value = serde_json::from_slice(&bad_cli.stdout).unwrap();
    assert_eq!(v["code"], "INVALID_ROLE", "{v}");
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("author, checker, verifier"),
        "vocabulary named: {v}"
    );

    // The append-only record: commit trailers carry exactly the
    // declared roles, and the role-less commit carries none.
    let log = Command::new("git")
        .arg("--git-dir")
        .arg(tmp.path().join("mem-repo").join(".git"))
        .args(["log", "--format=%H%n%B%n---", "refs/heads/specs"])
        .output()
        .expect("git log");
    let log = String::from_utf8_lossy(&log.stdout).to_string();
    let commits: Vec<&str> = log.split("\n---").collect();
    let author_commit = commits
        .iter()
        .find(|c| c.contains("create specs--derived-conclusion"))
        .expect("create commit present");
    assert!(
        author_commit.contains("Role: author"),
        "author role recorded: {author_commit}"
    );
    let checker_commit = commits
        .iter()
        .find(|c| c.contains("update specs--derived-conclusion"))
        .expect("update commit present");
    assert!(
        checker_commit.contains("Role: checker"),
        "checker role recorded: {checker_commit}"
    );
    let plain_commit = commits
        .iter()
        .find(|c| c.contains("create specs--plain-entity"))
        .expect("plain create commit present");
    assert!(
        !plain_commit.contains("Role:"),
        "unspecified role records NO trailer: {plain_commit}"
    );

    // Folder-backend parity: a quickstart (folder) workspace's JSONL
    // ledger records the same shape for the same operations.
    let folder = TempDir::new().unwrap();
    let ws = folder.path().join("plainws");
    std::fs::create_dir_all(&ws).unwrap();
    let ok = Command::new(&cli_bin)
        .current_dir(&ws)
        .args(["quickstart"])
        .output()
        .expect("quickstart");
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let ok = Command::new(&cli_bin)
        .current_dir(&ws)
        .args([
            "--role",
            "verifier",
            "create",
            "--title",
            "Ledger Roled",
            "--type",
            "memo",
            "--section",
            "claim=Recorded.",
            "--section",
            "context=Role test.",
        ])
        .output()
        .expect("folder create");
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stdout)
    );
    let ok = Command::new(&cli_bin)
        .current_dir(&ws)
        .args([
            "--role",
            "checker",
            "update",
            "plainws--ledger-roled",
            "--force",
            "--section",
            "claim=Checked.",
        ])
        .output()
        .expect("folder update");
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stdout)
    );
    let ledger =
        std::fs::read_to_string(ws.join("plainws").join(".memstead").join("changes.jsonl"))
            .or_else(|_| std::fs::read_to_string(ws.join(".memstead").join("changes.jsonl")));
    let ledger = match ledger {
        Ok(l) => l,
        Err(_) => {
            // Quickstart mem dir name derives from the folder; find it.
            let mut found = String::new();
            for entry in std::fs::read_dir(&ws).unwrap().flatten() {
                let p = entry.path().join(".memstead").join("changes.jsonl");
                if p.exists() {
                    found = std::fs::read_to_string(p).unwrap();
                    break;
                }
            }
            found
        }
    };
    assert!(
        ledger.contains("\"role\":\"verifier\""),
        "folder ledger records the create role: {ledger}"
    );
    assert!(
        ledger.contains("\"role\":\"checker\""),
        "folder ledger records the update role: {ledger}"
    );

    // ---- Serve half: the entity read's opt-in provenance block. ----

    // Default read: byte-unchanged — no mutation_provenance key.
    let plain_read = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--derived-conclusion" }),
    );
    assert!(plain_read["isError"] != true, "{plain_read}");
    assert!(
        plain_read["structuredContent"]
            .get("mutation_provenance")
            .is_none(),
        "default entity reads carry no provenance block: {plain_read}"
    );

    // Opt-in read: created-by author, last-modified-by checker — the
    // criterion-1 fixture retrieved end to end.
    let read = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--derived-conclusion", "include_provenance": true }),
    );
    assert!(read["isError"] != true, "{read}");
    let prov = &read["structuredContent"]["mutation_provenance"];
    assert_eq!(prov["created_by"]["role"], "author", "{prov}");
    assert_eq!(prov["last_modified_by"]["role"], "checker", "{prov}");
    assert!(
        prov["created_by"]["client"].as_str().is_some(),
        "identity recorded: {prov}"
    );
    assert!(prov["created_by"]["timestamp"].as_i64().unwrap() > 0);
    // Identities compared across operations — the gate primitive:
    // both records carry actor identity, distinct roles.
    assert_ne!(
        prov["created_by"]["role"], prov["last_modified_by"]["role"],
        "author≠checker distinguishable from records"
    );

    // The role-less entity serves `unspecified` — recorded absence,
    // never defaulted to a real role.
    let read = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--plain-entity", "include_provenance": true }),
    );
    let prov = &read["structuredContent"]["mutation_provenance"];
    assert_eq!(prov["created_by"]["role"], "unspecified", "{prov}");

    // Immutability complement: reading provenance changes nothing —
    // `_hash` identical before/after, and the checker update did not
    // rewrite the creation record (append-only history is the
    // storage; no verb edits past provenance).
    let hash_now = read_hash_of(&mut harness, "specs--derived-conclusion");
    let reread = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--derived-conclusion", "include_provenance": true }),
    );
    assert_eq!(
        reread["structuredContent"]["_hash"], hash_now,
        "provenance reads are pure"
    );
    assert_eq!(
        reread["structuredContent"]["mutation_provenance"]["created_by"]["role"], "author",
        "the later checker update never altered the creation record"
    );

    // CLI parity on the SAME mem-repo workspace…
    let out = Command::new(&cli_bin)
        .current_dir(tmp.path())
        .args([
            "--json",
            "entity",
            "specs--derived-conclusion",
            "--provenance",
        ])
        .output()
        .expect("run memstead CLI");
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["mutation_provenance"]["created_by"]["role"], "author",
        "{v}"
    );
    assert_eq!(
        v["mutation_provenance"]["last_modified_by"]["role"],
        "checker"
    );

    // …and on the FOLDER workspace (backend parity: same shape for
    // the same operation sequence).
    let out = Command::new(&cli_bin)
        .current_dir(&ws)
        .args(["--json", "entity", "plainws--ledger-roled", "--provenance"])
        .output()
        .expect("run memstead CLI");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let p = &v["mutation_provenance"];
    assert_eq!(p["created_by"]["role"], "verifier", "folder parity: {v}");
    assert_eq!(
        p["last_modified_by"]["role"], "checker",
        "folder parity: {v}"
    );
    assert!(p["created_by"]["timestamp"].as_i64().unwrap() > 0);
}

/// Current `_hash` of an entity via a plain read.
fn read_hash_of(harness: &mut WireHarness, id: &str) -> Value {
    let r = harness.call_tool("memstead_entity", json!({ "id": id }));
    r["structuredContent"]["_hash"].clone()
}

/// Lazy-mount regression (flywheel W7/01 final grade): `memstead_entity`
/// with `include_relations: true` renders INCOMING edges, which can
/// originate in any mem — so that form must take the full lazy-mount
/// load. The refuted first cut scoped the reload to the target's mem and
/// silently dropped incoming edges from unloaded lazy mems (14 of 25 on
/// the dogfood bench). This pins the fix: an incoming cross-mem edge
/// from a LAZY, not-yet-loaded mem appears in the answer.
#[test]
fn full_entity_include_relations_sees_incoming_edges_from_lazy_mems() {
    use memstead_base::WorkspaceStoreAdapter;

    let tmp = TempDir::new().unwrap();
    // Cross-mem edges are default-deny; grant linker → target so the
    // fixture edge can land.
    let toml = format!("{WORKSPACE_TOML_BODY}\n[cross_mem_links]\nlinker = [\"target\"]\n");
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("linker", "default@1.0.0"), ("target", "default@1.0.0")],
        &toml,
    );

    // Author the cross-mem edge while both mems are eager.
    let (from, to) = {
        let mut harness = WireHarness::start(tmp.path());
        let create = |h: &mut WireHarness, mem: &str, title: &str| -> String {
            let r = h.call_tool(
                "memstead_create",
                json!({
                    "mem": mem,
                    "title": title,
                    "entity_type": "spec",
                    "sections": { "identity": "seed", "purpose": "seed" },
                }),
            );
            r["structuredContent"]["id"]
                .as_str()
                .expect("create returns id")
                .to_string()
        };
        let from = create(&mut harness, "linker", "Source");
        let to = create(&mut harness, "target", "Destination");
        let rel = harness.call_tool(
            "memstead_relate",
            json!({ "relations": [{ "from": from, "to": to, "type": "USES" }] }),
        );
        assert!(
            rel["structuredContent"]["results"][0]["to"].is_string(),
            "cross-mem relate must land for this pin to mean anything: {rel}"
        );
        (from, to)
    };

    // Flip the LINKER mem to lazy — the incoming edge's origin is now a
    // deferred mount on the next boot.
    let mut workspace = memstead_base::FileWorkspaceStore::new()
        .load(tmp.path())
        .unwrap();
    for mount in &mut workspace.mounts {
        if mount.mem == "linker" {
            mount.lifecycle = memstead_base::MountLifecycle::Lazy;
        }
    }
    memstead_base::FileWorkspaceStore::new()
        .save_state(tmp.path(), &workspace)
        .unwrap();

    // Fresh boot: linker is deferred; the include_relations read of the
    // TARGET must still surface the incoming edge from linker.
    let mut harness = WireHarness::start(tmp.path());
    let r = harness.call_tool(
        "memstead_entity",
        json!({ "id": to, "include_relations": true }),
    );
    let rendered = r["content"][0]["text"].as_str().unwrap_or_default();
    let structured = serde_json::to_string(&r["structuredContent"]).unwrap_or_default();
    assert!(
        rendered.contains(&from) || structured.contains(&from),
        "the incoming cross-mem edge from the lazy mem must appear — a partial-store \
         answer is the refuted defect. rendered:\n{rendered}\nstructured:\n{structured}"
    );
}
