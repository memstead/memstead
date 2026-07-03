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
    std::fs::write(memstead.join("state").join("mounts.json"), MOUNTS_JSON_BODY_EMPTY).unwrap();
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
        self.send_notification(
            "notifications/initialized",
            json!({}),
        );
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
/// today (see `lean_memstead_entity_*` vs `pro_memstead_entity_*`). The
/// wire-byte-identity contract is *per-flavor*, not inter-flavor, so
/// each pin records its own server's current bytes.
fn assert_error_envelope(result: &Value, expected_code: &str, expected_message: &str) {
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
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
fn pro_memstead_entity_emits_typed_envelope_for_missing_id() {
    let tmp = TempDir::new().unwrap();
    seed_empty_workspace(tmp.path());
    // Full boot checks `<workspace>/mem-repo/.git` shape on startup —
    // seed a real bare repo with `main` + `__MEMSTEAD` refs.
    memstead_git_branch::test_support::init_real_mem_repo(tmp.path(), &[]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--does-not-exist" }),
    );
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
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
    assert!(!is_error, "expected success but got isError=true: {result}");
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .expect("content[] missing — wire envelope drifted");
    assert!(!content.is_empty(), "content[] empty — wire envelope drifted");
    let first = &content[0];
    let kind = first.get("type").and_then(Value::as_str).unwrap_or_default();
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
fn pro_memstead_search_succeeds_on_empty_seeded_workspace() {
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
fn pro_memstead_overview_succeeds_on_empty_seeded_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_overview", json!({}));
    let text = assert_success_envelope(&result);
    for anchor in ["## Mems", "## Schemas", "## Communities", "## Lifecycle Namespaces"] {
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
fn pro_memstead_schema_unknown_name_emits_entity_not_found() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_schema",
        json!({ "name": "not-a-schema" }),
    );
    assert_error_envelope(
        &result,
        "ENTITY_NOT_FOUND",
        "schema not found: \"not-a-schema\"",
    );
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
fn pro_memstead_create_returns_typed_success_envelope() {
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
fn pro_memstead_create_unknown_type_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_create",
        json!({ "title": "X", "entity_type": "totally-not-a-type" }),
    );

    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
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
fn pro_memstead_health_succeeds_on_seeded_workspace() {
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
fn pro_memstead_changes_since_bad_cursor_returns_invalid_cursor() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);
    let mut harness = WireHarness::start(tmp.path());

    let bad = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let result = harness.call_tool(
        "memstead_changes_since",
        json!({ "mem": "demo", "since": bad }),
    );
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
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
        sc.get("details").and_then(|d| d.get("since")).and_then(Value::as_str),
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
fn pro_default_writable_mem_is_stable_after_second_mem() {
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
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
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
fn pro_memstead_update_stale_hash_emits_typed_envelope() {
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
fn pro_memstead_update_succeeds_and_rotates_hash() {
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
fn pro_memstead_delete_succeeds_and_entity_becomes_unreadable() {
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
fn pro_memstead_relate_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (from, _) = create_and_get_id_hash(&mut harness, "Source");
    let (to, _) = create_and_get_id_hash(&mut harness, "Target");

    let result = harness.call_tool(
        "memstead_relate",
        json!({ "from": from, "to": to, "type": "USES" }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on relate success");
    assert_eq!(
        body.get("from").and_then(Value::as_str),
        Some(from.as_str()),
        "relate `from` drifted: {body}"
    );
    assert_eq!(
        body.get("to").and_then(Value::as_str),
        Some(to.as_str()),
        "relate `to` drifted: {body}"
    );
    // Full uses `rel_type` (not `type`). USES (not REFERENCES) — explicit
    // author of REFERENCES is refused under the default schema's
    // `alias_target_rel_type` pointer; this test pins the envelope
    // shape, not the rel-type specifically.
    assert_eq!(
        body.get("rel_type").and_then(Value::as_str),
        Some("USES"),
        "full relate `rel_type` drifted: {body}"
    );
    assert!(
        body.get("type").is_none(),
        "full must not carry `type` (lean field name): {body}"
    );
    // Full omits `action` — the lean surface carries it.
    assert!(
        body.get("action").is_none(),
        "full unexpectedly carries `action`: {body}"
    );
}

// ---------------------------------------------------------------------------
// `memstead_rename` pins — success + RENAME_NO_OP
// ---------------------------------------------------------------------------


/// Full pin: same flow.
#[test]
fn pro_memstead_rename_returns_typed_success_envelope() {
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
fn pro_memstead_rename_same_slug_emits_typed_warning() {
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
fn pro_memstead_reload_returns_typed_success_envelope() {
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

fn assert_has_incoming_refs_envelope(
    result: &Value,
    expected_target: &str,
    expected_source: &str,
) {
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
    assert!(is_error, "expected isError on delete with referrers: {result}");
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
fn pro_memstead_delete_with_incoming_refs_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, _) = create_and_get_id_hash(&mut harness, "Referrer");
    let (target, target_hash) = create_and_get_id_hash(&mut harness, "Referenced");

    let relate = harness.call_tool(
        "memstead_relate",
        json!({ "from": source, "to": target, "type": "USES" }),
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
fn pro_memstead_changes_since_returns_typed_success_envelope() {
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
fn pro_memstead_changes_since_wide_window_uses_authoritative_rename_map() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());

    // Step 1: seed the workspace with all the entities that exist
    // BEFORE the cursor — pre-rename adjacent entities and the
    // rename target. Then capture the cursor SHA. Anything that
    // happens after this is "inside the polling window".
    let (rename_id, _) =
        create_and_get_id_hash(&mut harness, "Leading And Trailing Whitespace");
    // Adjacent unrelated entities — bodies share enough lexical
    // mass with the rename target that gix's similarity scorer can
    // mispair them over the wide window (the F16 trip).
    let (other_a, _) = create_and_get_id_hash(&mut harness, "Adjacent Memo Alpha");
    let (other_b, _) = create_and_get_id_hash(&mut harness, "Adjacent Memo Beta");

    // Re-read the rename target so we have a fresh hash for the
    // rename call (the post-create hash, which is still current
    // because nothing has touched the target since).
    let entity_read = harness.call_tool(
        "memstead_entity",
        json!({ "id": rename_id }),
    );
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
    let last_seed_create = harness.call_tool(
        "memstead_entity",
        json!({ "id": other_b }),
    );
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
    let entity_a_read = harness.call_tool(
        "memstead_entity",
        json!({ "id": other_a }),
    );
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
        .filter(|ev| {
            ev.get("action").and_then(Value::as_str) == Some("renamed")
        })
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
        let from_id = ev.get("from_id").and_then(Value::as_str).unwrap_or_default();
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
fn pro_memstead_changes_since_include_notes_false_strips_notes_and_memstead_ref() {
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
fn pro_memstead_entity_returns_structured_envelope_alongside_markdown() {
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
    assert_eq!(
        body.get("id").and_then(Value::as_str),
        Some(id.as_str()),
    );
    assert_eq!(
        body.get("mem").and_then(Value::as_str),
        Some("demo"),
    );
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
        body.get("relationships").and_then(Value::as_array).is_some(),
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
fn pro_memstead_search_returns_structured_envelope_alongside_markdown() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let _ = create_and_get_id_hash(&mut harness, "Authorization Flow");
    let _ = create_and_get_id_hash(&mut harness, "Anchor Memo");

    let result = harness.call_tool(
        "memstead_search",
        json!({ "query": { "any": ["Anchor"] } }),
    );
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
fn pro_memstead_entity_structured_relationships_carry_typed_shape() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (from, _) = create_and_get_id_hash(&mut harness, "Rel Source");
    let (to, _) = create_and_get_id_hash(&mut harness, "Rel Target");
    let _ = harness.call_tool(
        "memstead_relate",
        json!({ "from": from, "to": to, "type": "PART_OF" }),
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
    assert_eq!(
        rel.get("rel_type").and_then(Value::as_str),
        Some("PART_OF"),
    );
    assert_eq!(
        rel.get("target").and_then(Value::as_str),
        Some(to.as_str()),
    );
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
fn pro_memstead_changes_since_include_notes_true_carries_notes_and_rename_note() {
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
        notes.iter().any(|n| {
            n.get("tool_verb").and_then(Value::as_str) == Some("rename")
        }),
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
fn pro_auto_stub_then_update_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, _) = create_and_get_id_hash(&mut harness, "Source");
    let stub_id = "demo--ghost";

    let relate = harness.call_tool(
        "memstead_relate",
        json!({ "from": source, "to": stub_id, "type": "USES" }),
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
    let is_error = update.get("isError").and_then(Value::as_bool).unwrap_or(false);
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
fn pro_rename_stub_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, _) = create_and_get_id_hash(&mut harness, "Source");
    let stub_id = "demo--ghost";

    let _ = harness.call_tool(
        "memstead_relate",
        json!({ "from": source, "to": stub_id, "type": "USES" }),
    );

    let rename = harness.call_tool(
        "memstead_rename",
        json!({ "id": stub_id, "new_title": "Promoted", "expected_hash": "" }),
    );
    let is_error = rename.get("isError").and_then(Value::as_bool).unwrap_or(false);
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
fn pro_relate_from_stub_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (source, _) = create_and_get_id_hash(&mut harness, "Real");
    let stub_id = "demo--ghost";

    let _ = harness.call_tool(
        "memstead_relate",
        json!({ "from": source, "to": stub_id, "type": "USES" }),
    );

    let result = harness.call_tool(
        "memstead_relate",
        json!({ "from": stub_id, "to": source, "type": "USES" }),
    );
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
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
fn pro_memstead_mem_create_returns_typed_success_envelope() {
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
fn pro_memstead_mem_delete_returns_typed_success_envelope() {
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
    let del = harness.call_tool(
        "memstead_mem_delete",
        json!({ "name": "ephemeral" }),
    );
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
fn pro_mem_delete_preserves_allowlist_rules_so_recreate_succeeds() {
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
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("demo", "default@1.0.0")],
        WORKSPACE_TOML,
    );

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
    let del = harness.call_tool(
        "memstead_mem_delete",
        json!({ "name": "ephemeral" }),
    );
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
fn pro_operator_mode_bypasses_empty_allowlist_via_mcp() {
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
        let del = harness.call_tool(
            "memstead_mem_delete",
            json!({ "name": "fresh" }),
        );
        let _ = assert_success_envelope(&del);
    }
}

/// Item 01 pin: `memstead_overview` surfaces the operator-mode posture so
/// anyone reading the engine's output can confirm the bypass is in
/// force. The disclosure lives under `## Lifecycle Namespaces`, where
/// the allowlist policy itself is rendered — colocating the policy
/// and its bypass posture keeps the surface coherent.
#[test]
fn pro_memstead_overview_surfaces_operator_mode_bypass() {
    const WORKSPACE_TOML: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
";

    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("demo", "default@1.0.0")],
        WORKSPACE_TOML,
    );

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
fn pro_memstead_mem_create_writes_refs_heads_branch_in_mounts_json() {
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

    let mounts_json_path = tmp.path().join(".memstead").join("state").join("mounts.json");
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
fn pro_memstead_relate_with_forbidden_description_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace(tmp.path(), &[("demo", "default@1.0.0")]);

    let mut harness = WireHarness::start(tmp.path());
    let (from, _) = create_and_get_id_hash(&mut harness, "Forbid Source");
    let (to, _) = create_and_get_id_hash(&mut harness, "Forbid Target");

    let result = harness.call_tool(
        "memstead_relate",
        json!({
            "from": from,
            "to": to,
            "type": "REFERENCES",
            "description": "should be refused",
        }),
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
fn pro_memstead_update_body_wikilink_auto_synthesises_alias_relation() {
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
// MCP wire tests for the six
// `memstead_workspace_*` tools wrapping the engine-located
// `workspace_config_edit` writers. Closes the F7 dynamic-mem-
// lifecycle gap from MCP — an agent can now grant, mutate, revoke,
// and delete without dropping to CLI.
// ---------------------------------------------------------------------------

const TIER_C_WORKSPACE_TOML: &str = "\
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

/// `memstead_workspace_grant_cross_link` writes the
/// `[cross_mem_links]` section. Round-trip: invoke the tool, read
/// `.memstead/workspace.toml` back, assert the grant appears.
#[test]
fn pro_memstead_workspace_grant_cross_link_round_trip() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("source", "default@1.0.0"), ("target", "default@1.0.0")],
        TIER_C_WORKSPACE_TOML,
    );

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_workspace_grant_cross_link",
        json!({ "from": "source", "to": "target" }),
    );
    let _ = assert_success_envelope(&result);

    let body = std::fs::read_to_string(tmp.path().join(".memstead").join("workspace.toml")).unwrap();
    assert!(
        body.contains("[cross_mem_links]"),
        "grant must write the cross_mem_links section; got:\n{body}",
    );
    assert!(
        body.contains("source = [\"target\"]"),
        "grant must record the source → [target] entry; got:\n{body}",
    );
}

/// `memstead_workspace_grant_cross_link` is idempotent.
/// Re-granting an existing grant returns success with
/// `GRANT_ALREADY_PRESENT` warning and leaves the file unchanged.
#[test]
fn pro_memstead_workspace_grant_cross_link_idempotent_with_warning() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("source", "default@1.0.0"), ("target", "default@1.0.0")],
        TIER_C_WORKSPACE_TOML,
    );
    let mut harness = WireHarness::start(tmp.path());
    let _ = harness.call_tool(
        "memstead_workspace_grant_cross_link",
        json!({ "from": "source", "to": "target" }),
    );
    let body_before =
        std::fs::read_to_string(tmp.path().join(".memstead").join("workspace.toml")).unwrap();
    let result = harness.call_tool(
        "memstead_workspace_grant_cross_link",
        json!({ "from": "source", "to": "target" }),
    );
    let text = assert_success_envelope(&result);
    let body_after =
        std::fs::read_to_string(tmp.path().join(".memstead").join("workspace.toml")).unwrap();
    assert_eq!(
        body_before, body_after,
        "duplicate grant must not rewrite the file",
    );
    let structured = result.get("structuredContent").expect("structuredContent missing");
    let warnings = structured
        .get("warnings")
        .and_then(Value::as_array)
        .expect("warnings array missing");
    assert!(
        warnings.iter().any(|w| w
            .get("code")
            .and_then(Value::as_str)
            == Some("GRANT_ALREADY_PRESENT")),
        "duplicate grant must emit GRANT_ALREADY_PRESENT in the warnings array; got:\n{structured}\n(text: {text})",
    );
}

/// `memstead_workspace_revoke_cross_link` of an absent grant
/// is idempotent: returns success with `GRANT_NOT_FOUND` warning.
#[test]
fn pro_memstead_workspace_revoke_cross_link_idempotent_when_absent() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("source", "default@1.0.0"), ("target", "default@1.0.0")],
        TIER_C_WORKSPACE_TOML,
    );
    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_workspace_revoke_cross_link",
        json!({ "from": "source", "to": "target" }),
    );
    let _ = assert_success_envelope(&result);
    let structured = result.get("structuredContent").expect("structuredContent missing");
    let warnings = structured
        .get("warnings")
        .and_then(Value::as_array)
        .expect("warnings array missing");
    assert!(
        warnings.iter().any(|w| w
            .get("code")
            .and_then(Value::as_str)
            == Some("GRANT_NOT_FOUND")),
        "no-op revoke must emit GRANT_NOT_FOUND in the warnings array; got:\n{structured}",
    );
}

/// `memstead_workspace_allow_create` writes a new rule.
/// Round-trip: invoke the tool, parse the workspace TOML, assert
/// the new rule appears in `[[mem_management.create]]`.
#[test]
fn pro_memstead_workspace_allow_create_round_trip() {
    // Seed with empty rules — exercise the "append first rule" path.
    const EMPTY_TOML: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
";
    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(tmp.path(), &[("seed", "default@1.0.0")], EMPTY_TOML);

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_workspace_allow_create",
        json!({
            "pattern": "exec-*",
            "schemas": ["default@1.0.0"],
        }),
    );
    let _ = assert_success_envelope(&result);

    let body = std::fs::read_to_string(tmp.path().join(".memstead").join("workspace.toml")).unwrap();
    assert!(
        body.contains("[[mem_management.create]]"),
        "allow_create must write the section header; got:\n{body}",
    );
    assert!(
        body.contains("pattern = \"exec-*\""),
        "allow_create must record the pattern; got:\n{body}",
    );
}

/// MCP F3 — re-adding an existing `allow_create` pattern with a
/// different `schemas` set is refused with `RULE_EXISTS_SCHEMAS_DIFFER`
/// (not a deceptive success echoing a change that did not land); the
/// stored pins are unchanged, and an identical re-add stays the no-op.
#[test]
fn pro_allow_create_differing_schemas_refused_stored_unchanged() {
    const EMPTY_TOML: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
";
    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(tmp.path(), &[("seed", "default@1.0.0")], EMPTY_TOML);

    let mut harness = WireHarness::start(tmp.path());

    // First add pins `scratch` to software@0.1.0.
    let first = harness.call_tool(
        "memstead_workspace_allow_create",
        json!({ "pattern": "scratch", "schemas": ["software@0.1.0"] }),
    );
    let _ = assert_success_envelope(&first);

    // Re-add with a different schema set → typed refusal, not success.
    let differ = harness.call_tool(
        "memstead_workspace_allow_create",
        json!({ "pattern": "scratch", "schemas": ["nonexistent@9.9.9"] }),
    );
    assert!(
        differ.get("isError").and_then(Value::as_bool).unwrap_or(false),
        "differing-schemas re-add must be an error envelope: {differ}",
    );
    let structured = differ.get("structuredContent").expect("structured payload present");
    assert_eq!(structured["code"], "RULE_EXISTS_SCHEMAS_DIFFER", "payload: {structured}");
    assert_eq!(
        structured["details"]["stored_schemas"],
        json!(["software@0.1.0"]),
        "refusal names the stored schemas: {structured}",
    );
    assert_eq!(
        structured["details"]["requested_schemas"],
        json!(["nonexistent@9.9.9"]),
        "refusal names the requested schemas: {structured}",
    );
    assert!(
        structured["details"]["recovery"].as_str().is_some_and(|s| s.contains("revoke")),
        "refusal points at the revoke-then-readd recovery: {structured}",
    );

    // The stored rule is unchanged — still software@0.1.0.
    let body = std::fs::read_to_string(tmp.path().join(".memstead").join("workspace.toml")).unwrap();
    assert!(body.contains("software@0.1.0"), "stored pins stay put; got:\n{body}");
    assert!(!body.contains("nonexistent@9.9.9"), "rejected pins not written; got:\n{body}");

    // An identical re-add is still the idempotent no-op (success).
    let same = harness.call_tool(
        "memstead_workspace_allow_create",
        json!({ "pattern": "scratch", "schemas": ["software@0.1.0"] }),
    );
    let _ = assert_success_envelope(&same);
}

/// Dynamic mem lifecycle end-to-end via MCP only. Mirrors the
/// workflow named
/// in the tool descriptions: create a target mem, grant the
/// source mem permission to link into it, revoke the grant, then
/// delete the target. No CLI calls.
#[test]
fn pro_f7_dynamic_mem_lifecycle_completes_via_mcp_only() {
    let tmp = TempDir::new().unwrap();
    seed_full_workspace_with_toml(
        tmp.path(),
        &[("source", "default@1.0.0")],
        TIER_C_WORKSPACE_TOML,
    );

    let mut harness = WireHarness::start_with_args(tmp.path(), &["--operator-mode"]);

    // 1. Create the target mem.
    let create = harness.call_tool(
        "memstead_mem_create",
        json!({
            "name": "target",
            "location": "mems/target",
            "schema": "default@1.0.0",
        }),
    );
    let _ = assert_success_envelope(&create);

    // 2. Grant source → target permission.
    let grant = harness.call_tool(
        "memstead_workspace_grant_cross_link",
        json!({ "from": "source", "to": "target" }),
    );
    let _ = assert_success_envelope(&grant);

    // 3. Revoke the grant before deleting (otherwise step 4 would
    //    refuse with MEM_REFERENCED_BY_POLICY — the safeguard the
    //    policy-check gates on delete_files=true).
    let revoke = harness.call_tool(
        "memstead_workspace_revoke_cross_link",
        json!({ "from": "source", "to": "target" }),
    );
    let _ = assert_success_envelope(&revoke);

    // 4. Delete the target mem. delete_files=true now succeeds
    //    because the cross-link grant was revoked in step 3.
    let delete = harness.call_tool(
        "memstead_mem_delete",
        json!({ "name": "target" }),
    );
    let _ = assert_success_envelope(&delete);
}
