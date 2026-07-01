#![cfg(not(feature = "vault-repo"))]
//! Wire-shape characterization for the MCP tool surface.
//!
//! This suite pins the bytes the server emits in `result.content[]` and
//! `result.structured_content` for representative tool calls. Both server
//! implementations live in this crate, gated by `vault-repo`: each
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
//!   4. If the path is flavor-specific, gate on `vault-repo`.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

const WORKSPACE_TOML_BODY: &str = "format = \"memstead-git-branch-1\"\n\n\
[persistence_adapter]\nname = \"file-two-layer\"\n";

const MOUNTS_JSON_BODY_EMPTY: &str = r#"{ "format": "memstead-mounts-1", "mounts": [] }"#;

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

/// Seed a single-vault folder workspace at `root` pinned to the
/// `default@1.0.0` builtin schema. **Basis-only fixture.** The mount is
/// `Write`/`Eager` and stores entities in `root` itself.
///
/// Pro flavor cannot use this — the pro binary discovers vaults from
/// `vault-repo/.git/` branches (not from the folder-mount state file)
/// and would boot with zero writable vaults against a folder-seeded
/// workspace. Pro tests use [`seed_pro_workspace`] instead.
fn seed_folder_workspace(root: &Path, vault_name: &str) {
    use memstead_base::WorkspaceStoreAdapter;
    use memstead_base::filesystem::config::{WorkspaceConfig, write_workspace_config};
    use memstead_schema::SchemaRef;

    let pin: SchemaRef = "default@1.0.0".parse().unwrap();
    let cfg = WorkspaceConfig::new(vault_name, pin.clone());
    write_workspace_config(root, &cfg).unwrap();

    let memstead = root.join(".memstead");
    std::fs::create_dir_all(memstead.join("state")).unwrap();
    std::fs::write(memstead.join("workspace.toml"), WORKSPACE_TOML_BODY).unwrap();

    let workspace = memstead_base::Workspace {
        mounts: vec![memstead_base::Mount {
            vault: vault_name.to_string(),
            schema: Some(pin),
            storage: memstead_base::MountStorage::Folder {
                path: root.to_path_buf(),
            },
            capability: memstead_base::MountCapability::Write,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }],
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
        let mut child = Command::new(memstead_mcp_bin())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
// Basis-flavor pins (FilesystemMcpServer)
// ---------------------------------------------------------------------------
//
// Run with `cargo nextest run --no-default-features -p memstead-mcp wire_shape`.

/// Shared assertion shape: every error envelope must carry `isError=true`,
/// the expected typed `code`, and a `message` matching the per-flavor
/// pinned text. Pre-extraction the two server files own independent
/// mappers (`FilesystemMcpServer::engine_op_error` vs
/// `McpServer::engine_err_unified`) — message text DRIFTS between them
/// today (see `basis_memstead_entity_*` vs `pro_memstead_entity_*`). The plan's
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

/// Basis pin: `memstead_entity` against a missing id on an empty-mounts
/// workspace flows through `FilesystemMcpServer::engine_op_error`'s
/// `ENTITY_NOT_FOUND` arm. The exact message bytes are pinned — they
/// MUST stay unchanged across the lift (the basis server file moves
/// crates but its emit semantics do not).
#[test]
fn basis_memstead_entity_emits_typed_envelope_for_missing_id() {
    let tmp = TempDir::new().unwrap();
    seed_empty_workspace(tmp.path());

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_entity",
        json!({ "id": "specs--does-not-exist" }),
    );
    // Basis mapper formats with lowercase "entity not found" — the
    // engine's Display string ("entity not found: {id}") passed through
    // verbatim. Diverges from the pro mapper's "Entity not found"
    // (which capitalises the leading word); inter-flavor drift
    // recorded.
    assert_error_envelope(
        &result,
        "ENTITY_NOT_FOUND",
        "entity not found: specs--does-not-exist",
    );
}

// ---------------------------------------------------------------------------
// Pro-flavor pins (McpServer)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Success-path pins — pin envelope SHAPE, not exact content
// ---------------------------------------------------------------------------
//
// Success responses carry markdown content (often dependent on dynamic
// state like vault counts or schema version names). Pinning every byte
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

/// Basis pin: `memstead_search` with no filter on an empty workspace
/// emits a frontmatter-only response carrying `_total: 0`, `_returned:
/// 0`, `_offset: 0`. No `# Search results` heading is emitted because
/// the result set is empty.
#[test]
fn basis_memstead_search_succeeds_on_empty_seeded_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_search", json!({}));
    let text = assert_success_envelope(&result);
    // Pinned frontmatter shape — `render_search_markdown` emits this
    // sub-anchor set on every call; the empty-result body has no
    // heading. Trips if the renderer changes its frontmatter keys.
    for marker in ["_total: 0", "_returned: 0", "_offset: 0"] {
        assert!(
            text.contains(marker),
            "search response missing {marker:?}: {text:?}"
        );
    }
}


/// Basis pin: `memstead_overview` on a seeded folder workspace produces
/// the cold-start markdown with the `## Vaults` and `## Schemas`
/// anchors. Empty graph → no community section.
#[test]
fn basis_memstead_overview_succeeds_on_empty_seeded_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_overview", json!({}));
    let text = assert_success_envelope(&result);
    assert!(
        text.contains("## Vaults"),
        "overview missing ## Vaults anchor: {text:?}"
    );
    assert!(
        text.contains("## Schemas"),
        "overview missing ## Schemas anchor: {text:?}"
    );
    assert!(
        text.contains("demo"),
        "overview missing vault name: {text:?}"
    );
}


// ---------------------------------------------------------------------------
// `memstead_schema` error pin — both flavors emit `ENTITY_NOT_FOUND` for
// names that don't match the workspace's pinned schema. Helps confirm
// the pre-extraction message divergence story applies symmetrically
// across tools, not just `memstead_entity`.
// ---------------------------------------------------------------------------

/// Basis pin: `memstead_schema(name="unknown")` against the default-pinned
/// workspace must emit `ENTITY_NOT_FOUND` with a message that names the
/// requested schema and what the workspace actually pins.
#[test]
fn basis_memstead_schema_unknown_name_emits_entity_not_found() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool(
        "memstead_schema",
        json!({ "name": "not-a-schema" }),
    );
    assert_error_envelope(
        &result,
        "ENTITY_NOT_FOUND",
        "schema not found: \"not-a-schema\" — workspace pins default@1.0.0",
    );
}


// ---------------------------------------------------------------------------
// Mutation pins — `memstead_create` success + UNKNOWN_ENTITY_TYPE error
// ---------------------------------------------------------------------------
//
// Success path: the create response carries a JSON body on
// `structured_content` whose `id` field is the slugified id, plus
// `title`, `vault`, `content_hash`, `commit_sha`, and `warnings`. The
// pins assert on field PRESENCE + the deterministic `id` slug; the
// hashes / commit shas are content-derived and pinning them would
// couple the suite to the markdown render exactly.

fn assert_create_success_shape(result: &Value, expected_id: &str, expected_vault: &str) {
    let _text = assert_success_envelope(result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on create success");
    for field in ["id", "title", "vault", "_hash", "warnings"] {
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
        body.get("vault").and_then(Value::as_str),
        Some(expected_vault),
        "create response vault drifted: {body}"
    );
}

/// Basis pin: `memstead_create` against a seeded folder workspace creates a
/// `default@1.0.0` spec and returns a success envelope. Pins the shape
/// of the response body, not the content-derived hashes.
#[test]
fn basis_memstead_create_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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


/// Basis pin: `memstead_create` with an unknown `entity_type` rejects with
/// `code=UNKNOWN_ENTITY_TYPE`. The message names the rejected type and
/// lists the declared types; an exact-string pin is too brittle for the
/// declared list (schema-version-dependent), so the assertion checks
/// `code` plus message substrings.
#[test]
fn basis_memstead_create_unknown_type_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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

/// Basis pin: `memstead_health` returns the engine's `HealthSummary`
/// serialised directly — `{missing_fields, orphan_count, stale_entities,
/// stub_count}`. There is **no** `writable_vaults` field on the basis
/// response shape, even though the pro flavor (and the agent-facing
/// contract documented in the tool description) does carry one.
///
/// **Drift discovered:** basis `memstead_health` ships a strict subset of
/// the pro response shape. The pro server returns a richer
/// `HealthReport` envelope with vault-level detail; basis returns only
/// the workspace-wide scalars from `compute_health`. The pin records
/// today's basis truth so the lift cannot regress it.
#[test]
fn basis_memstead_health_succeeds_on_seeded_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

    let mut harness = WireHarness::start(tmp.path());
    let result = harness.call_tool("memstead_health", json!({}));
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on health success");
    for field in ["missing_fields", "orphan_count", "stale_entities", "stub_count"] {
        assert!(
            body.get(field).is_some(),
            "basis health response missing {field:?}: {body}"
        );
    }
    // Today the basis surface does NOT include `writable_vaults`; pro
    // does. The pin trips if the field appears (or disappears).
    assert!(
        body.get("writable_vaults").is_none(),
        "basis health unexpectedly carries writable_vaults — \
         if this is intended, update the pin: {body}"
    );
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
/// `spec` type's required `identity` + `purpose` sections so every
/// downstream test lands a valid entity.
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

/// Basis pin: create then update with a deliberately-stale hash trips
/// HASH_MISMATCH; `details.current` carries the actual on-disk hash so
/// the caller can recover without re-reading.
#[test]
fn basis_memstead_update_stale_hash_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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


/// Basis pin: `memstead_update` with the right expected_hash and a section
/// body returns a success envelope. Pins the new content_hash field
/// presence + that the hash actually changed (modulo serialization).
#[test]
fn basis_memstead_update_succeeds_and_rotates_hash() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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


/// Basis pin: `memstead_delete` with the right expected_hash succeeds. The
/// post-delete envelope's exact shape is engine-derived; the pin
/// checks success + reading the entity back returns ENTITY_NOT_FOUND.
#[test]
fn basis_memstead_delete_succeeds_and_entity_becomes_unreadable() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

    let mut harness = WireHarness::start(tmp.path());
    let (id, hash) = create_and_get_id_hash(&mut harness, "Doomed");

    let del = harness.call_tool(
        "memstead_delete",
        json!({ "id": id, "expected_hash": hash }),
    );
    let _ = assert_success_envelope(&del);

    // Reading the deleted entity must now trip ENTITY_NOT_FOUND.
    let read = harness.call_tool("memstead_entity", json!({ "id": id }));
    assert_error_envelope(
        &read,
        "ENTITY_NOT_FOUND",
        &format!("entity not found: {id}"),
    );
}


// ---------------------------------------------------------------------------
// `memstead_relate` success pins
// ---------------------------------------------------------------------------

/// Basis pin: create two entities, then relate them with `USES`.
/// Basis emits a response with `from`, `to`, `type` (yes — the literal
/// field name is `type`, not `rel_type`), `action: "added"`, plus
/// `content_hash`. (REFERENCES would be refused under the default
/// schema's `alias_target_rel_type` pointer; this test pins the
/// wire shape, not the rel-type specifically.) The pro flavor uses different field names — see
/// `pro_memstead_relate_returns_typed_success_envelope` for the
/// inter-flavor drift recorded below.
#[test]
fn basis_memstead_relate_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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
    // **Drift recorded:** basis names the field `type`; pro names it `rel_type`.
    assert_eq!(
        body.get("type").and_then(Value::as_str),
        Some("USES"),
        "basis relate `type` drifted: {body}"
    );
    assert!(
        body.get("rel_type").is_none(),
        "basis must not carry `rel_type` (pro field name): {body}"
    );
    assert_eq!(
        body.get("action").and_then(Value::as_str),
        Some("added"),
        "basis relate `action` drifted: {body}"
    );
}


// ---------------------------------------------------------------------------
// `memstead_rename` pins — success + RENAME_NO_OP
// ---------------------------------------------------------------------------

/// Basis pin: rename an entity to a new title. The response carries
/// `old_id`, `new_id`, plus rotated `content_hash`. The slug rule
/// (`<vault>--<lower-kebab>`) is engine-internal so the expected new_id
/// is computable from the new title.
#[test]
fn basis_memstead_rename_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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


/// Basis pin: renaming to a title that slugs to the same id is a
/// SILENT NO-OP on basis today — the response is a plain success with
/// `old_id == new_id` and no warning hint. The `RENAME_NO_OP` typed
/// EngineError variant exists but the basis handler does not surface
/// it for the same-slug case.
///
/// **Drift recorded:** pro emits the same success but additionally
/// rides a `TITLE_NORMALIZED_TO_SLUG_NOOP` warning hint on the
/// response's `warnings[]` array (see the pro pin below). Basis emits
/// no such hint — same wire envelope shape minus the typed-warning
/// payload. Pending reconciliation on whichever flavor becomes the
/// canonical surface.
#[test]
fn basis_memstead_rename_same_slug_silent_noop() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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
        "old_id drifted: {body}"
    );
    assert_eq!(
        body.get("new_id").and_then(Value::as_str),
        Some(id.as_str()),
        "new_id should equal old_id for same-slug rename: {body}"
    );
    // Basis now surfaces `outcome.warnings` on the rename response (the
    // `require_notes` reconciliation lifted `NOTE_MISSING` into the
    // engine and routed every mutation's warnings to the wire). A
    // same-slug rename carries the typed slug-noop warning; pin its
    // presence and shape.
    let warnings = body
        .get("warnings")
        .and_then(Value::as_array)
        .expect("basis rename must now carry a warnings array");
    assert!(
        warnings
            .iter()
            .any(|w| w.get("code").and_then(Value::as_str) == Some("TITLE_NORMALIZED_TO_SLUG_NOOP")),
        "same-slug rename must surface TITLE_NORMALIZED_TO_SLUG_NOOP: {body}"
    );
}


// ---------------------------------------------------------------------------
// `memstead_reload` (pro-only) success pin
// ---------------------------------------------------------------------------
//
// The basis filesystem-vault server doesn't expose memstead_reload —
// drift-reload is a vault-repo concept (sibling writer commits a new
// HEAD; engine re-derives memo state). Pinning is pro-only.


// ---------------------------------------------------------------------------
// `memstead_delete` HAS_INCOMING_REFS pin — multi-step (create×2 → relate → delete)
// ---------------------------------------------------------------------------
//
// The recovery payload contract: `details.referrers[]` carries
// `{from_id, rel_type, vault, capability: "write"}` for each Write-Vault
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
        first.get("vault").and_then(Value::as_str).is_some(),
        "referrer.vault missing: {first}"
    );
}

/// Basis pin: create two entities, relate them, then try to delete the
/// referenced target. Expect HAS_INCOMING_REFS with the recovery payload
/// (`details.referrers[]` listing the source as the offending referrer).
#[test]
fn basis_memstead_delete_with_incoming_refs_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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
// Basis and pro use STRUCTURALLY different change-feeds: basis reads
// timestamp-keyed entries from `.memstead/changes.jsonl`; pro reads git
// commits between `since` and HEAD. The two response envelopes
// diverge — each pin records its flavor's shape per-flavor.

/// Basis pin: `memstead_changes_since` reads `.memstead/changes.jsonl` and emits
/// `{since, count, entries[]}`. Passing `since: ""` returns every
/// recorded change (basis filters with `ts > since`; empty since
/// admits all). The basis flavor IGNORES the `vault` param (the
/// JSONL changelog is workspace-global, not per-vault).
#[test]
fn basis_memstead_changes_since_returns_typed_success_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

    let mut harness = WireHarness::start(tmp.path());
    let _ = create_and_get_id_hash(&mut harness, "First");

    let result = harness.call_tool(
        "memstead_changes_since",
        json!({ "vault": "demo", "since": "" }),
    );
    let _ = assert_success_envelope(&result);
    let body = result
        .get("structuredContent")
        .expect("structuredContent missing on changes_since success");
    for field in ["since", "count", "entries"] {
        assert!(
            body.get(field).is_some(),
            "basis changes_since response missing {field:?}: {body}"
        );
    }
    let count = body.get("count").and_then(Value::as_u64).unwrap_or(0);
    assert!(
        count >= 1,
        "expected at least one changelog entry after create: {body}"
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

/// Basis pin: `memstead_relate` against an absent same-vault target
/// auto-stubs the target and emits a `AUTO_STUB_CREATED` warning on
/// the response's `warnings[]`. Try-then-update the stub trips
/// `STUB_NOT_UPDATABLE`.
#[test]
fn basis_auto_stub_then_update_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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

    // The stub now exists at stub_id with empty content_hash. Updating
    // it (with expected_hash="" per the stub contract) trips
    // STUB_NOT_UPDATABLE.
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


/// Basis pin: trying to rename a stub trips `STUB_NOT_RENAMABLE`. Uses
/// the same auto-stub bootstrap (relate to absent target) and then
/// asks for a rename. The agent's recovery is `memstead_create` to promote
/// the stub before renaming.
#[test]
fn basis_rename_stub_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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

/// Basis pin.
#[test]
fn basis_relate_from_stub_emits_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    seed_folder_workspace(tmp.path(), "demo");

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
// Pro-only vault-lifecycle pin — `memstead_vault_create` success
// ---------------------------------------------------------------------------


