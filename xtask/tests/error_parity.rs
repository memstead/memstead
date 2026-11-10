//! Every error the engine defines, rendered by the MCP server and by the
//! CLI's JSON output, compared field for field.
//!
//! (Lives in xtask, the one crate that depends on both surfaces; the MCP
//! crate must not depend on the CLI.) The engine's sample list (`memstead_base::engine::error::samples`) is
//! complete: a variant missing from its tag lists fails the build, and one
//! without a sample is refused below (`missing_samples`). This test
//! raises each sample through both surfaces' real mappers and records where
//! the code, the recovery payload or the message differ.
//!
//! Both surfaces render one engine-side value, so the code and the payload
//! are equal field for field and each message begins with the engine's.
//! On a failure the differences are printed (and written as markdown to
//! `$ERROR_PARITY_OUT` when set; `$ERROR_PARITY_SNAPSHOT` writes every
//! rendering as JSON).

use memstead_base::engine::error::samples::{
    every_engine_error, every_full_engine_error, missing_samples,
};
use memstead_base::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

fn engine(tmp: &std::path::Path) -> memstead_base::Engine {
    let dir = tmp.join("specs");
    std::fs::create_dir_all(dir.join(".memstead")).unwrap();
    std::fs::write(
        dir.join(".memstead/config.json"),
        r#"{"version":"0.1.0","schema":"default@1.0.0","mediums":{},"projections":{}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("alpha.md"),
        "---\ntype: spec\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA.\n\n## Purpose\n\nB.\n",
    )
    .unwrap();
    let mount = Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::Folder { path: dir },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let backend = memstead_base::instantiate_local_backend(&mount).unwrap();
    memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap()
}

struct Rendered {
    code: String,
    message: String,
    details: serde_json::Value,
}

fn mcp(e: memstead_base::EngineError, engine: &memstead_base::Engine) -> Rendered {
    decode_mcp(memstead_mcp::server::render_engine_error(e, engine))
}

fn decode_mcp(r: rmcp::model::CallToolResult) -> Rendered {
    let sc = r.structured_content.unwrap_or_default();
    Rendered {
        code: sc["code"].as_str().unwrap_or_default().to_string(),
        message: sc["message"].as_str().unwrap_or_default().to_string(),
        details: sc
            .get("details")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    }
}

fn cli(e: memstead_base::EngineError) -> Rendered {
    encode_cli(memstead_cli::CliError::from_engine_op(e))
}

fn mcp_full(e: memstead_base::FullEngineError, engine: &memstead_base::Engine) -> Rendered {
    decode_mcp(memstead_mcp::server::render_full_engine_error(e, engine))
}

fn cli_full(e: memstead_base::FullEngineError) -> Rendered {
    encode_cli(memstead_cli::CliError::from_full_engine_op(e))
}

fn encode_cli(c: memstead_cli::CliError) -> Rendered {
    let (_, line) =
        memstead_cli::output::render_cli_error(c.code, &c.message, true, c.details.as_ref());
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    Rendered {
        code: v["code"].as_str().unwrap_or_default().to_string(),
        message: v["message"].as_str().unwrap_or_default().to_string(),
        details: v.get("details").cloned().unwrap_or(serde_json::Value::Null),
    }
}

/// Keys present on one side only, and keys whose values differ.
fn key_diff(
    a: &serde_json::Value,
    b: &serde_json::Value,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let empty = serde_json::Map::new();
    let am = a.as_object().unwrap_or(&empty);
    let bm = b.as_object().unwrap_or(&empty);
    let only_a = am
        .keys()
        .filter(|k| !bm.contains_key(*k))
        .cloned()
        .collect();
    let only_b = bm
        .keys()
        .filter(|k| !am.contains_key(*k))
        .cloned()
        .collect();
    let differ = am
        .iter()
        .filter(|(k, v)| bm.get(*k).is_some_and(|w| w != *v))
        .map(|(k, _)| k.clone())
        .collect();
    (only_a, only_b, differ)
}

struct Case {
    label: String,
    code: String,
    message: String,
    details: serde_json::Value,
    mcp: Rendered,
    cli: Rendered,
}

/// Every error type the engine defines, each value rendered by both
/// surfaces. The error types are not Clone (they can carry an
/// io::Error), so each list is built twice, deterministically.
fn cases(engine: &memstead_base::Engine) -> Vec<Case> {
    let mut out = Vec::new();
    for ((label, e), (_, e2)) in every_engine_error().into_iter().zip(every_engine_error()) {
        out.push(Case {
            label,
            code: e.code().to_string(),
            message: e.prose_render(),
            details: e.details(),
            mcp: mcp(e, engine),
            cli: cli(e2),
        });
    }
    for ((label, e), (_, e2)) in every_full_engine_error()
        .into_iter()
        .zip(every_full_engine_error())
    {
        out.push(Case {
            label,
            code: e.code().to_string(),
            message: e.prose_render(),
            details: e.details(),
            mcp: mcp_full(e, engine),
            cli: cli_full(e2),
        });
    }
    out
}

#[test]
fn every_engine_error_carries_the_same_code_payload_and_message_on_mcp_and_cli() {
    assert_eq!(
        missing_samples(),
        Vec::<String>::new(),
        "every variant of every engine error needs a sample"
    );
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = engine(tmp.path());
    let mut rows = Vec::new();
    let mut snapshot = serde_json::Map::new();
    let mut agree = 0usize;
    let all = cases(&engine);
    let total = all.len();
    for Case {
        label,
        code: engine_code,
        message: engine_message,
        details: engine_details,
        mcp: m,
        cli: c,
    } in all
    {
        assert!(!m.code.is_empty(), "{label}: MCP rendered no code");
        assert!(!c.code.is_empty(), "{label}: CLI rendered no code");
        snapshot.insert(
            label.clone(),
            serde_json::json!({
                "mcp": { "code": m.code, "message": m.message, "details": m.details },
                "cli": { "code": c.code, "message": c.message, "details": c.details },
                "engine": { "code": engine_code, "message": engine_message, "details": engine_details },
            }),
        );
        let mut notes = Vec::new();
        if m.code != c.code || m.code != engine_code {
            notes.push(format!(
                "code engine `{engine_code}`, MCP `{}`, CLI `{}`",
                m.code, c.code
            ));
        }
        if m.details != c.details {
            let (only_m, only_c, differ) = key_diff(&m.details, &c.details);
            notes.push(format!(
                "details: MCP only {only_m:?}, CLI only {only_c:?}, values differ {differ:?}"
            ));
        }
        // Where both surfaces agree, they may still serve a payload the
        // engine's own `details()` does not build: a hand-built copy.
        let (surface_only, engine_only, values) = key_diff(&m.details, &engine_details);
        if m.details == c.details && m.details != engine_details {
            notes.push(format!(
                "both surfaces serve a hand-built payload: keys not in engine details() {surface_only:?}, engine-only keys {engine_only:?}, values differ {values:?}"
            ));
        }
        if !m
            .message
            .starts_with(engine_message.lines().next().unwrap_or_default())
            || !c
                .message
                .starts_with(engine_message.lines().next().unwrap_or_default())
        {
            notes.push("message does not begin with the engine's on both surfaces".to_string());
        }
        if notes.is_empty() {
            agree += 1;
        } else {
            rows.push(format!(
                "| `{label}` | `{engine_code}` | {} |",
                notes.join("; ")
            ));
        }
    }
    let report = format!(
        "{} of {total} engine errors render identically on MCP and the CLI; {} differ.\n\n| sample | engine code | difference |\n|---|---|---|\n{}\n",
        agree,
        rows.len(),
        rows.join("\n")
    );
    eprintln!("{report}");
    if let Ok(path) = std::env::var("ERROR_PARITY_SNAPSHOT") {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::Value::Object(snapshot)).unwrap(),
        )
        .unwrap();
    }
    if let Ok(path) = std::env::var("ERROR_PARITY_OUT") {
        std::fs::write(path, &report).unwrap();
    }
    assert!(rows.is_empty(), "{report}");
}
