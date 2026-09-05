//! The anchor-state vocabulary lives only in the engine's enum: the CLI's
//! `--json` output spells states with the enum's wire names, `verify-anchors
//! --help` lists every state from the enum's own documentation, and the
//! resolution figure never leaves the engine without its population.

use std::fs;

use assert_cmd::Command;
use memstead_base::anchor::AnchorState;
use serde_json::Value;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder-mount workspace whose one mem carries two anchored rows: one
/// whose artifact is present (resolves or recheck, depending on the hash
/// the row carries) and one whose artifact is gone (orphaned).
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let store = root.join(".memstead");
    fs::create_dir_all(store.join("state")).unwrap();
    fs::write(
        store.join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    fs::write(
        store.join("state/mounts.json"),
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"hold","schema":"default@1.0.0","storage":{"type":"folder","path":"hold-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    )
    .unwrap();
    let mem = root.join("hold-mem");
    fs::create_dir_all(mem.join(".memstead")).unwrap();
    fs::write(
        mem.join(".memstead/config.json"),
        r#"{"format":1,"schema":"default@1.0.0"}"#,
    )
    .unwrap();
    fs::write(
        mem.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nAnchored.\n\n## Purpose\n\nA fixture.\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/present.rs"), "fn present() {}\n").unwrap();
    fs::write(
        mem.join(".memstead/anchors.json"),
        r#"{"version":1,"entities":{"hold--alpha":[{"artifact":"src/present.rs","grain":"file","class":"anchored","hash_stability":"stable"},{"artifact":"src/gone.rs","grain":"file","class":"anchored","hash_stability":"stable"}]}}"#,
    )
    .unwrap();
    tmp
}

/// Every state the `--json` output prints is one of the enum's wire names,
/// the figure rides the payload only beside its population fields, and the
/// markdown prints the count in the same sentence as the population.
#[test]
fn json_states_are_the_enums_wire_names_and_the_figure_carries_its_population() {
    let tmp = workspace();
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "verify-anchors", "--mem", "hold"])
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}\n{}", String::from_utf8_lossy(&out.stdout)));
    let wire: Vec<&str> = AnchorState::ALL.iter().map(|s| s.as_wire()).collect();
    let rows = json["anchors"].as_array().expect("anchor rows");
    assert_eq!(rows.len(), 2, "{json}");
    for row in rows {
        let state = row["state"].as_str().unwrap();
        assert!(
            wire.contains(&state),
            "state `{state}` is not in the enum's vocabulary"
        );
    }
    assert!(
        rows.iter().any(|r| r["state"] == "orphaned"),
        "the missing artifact reads as orphaned: {json}"
    );
    // The figure's three fields travel together, at the same level.
    assert!(json["resolves"].is_number(), "{json}");
    let population = json["population"].as_str().unwrap();
    assert!(population.starts_with("over "), "{population}");
    assert!(json["fully_adjudicated"].is_boolean());

    // Health's anchors axis carries the same three fields per mem.
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "health", "--include", "anchors"])
        .output()
        .unwrap();
    let health: Value = serde_json::from_slice(&out.stdout).unwrap();
    let row = &health["anchors"]["hold"];
    assert!(
        row["resolves"].is_number() && row["population"].is_string(),
        "{row}"
    );
    assert!(
        row["population"].as_str().unwrap().starts_with("over "),
        "{row}"
    );

    // Markdown: the count and its population share one sentence.
    let md = memstead()
        .current_dir(tmp.path())
        .args(["verify-anchors", "--mem", "hold"])
        .output()
        .unwrap();
    let md = String::from_utf8(md.stdout).unwrap();
    let line = md
        .lines()
        .find(|l| l.starts_with("- Resolves:"))
        .unwrap_or_else(|| panic!("{md}"));
    assert!(
        line.contains("over ") && line.contains("counted row(s)"),
        "{line}"
    );
    let md = memstead()
        .current_dir(tmp.path())
        .args(["health", "--include", "anchors"])
        .output()
        .unwrap();
    let md = String::from_utf8(md.stdout).unwrap();
    let line = md
        .lines()
        .find(|l| l.starts_with("- `hold`: resolves"))
        .unwrap_or_else(|| panic!("{md}"));
    assert!(
        line.contains("over ") && line.contains("counted row(s)"),
        "{line}"
    );
}

/// `verify-anchors --help` lists every state from the enum's documentation.
#[test]
fn help_names_every_state_from_the_enum() {
    let out = memstead()
        .args(["verify-anchors", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    for state in AnchorState::ALL {
        assert!(help.contains(state.as_wire()), "{help}");
        assert!(help.contains(state.describe()), "{help}");
    }
}
