//! The supported library surface, used the way a program that links the
//! engine uses it.
//!
//! Of the engine crates this file imports `memstead_mcp::library` and
//! nothing else, and it names every item of the closed list that module
//! states: removing or renaming one stops this file from compiling.
//! What it brings itself is what such a program brings itself: a
//! transport (an in-memory pipe here) and the `rmcp` service loop.

use std::collections::HashSet;
use std::path::Path;

use memstead_mcp::library::{
    Actor, BootError, CallerKey, ClientId, DEFAULT_TOKEN_BUDGET, Engine, FullEngineError,
    McpServer, MemDeleteParams, MemDeleteResponse, MemMountParams, MemMountResponse, MemRepoInit,
    MemRepoInitError, MountCapability, MountSource, SchemaRef, delete_mem,
    engine_from_workspace_root, init_mem_repo, mount_mem,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Lay a mem-repo workspace through the module and admit every mem
/// name, the way an operator's `workspace.toml` would.
fn lay_workspace(root: &Path) -> Result<MemRepoInit, MemRepoInitError> {
    let laid = init_mem_repo(root)?;
    let mut policy = std::fs::read_to_string(&laid.workspace_toml).unwrap();
    policy.push_str("\n[[mem_management.create]]\npattern = \"*\"\nschemas = [\"*\"]\n");
    std::fs::write(&laid.workspace_toml, policy).unwrap();
    Ok(laid)
}

fn boot(root: &Path) -> Result<Engine, BootError> {
    engine_from_workspace_root(root)
}

/// Serve `server` over an in-memory pipe, send `calls` as tool calls
/// after the handshake, and hand back each call's result.
fn serve_and_call(
    server: McpServer,
    calls: Vec<(&'static str, serde_json::Value)>,
) -> Vec<serde_json::Value> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (server_read, server_write) = tokio::io::split(server_io);
        tokio::spawn(async move {
            let service = rmcp::ServiceExt::serve(server, (server_read, server_write))
                .await
                .expect("the server serves over the program's transport");
            let _ = service.waiting().await;
        });
        let (client_read, mut client_write) = tokio::io::split(client_io);
        let mut lines = BufReader::new(client_read).lines();
        let mut next_id = 0u64;
        let mut results = Vec::new();
        let mut requests = vec![(
            "initialize".to_string(),
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "a-linking-program", "version": "1" }
            }),
        )];
        for (name, arguments) in calls {
            requests.push((
                "tools/call".to_string(),
                serde_json::json!({ "name": name, "arguments": arguments }),
            ));
        }
        for (index, (method, params)) in requests.into_iter().enumerate() {
            next_id += 1;
            let message = serde_json::json!({
                "jsonrpc": "2.0", "id": next_id, "method": method, "params": params
            });
            client_write
                .write_all(format!("{message}\n").as_bytes())
                .await
                .unwrap();
            let result = loop {
                let line = lines.next_line().await.unwrap().expect("a response");
                let value: serde_json::Value = serde_json::from_str(&line).unwrap();
                if value["id"] == next_id {
                    break value["result"].clone();
                }
            };
            if index == 0 {
                client_write
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
                    .await
                    .unwrap();
            } else {
                results.push(result);
            }
        }
        results
    })
}

fn text_of(result: &serde_json::Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn a_program_builds_on_the_library_module_alone() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let view = tmp.path().join("view");

    // A workspace laid through the module, and an engine booted over it.
    let laid = lay_workspace(&store).expect("the mem-repo is laid");
    assert!(laid.mem_repo_dir.join(".git").is_dir());
    assert!(matches!(
        init_mem_repo(&store),
        Err(MemRepoInitError::Exists(_))
    ));
    let engine: Engine = boot(&laid.workspace_root).expect("the engine boots");

    // The server over the engine it owns, with one tool filtered out,
    // and the tool list it serves.
    let disabled: HashSet<String> = HashSet::from(["memstead_delete".to_string()]);
    let server = McpServer::new_with_filter(engine, DEFAULT_TOKEN_BUDGET, disabled, None);
    let listed: Vec<String> = server
        .filtered_tool_list()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(listed.iter().any(|name| name == "memstead_create"));
    assert!(!listed.iter().any(|name| name == "memstead_delete"));

    // A further instance over the same engine for one caller, bound to
    // a pinned identity. One write through it lands; a call that names
    // another identity is refused, which is what the pin is.
    let for_one_caller = server
        .for_caller(CallerKey::new("caller-one"))
        .with_default_identity(Some("writer-one".to_string()))
        .with_pin_identity(true);
    let results = serve_and_call(
        for_one_caller,
        vec![
            (
                "memstead_mem_create",
                serde_json::json!({
                    "name": "notes", "location": "notes", "schema": "default@1.3.0"
                }),
            ),
            (
                "memstead_create",
                serde_json::json!({
                    "mem": "notes", "title": "Written Through The Library", "entity_type": "spec",
                    "metadata": { "level": "M0" },
                    "sections": { "identity": "i", "purpose": "p" }
                }),
            ),
            (
                "memstead_create",
                serde_json::json!({
                    "mem": "notes", "title": "Under Another Name", "entity_type": "spec",
                    "metadata": { "level": "M0" },
                    "sections": { "identity": "i", "purpose": "p" },
                    "identity": "someone-else"
                }),
            ),
            (
                "memstead_entity",
                serde_json::json!({ "id": "notes--written-through-the-library" }),
            ),
        ],
    );
    assert_ne!(results[0]["isError"], true, "{}", text_of(&results[0]));
    assert_ne!(results[1]["isError"], true, "{}", text_of(&results[1]));
    assert_eq!(results[2]["isError"], true, "the pinned identity holds");
    assert!(
        text_of(&results[2]).contains("INVALID_IDENTITY"),
        "{}",
        text_of(&results[2])
    );
    assert!(
        text_of(&results[3]).contains("Written Through The Library"),
        "{}",
        text_of(&results[3])
    );
    // The first instance serves what the other one wrote, from the one
    // engine both share: the mem the other caller created is mounted
    // in it already. An instance over an engine of its own would learn
    // of the mem from the roster and say so.
    let read_back = serve_and_call(
        server,
        vec![(
            "memstead_entity",
            serde_json::json!({ "id": "notes--written-through-the-library" }),
        )],
    );
    assert_ne!(read_back[0]["isError"], true, "{}", text_of(&read_back[0]));
    assert!(
        !read_back[0].to_string().contains("MEM_ROSTER_CHANGED"),
        "the two instances share one engine: {}",
        read_back[0]
    );

    // A second workspace mounts the mem of the first, read-only, and
    // unregisters it again.
    lay_workspace(&view).expect("the second workspace is laid");
    let mut viewer = boot(&view).expect("the second engine boots");
    let pin: SchemaRef = "default@1.3.0".parse().expect("a schema pin");
    let mounted: Result<MemMountResponse, FullEngineError> = mount_mem(
        &mut viewer,
        MemMountParams {
            name: "notes".to_string(),
            source: MountSource::Branch {
                branch: "notes".to_string(),
                gitdir: Some(laid.mem_repo_dir.join(".git")),
            },
            capability: MountCapability::ReadOnly,
            schema: Some(pin),
            note: None,
            operator_mode: false,
            actor: Actor::App,
            client: Some(ClientId {
                name: "a-linking-program".to_string(),
                version: "1".to_string(),
            }),
        },
    );
    let mounted = mounted.expect("the mem mounts");
    assert_eq!(mounted.capability, "read-only");
    assert_eq!(mounted.schema_ref, "default@1.3.0");
    let unregistered: MemDeleteResponse = delete_mem(
        &mut viewer,
        MemDeleteParams {
            name: "notes".to_string(),
            delete_files: false,
            note: None,
            actor: Actor::App,
            client: None,
            operator_mode: true,
            detach_incoming: false,
        },
    )
    .expect("the mount is unregistered");
    assert!(unregistered.deleted_from_router && !unregistered.files_deleted);

    // The server over an engine it owns, unfiltered.
    let whole = McpServer::new(viewer, DEFAULT_TOKEN_BUDGET);
    assert!(
        whole
            .filtered_tool_list()
            .iter()
            .any(|tool| tool.name == "memstead_delete")
    );
}
