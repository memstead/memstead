#![cfg(test)]

use super::*;

#[test]
fn derive_mem_name_handles_common_directory_names() {
    assert_eq!(derive_mem_name("my-graph").as_deref(), Some("my-graph"));
    assert_eq!(derive_mem_name("My Project").as_deref(), Some("my-project"));
    assert_eq!(
        derive_mem_name("Notes_2026 (v2)").as_deref(),
        Some("notes-2026-v2")
    );
    // Nothing valid survives: prompt/refusal path.
    assert_eq!(derive_mem_name("日本語"), None);
    assert_eq!(derive_mem_name(""), None);
    // Single char fails the two-char slug rule.
    assert_eq!(derive_mem_name("a"), None);
}

#[test]
fn blocking_entries_tolerates_dotfiles_and_readme_grade() {
    let tmp = tempfile::tempdir().unwrap();
    for f in [".gitignore", ".mcp.json", "README", "LICENSE", "Readme.txt"] {
        std::fs::write(tmp.path().join(f), b"x").unwrap();
    }
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    assert!(blocking_entries(tmp.path()).unwrap().is_empty());

    // A `.md` README blocks — the folder backend would adopt it as
    // an entity, and quickstart never ingests user content.
    std::fs::write(tmp.path().join("README.md"), b"# hi").unwrap();
    assert_eq!(blocking_entries(tmp.path()).unwrap(), vec!["`README.md`"]);
    std::fs::remove_file(tmp.path().join("README.md")).unwrap();

    std::fs::write(tmp.path().join("main.rs"), b"fn main() {}").unwrap();
    assert_eq!(blocking_entries(tmp.path()).unwrap(), vec!["`main.rs`"]);
}

#[test]
fn wire_agent_merges_and_never_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    // Fresh write.
    let outcome = wire_agent(tmp.path(), AgentTarget::ClaudeCode, "/bin/memstead-mcp").unwrap();
    let rendered = outcome
        .action
        .render(outcome.target, &|rel: &str| rel.to_string());
    assert!(rendered.contains("wrote"), "got: {rendered}");
    let parsed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path().join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(
        parsed["mcpServers"]["memstead"]["command"],
        "/bin/memstead-mcp"
    );

    // Existing foreign server entries survive; existing `memstead`
    // entry is never overwritten.
    std::fs::write(
        tmp.path().join(".mcp.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "mcpServers": {
                "other": { "command": "/bin/other" },
                "memstead": { "command": "/custom/memstead-mcp" },
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let outcome = wire_agent(tmp.path(), AgentTarget::ClaudeCode, "/bin/memstead-mcp").unwrap();
    let rendered = outcome
        .action
        .render(outcome.target, &|rel: &str| rel.to_string());
    assert!(rendered.contains("left untouched"), "got: {rendered}");
    let parsed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path().join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(
        parsed["mcpServers"]["memstead"]["command"],
        "/custom/memstead-mcp"
    );
    assert_eq!(parsed["mcpServers"]["other"]["command"], "/bin/other");
}

#[test]
fn shell_quote_leaves_ordinary_paths_alone_and_quotes_the_rest() {
    assert_eq!(
        shell_quote("/usr/local/bin/memstead-mcp"),
        "/usr/local/bin/memstead-mcp"
    );
    assert_eq!(shell_quote("my-graph"), "my-graph");
    // The case that motivated this: a directory name with a space.
    assert_eq!(shell_quote("My Graph"), "'My Graph'");
    assert_eq!(
        shell_quote("/Users/a b/bin/memstead-mcp"),
        "'/Users/a b/bin/memstead-mcp'"
    );
    // Shell metacharacters are contained, not executed.
    assert_eq!(shell_quote("a;rm -rf /"), "'a;rm -rf /'");
    assert_eq!(shell_quote("$(whoami)"), "'$(whoami)'");
    // An embedded single quote closes, escapes, and reopens.
    assert_eq!(shell_quote("it's"), r"'it'\''s'");
    assert_eq!(shell_quote(""), "''");
}

#[test]
fn wire_agent_codex_prints_command_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let outcome = wire_agent(tmp.path(), AgentTarget::Codex, "/bin/memstead-mcp").unwrap();
    let rendered = outcome
        .action
        .render(outcome.target, &|rel: &str| rel.to_string());
    assert!(
        rendered.contains("codex mcp add memstead -- /bin/memstead-mcp"),
        "got: {rendered}",
    );
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}
