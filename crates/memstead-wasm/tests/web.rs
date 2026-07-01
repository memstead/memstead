//! Browser smoke tests for the `@memstead/wasm` JS bindings.
//!
//! Run with:
//!
//! ```ignore
//! wasm-pack test --chrome --headless engine/crates/memstead-wasm
//! ```
//!
//! Three smoke tests:
//!
//! - `from_snapshot_then_get_entity` — hydrate a `.mem` archive in
//!   wasm, look one entity up.
//! - `apply_commit_then_get_entity_after` — apply a `Modified`
//!   envelope, confirm the new body is visible.
//! - `search_refuses_with_typed_code` — calling `engine.search(...)`
//!   throws the `SEARCH_UNAVAILABLE_IN_WASM` envelope.
//!
//! `fixture.mem` is generated at build time by the crate's
//! [`build.rs`](../build.rs): a tiny two-entity folder vault
//! (`alpha`, `beta`) round-tripped through
//! `Engine::export_vault_to_bytes`. Embedding the bytes via
//! `include_bytes!` keeps the test self-contained — no fetch, no
//! filesystem.

#![cfg(target_arch = "wasm32")]

use memstead_wasm::Engine;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const FIXTURE_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fixture.mem"));

fn hydrate() -> Engine {
    Engine::from_snapshot(FIXTURE_BYTES.to_vec())
        .expect("fromSnapshot must succeed on the build-script fixture")
}

#[wasm_bindgen_test]
fn from_snapshot_then_get_entity() {
    let engine = hydrate();
    let v = engine
        .get_entity("specs--alpha")
        .expect("getEntity must not throw on a present id");
    assert!(
        !v.is_undefined(),
        "specs--alpha must be present after fromSnapshot"
    );
}

#[wasm_bindgen_test]
fn apply_commit_then_get_entity_after() {
    let mut engine = hydrate();

    // Hand-build a minimal CommitEnvelope JS object — same JSON shape
    // the bridge's `/commits` endpoint produces.
    let envelope_js = js_sys::eval(
        r#"({
            sha: "new-sha",
            parent: "",
            vault: "specs",
            timestamp: "2026-05-19T10:00:00Z",
            trailers: {},
            changes: [{
                op: "modified",
                path: "alpha.md",
                content: "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Alpha Renamed\n\n## Identity\n\nAlpha Renamed body\n"
            }]
        })"#,
    )
    .expect("eval envelope literal");

    engine
        .apply_commit(envelope_js)
        .expect("applyCommit must accept a well-formed envelope");

    let v = engine
        .get_entity("specs--alpha")
        .expect("getEntity must not throw");
    assert!(
        !v.is_undefined(),
        "specs--alpha must still exist after Modified apply"
    );
    // Title is the first `#`-heading — confirm it reflects the new
    // body by serialising back through JSON and parsing.
    let json = js_sys::JSON::stringify(&v).expect("stringify entity");
    let s: String = json.dyn_into::<js_sys::JsString>().unwrap().into();
    assert!(
        s.contains("Alpha Renamed"),
        "modified body must surface in the stored title; got {s}"
    );
}

#[wasm_bindgen_test]
fn search_refuses_with_typed_code() {
    let engine = hydrate();
    let err = engine
        .search(JsValue::NULL)
        .expect_err("search must refuse on wasm32");

    let code_js = js_sys::Reflect::get(&err, &JsValue::from_str("code")).unwrap_or(JsValue::NULL);
    let code: String = code_js
        .as_string()
        .expect("search refuse envelope must carry a string `code`");
    assert_eq!(
        code, "SEARCH_UNAVAILABLE_IN_WASM",
        "expected SEARCH_UNAVAILABLE_IN_WASM, got {code}"
    );
}
