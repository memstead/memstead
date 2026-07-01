//! Embedder integration test for the bridge HTTP surface.
//!
//! A mini axum router mounts the four canonical handlers under a
//! `/api/vaults/:name/...` path prefix behind a trivial mock-auth
//! layer. The test drives each endpoint via `tower::ServiceExt::oneshot`
//! against an in-memory `Router` and asserts the wire-format / HTTP
//! contract.
//!
//! Read-only by construction: the test never POSTs / PUTs / DELETEs
//! and the handlers themselves do not call any mutating engine
//! method.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use bytes::Bytes;
use http_body_util::BodyExt;
use memstead_base::storage::VaultWriter;
use memstead_base::vcs::CommitContext;
use memstead_bridge::{
    BridgeState, BuildConfig, CommitEnvelope, commits_handler, events_handler, head_handler,
    search_handler, snapshot_handler, SearchResult,
};
use memstead_git_branch::storage::git_tree::GitTreeVaultWriter;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tower::ServiceExt;

fn init_gitdir(tmp: &TempDir) -> PathBuf {
    let gitdir = tmp.path().join("vault-repo").join(".git");
    std::fs::create_dir_all(&gitdir).unwrap();
    gix::init_bare(&gitdir).unwrap();
    gitdir
}

fn body(title: &str) -> String {
    format!(
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\n{title}\n"
    )
}

fn commit(gitdir: &Path, branch: &str, file: &str, content: &str, subject: &str) -> String {
    let writer = GitTreeVaultWriter::new(
        gitdir.to_path_buf(),
        format!("refs/heads/{branch}"),
    );
    writer
        .write_entity(Path::new(file), content.as_bytes())
        .unwrap();
    writer
        .commit(subject, &CommitContext::internal())
        .unwrap()
}

fn engine_with_specs(gitdir: &Path) -> memstead_base::Engine {
    let mount = memstead_base::Mount {
        vault: "specs".to_string(),
        schema: Some(memstead_schema::SchemaRef::new("default", semver::Version::new(1, 0, 0))),
        storage: memstead_base::MountStorage::GitBranch {
            gitdir: gitdir.to_path_buf(),
            branch: "specs".to_string(),
        },
        capability: memstead_base::MountCapability::Write,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: true,
            migration_target: None,
        };
    let backend = memstead_git_branch::storage::instantiate_pro_backend(&mount).unwrap();
    let mut engine =
        memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap();
    engine.set_git_branch_ops(memstead_git_branch::storage::PRO_GIT_BRANCH_OPS);
    engine
}

/// Seed an `__MEMSTEAD` ref-branch entry so `Engine::export_vault_to_bytes`
/// can resolve a `VaultConfig` for the vault. Without this the snapshot
/// path refuses with `VAULT_CONFIG_INCOMPLETE`.
fn seed_vault_config(gitdir: &Path) {
    let writer = GitTreeVaultWriter::new(
        gitdir.to_path_buf(),
        "refs/heads/__MEMSTEAD".to_string(),
    );
    writer
        .write_entity(
            Path::new("vaults/specs/config.json"),
            br#"{"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
    writer
        .commit("seed config", &CommitContext::internal())
        .unwrap();
}

/// Mock authentication middleware: refuses requests without a
/// specific header. Demonstrates that the embedder layers auth in
/// front of the bridge handlers and the bridge itself does not
/// know about credentials.
async fn mock_auth(
    req: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req
        .headers()
        .get("x-mock-auth")
        .map(|v| v.as_bytes())
        != Some(b"yes")
    {
        return (StatusCode::UNAUTHORIZED, "forbidden").into_response();
    }
    next.run(req).await
}

use axum::response::IntoResponse;

fn build_router(state: BridgeState) -> Router {
    Router::new()
        .route("/api/vaults/:name/snapshot", get(snapshot_handler))
        .route("/api/vaults/:name/head", get(head_handler))
        .route("/api/vaults/:name/commits", get(commits_handler))
        .route("/api/vaults/:name/events", get(events_handler))
        .route("/api/vaults/:name/search", get(search_handler))
        .layer(axum::middleware::from_fn(mock_auth))
        .with_state(state)
}

fn auth_request(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("x-mock-auth", "yes")
        .body(Body::empty())
        .unwrap()
}

async fn read_body(response: axum::response::Response) -> Bytes {
    response.into_body().collect().await.unwrap().to_bytes()
}

#[tokio::test]
async fn auth_middleware_blocks_unauthenticated_requests() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    seed_vault_config(&gitdir);
    commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "seed");
    let engine = Arc::new(Mutex::new(engine_with_specs(&gitdir)));
    let state = BridgeState::new(engine);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/vaults/specs/head")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn head_endpoint_returns_current_sha() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    seed_vault_config(&gitdir);
    let sha = commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "seed");
    let engine = Arc::new(Mutex::new(engine_with_specs(&gitdir)));
    let state = BridgeState::new(engine);
    let app = build_router(state);

    let response = app
        .oneshot(auth_request("/api/vaults/specs/head"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_body(response).await;
    assert_eq!(std::str::from_utf8(&body).unwrap(), sha);
}

#[tokio::test]
async fn snapshot_endpoint_returns_archive_bytes_and_head_header() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    seed_vault_config(&gitdir);
    let sha = commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "seed");
    let engine = Arc::new(Mutex::new(engine_with_specs(&gitdir)));
    let state = BridgeState::new(engine);
    let app = build_router(state);

    let response = app
        .oneshot(auth_request("/api/vaults/specs/snapshot"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/zip"
    );
    assert_eq!(
        response
            .headers()
            .get("x-memstead-head")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default(),
        sha,
        "snapshot response must surface HEAD SHA on `x-memstead-head` for clients to persist as a cursor"
    );
    let bytes = read_body(response).await;
    assert!(!bytes.is_empty(), "snapshot body must carry archive bytes");
    // Snapshot hydrates: the bytes feed Engine::from_archive_bytes
    // and produce an engine with the same vault entity visible.
    let hydrated = memstead_base::Engine::from_archive_bytes(bytes.to_vec()).unwrap();
    let alpha = hydrated
        .get_entity(&memstead_base::EntityId::new("specs", "alpha"))
        .expect("alpha must round-trip through snapshot");
    assert_eq!(alpha.title, "Alpha");
}

#[tokio::test]
async fn commits_endpoint_returns_chronological_envelopes() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    seed_vault_config(&gitdir);
    let sha_a = commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "first");
    let sha_b = commit(&gitdir, "specs", "beta.md", &body("Beta"), "second");
    let engine = Arc::new(Mutex::new(engine_with_specs(&gitdir)));
    let state = BridgeState::new(engine);
    let app = build_router(state);

    let response = app
        .oneshot(auth_request(&format!(
            "/api/vaults/specs/commits?until={sha_b}"
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_body(response).await;
    let envs: Vec<CommitEnvelope> = serde_json::from_slice(&body).unwrap();
    assert_eq!(envs.len(), 2, "envelopes for both commits expected");
    assert_eq!(envs[0].sha, sha_a, "oldest first");
    assert_eq!(envs[1].sha, sha_b);
}

#[tokio::test]
async fn commits_endpoint_returns_404_for_unknown_since() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    seed_vault_config(&gitdir);
    commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "seed");
    let engine = Arc::new(Mutex::new(engine_with_specs(&gitdir)));
    let state = BridgeState::new(engine);
    let app = build_router(state);

    let response = app
        .oneshot(auth_request(
            "/api/vaults/specs/commits?since=0000000000000000000000000000000000000000",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = read_body(response).await;
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["code"], "UNKNOWN_COMMIT");
}

#[tokio::test]
async fn commits_endpoint_returns_409_for_delta_too_large() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    seed_vault_config(&gitdir);
    let mut last = String::new();
    for i in 0..5 {
        last = commit(
            &gitdir,
            "specs",
            &format!("entity-{i}.md"),
            &body(&format!("E{i}")),
            &format!("c{i}"),
        );
    }
    let engine = Arc::new(Mutex::new(engine_with_specs(&gitdir)));
    let state = BridgeState::new(engine).with_config(BuildConfig {
        delta_limit: 2,
        ..BuildConfig::default()
    });
    let app = build_router(state);

    let response = app
        .oneshot(auth_request(&format!(
            "/api/vaults/specs/commits?until={last}"
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = read_body(response).await;
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["code"], "DELTA_TOO_LARGE");
    assert!(envelope["details"]["n_commits"].as_u64().unwrap() > 2);
}

#[tokio::test]
async fn allowlist_blocks_unlisted_vaults_with_404() {
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    seed_vault_config(&gitdir);
    commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "seed");
    let engine = Arc::new(Mutex::new(engine_with_specs(&gitdir)));
    let state = BridgeState::new(engine).with_allowlist(vec!["other".to_string()]);
    let app = build_router(state);

    let response = app
        .oneshot(auth_request("/api/vaults/specs/head"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = read_body(response).await;
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["code"], "UNKNOWN_VAULT");
}

#[tokio::test]
async fn events_endpoint_emits_vault_changed_within_window() {
    // Subscribe to the SSE stream, perform an engine mutation, assert
    // the resulting SSE frame arrives within 2 seconds. The mutation
    // here is a `create_entity` through the same engine the SSE handler
    // shares — the engine's emit-on-commit hook drives the broadcast
    // the bridge listens on.
    let tmp = TempDir::new().unwrap();
    let gitdir = init_gitdir(&tmp);
    seed_vault_config(&gitdir);
    commit(&gitdir, "specs", "seed.md", &body("Seed"), "init");

    let engine = Arc::new(Mutex::new(engine_with_specs(&gitdir)));
    let state = BridgeState::new(engine.clone());
    let app = build_router(state);

    // Drive the request through `oneshot` so we hold the response
    // body — a tokio mpsc-shaped Body that we'll read for SSE frames.
    let response = app
        .oneshot(auth_request("/api/vaults/specs/events"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // SSE content-type is `text/event-stream`.
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.starts_with("text/event-stream"))
            .unwrap_or(false),
        "expected SSE content-type, got {:?}",
        response.headers().get(header::CONTENT_TYPE),
    );

    // Pull the response into a streamable body, then spawn the
    // mutation. The body is a futures-aware stream we read frame by
    // frame.
    let mut body_stream = response.into_body().into_data_stream();
    let engine_for_mutate = engine.clone();
    let mutator = tokio::spawn(async move {
        // Brief delay so the SSE subscriber lands before the emit
        // fires — engine emits sequentially on the mutation thread.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut e = engine_for_mutate.lock().await;
        let (actor, client) = (
            memstead_base::vcs::Actor::Cli,
            memstead_base::vcs::ClientId {
                name: "test".to_string(),
                version: "0".to_string(),
            },
        );
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "Live entity for SSE test.".to_string());
        sections.insert("purpose".to_string(), "Trigger a vault_changed event.".to_string());
        e.create_entity(
            memstead_base::CreateEntityArgs {
                vault: "specs".to_string(),
                title: "Live".to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: indexmap::IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    });

    // Read frames for up to 2 seconds. axum SSE encodes events as
    // `event: <name>\ndata: <payload>\n\n` so we just look for
    // "vault_changed" in the concatenated bytes.
    let mut accumulated = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let chunk = match tokio::time::timeout(remaining, futures_util::StreamExt::next(&mut body_stream)).await {
            Ok(Some(Ok(bytes))) => bytes,
            Ok(Some(Err(_))) => break,
            Ok(None) => break,
            Err(_) => break,
        };
        accumulated.extend_from_slice(&chunk);
        if String::from_utf8_lossy(&accumulated).contains("event: vault_changed") {
            break;
        }
    }
    mutator.await.unwrap();
    let body_str = String::from_utf8_lossy(&accumulated);
    assert!(
        body_str.contains("event: vault_changed"),
        "expected SSE vault_changed event within window; got: {body_str}",
    );
}

// ---------------------------------------------------------------------------
// /search endpoint
// ---------------------------------------------------------------------------

/// Seed a git-branch-backed `specs` vault containing two entities
/// (`alpha.md`, `beta.md`) ready to be queried via the search
/// endpoint. Returns the running engine. Each entity has a
/// distinctive body word so a precise query can isolate either one.
fn engine_with_searchable_entities(tmp: &TempDir) -> memstead_base::Engine {
    let gitdir = init_gitdir(tmp);
    seed_vault_config(&gitdir);
    let alpha_body = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nAlpha distinctive marker phrase.\n";
    let beta_body = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Beta\n\n## Identity\n\nBeta carries the singular keyword unobtainium for tests.\n";
    commit(&gitdir, "specs", "alpha.md", alpha_body, "alpha seed");
    commit(&gitdir, "specs", "beta.md", beta_body, "beta seed");
    engine_with_specs(&gitdir)
}

#[tokio::test]
async fn search_endpoint_returns_hits_in_wire_shape() {
    let tmp = TempDir::new().unwrap();
    let engine = Arc::new(Mutex::new(engine_with_searchable_entities(&tmp)));
    let state = BridgeState::new(engine);
    let app = build_router(state);

    let response = app
        .oneshot(auth_request("/api/vaults/specs/search?q=unobtainium"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
    );
    let body = read_body(response).await;
    let parsed: SearchResult = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed.vault, "specs");
    assert_eq!(parsed.query, "unobtainium");
    assert!(
        parsed.hits.iter().any(|h| h.id == "specs--beta"),
        "expected specs--beta to surface for the unobtainium query; got {:?}",
        parsed.hits.iter().map(|h| &h.id).collect::<Vec<_>>(),
    );
    assert!(parsed.total_matched >= 1);
}

#[tokio::test]
async fn search_endpoint_404s_for_unallowed_vault() {
    let tmp = TempDir::new().unwrap();
    let engine = Arc::new(Mutex::new(engine_with_searchable_entities(&tmp)));
    // The vault `specs` is mounted but the allowlist only permits
    // `something-else` — the search handler must refuse before
    // touching the engine.
    let state = BridgeState::new(engine).with_allowlist(vec!["something-else".to_string()]);
    let app = build_router(state);

    let response = app
        .oneshot(auth_request("/api/vaults/specs/search?q=alpha"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = read_body(response).await;
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["code"], "UNKNOWN_VAULT");
}

#[tokio::test]
async fn search_endpoint_refuses_empty_query_with_400() {
    let tmp = TempDir::new().unwrap();
    let engine = Arc::new(Mutex::new(engine_with_searchable_entities(&tmp)));
    let state = BridgeState::new(engine);
    let app = build_router(state);

    let response = app
        .oneshot(auth_request("/api/vaults/specs/search?q="))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = read_body(response).await;
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["code"], "INVALID_SEARCH_QUERY");
    assert!(envelope["details"].is_object());
}

#[tokio::test]
async fn search_endpoint_refuses_oversized_limit_with_400() {
    let tmp = TempDir::new().unwrap();
    let engine = Arc::new(Mutex::new(engine_with_searchable_entities(&tmp)));
    let state = BridgeState::new(engine).with_config(BuildConfig {
        search_max_limit: 5,
        ..BuildConfig::default()
    });
    let app = build_router(state);

    let response = app
        .oneshot(auth_request("/api/vaults/specs/search?q=alpha&limit=100"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = read_body(response).await;
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["code"], "INVALID_SEARCH_QUERY");
}

#[tokio::test]
async fn search_endpoint_blocks_unauthenticated_requests() {
    let tmp = TempDir::new().unwrap();
    let engine = Arc::new(Mutex::new(engine_with_searchable_entities(&tmp)));
    let state = BridgeState::new(engine);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/vaults/specs/search?q=alpha")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "embedder auth must run before the search handler",
    );
}

#[tokio::test]
async fn search_endpoint_hit_shape_matches_engine_search_hit_shape() {
    // The bridge's per-hit JSON matches the engine's own `SearchHit`
    // JSON for the same vault state. We resolve a hit via the bridge,
    // then run the same query directly through
    // `Engine::search` and assert the JSON shape (sorted field
    // names) on the first hit is equal.
    let tmp = TempDir::new().unwrap();
    let engine_arc = Arc::new(Mutex::new(engine_with_searchable_entities(&tmp)));
    let state = BridgeState::new(engine_arc.clone());
    let app = build_router(state);

    let response = app
        .oneshot(auth_request("/api/vaults/specs/search?q=unobtainium"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_body(response).await;
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let bridge_hit = &parsed["hits"][0];
    assert!(!bridge_hit.is_null(), "bridge search must return at least one hit");

    // Drive Engine::search with the same scope `run_search`
    // synthesises so the engine's per-hit JSON is comparable.
    let engine = engine_arc.lock().await;
    let scope = memstead_base::ops::SearchScope {
        query: Some(memstead_base::ops::Query {
            any: vec!["unobtainium".to_string()],
            not: Vec::new(),
            phrase: None,
            field: None,
        }),
        vault: Some("specs".to_string()),
        entity_type: None,
        limit: Some(20),
        offset: None,
        filters: Default::default(),
        range_filters: Default::default(),
        edge_type: None,
        related_to: None,
        depth: None,
        expand_via: None,
        expand_depth: None,
        stub: None,
        token_budget: None,
    };
    let engine_result = engine.search(&scope).expect("engine search must succeed");
    let engine_hit_json = serde_json::to_value(&engine_result.hits[0]).unwrap();

    // Both must agree on the discriminator fields a consumer
    // branches on. Equality of every primitive field below pins the
    // MCP-shape conformance contract: a future field rename on the
    // engine side that forgets to mirror through to the bridge
    // surfaces here, not in production traffic.
    for key in &["id", "title", "vault", "entity_type", "stub", "tokens"] {
        assert_eq!(
            bridge_hit[key], engine_hit_json[key],
            "field `{key}` must match between bridge and engine SearchHit JSON",
        );
    }
}
