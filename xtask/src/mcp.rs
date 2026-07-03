//! Render the MCP tool catalogue as a Markdown reference. Both servers
//! (`FilesystemMcpServer` for the lean build, `McpServer` for the full
//! build) expose a static `tool_router()` associated function generated
//! by the rmcp `#[tool_router]` macro, so the live `Tool` list can be
//! lifted without booting an engine. The two lists overlap on the 11
//! shared tool names and the full list is a strict superset; sections
//! render with a `lean + full` / `full only` flavour tag.

use std::collections::{BTreeMap, BTreeSet};

use rmcp::model::{Tool, ToolAnnotations};

use memstead_mcp::filesystem_server::FilesystemMcpServer;
use memstead_mcp::server::McpServer;

pub fn render() -> String {
    let lean = FilesystemMcpServer::tool_router().list_all();
    let full = McpServer::tool_router().list_all();
    render_tools(&lean, &full)
}

/// Sorted lean / full tool-name lists. Sourced from the same routers as
/// [`render`]; consumed by the parity matrix.
pub fn tool_names() -> (Vec<String>, Vec<String>) {
    let mut lean: Vec<String> = FilesystemMcpServer::tool_router()
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    lean.sort();
    let mut full: Vec<String> = McpServer::tool_router()
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    full.sort();
    (lean, full)
}

fn render_tools(lean: &[Tool], full: &[Tool]) -> String {
    let lean_names: BTreeSet<String> =
        lean.iter().map(|t| t.name.to_string()).collect();
    let pro_names: BTreeSet<String> =
        full.iter().map(|t| t.name.to_string()).collect();
    let lean_by_name: BTreeMap<&str, &Tool> =
        lean.iter().map(|t| (t.name.as_ref(), t)).collect();
    let pro_by_name: BTreeMap<&str, &Tool> =
        full.iter().map(|t| (t.name.as_ref(), t)).collect();

    let mut out = String::new();
    out.push_str("# MCP tools\n\n");
    out.push_str(
        "Generated from the live `tool_router().list_all()` catalogues on \
         `FilesystemMcpServer` (the lean `--no-default-features` build) and \
         `McpServer` (the full default build). Every tool the running server \
         exposes appears below; each section is tagged with the flavour pair \
         (`lean + full`, `lean only`, or `full only`).\n\n",
    );

    out.push_str(&format!(
        "**Counts:** the lean build exposes {} tools; the full build exposes {} (a strict superset on shared names).\n\n",
        lean.len(),
        full.len(),
    ));

    let mut all_names: BTreeSet<&str> = BTreeSet::new();
    for n in lean_names.iter().chain(pro_names.iter()) {
        all_names.insert(n.as_str());
    }

    out.push_str("## Index\n\n");
    for name in &all_names {
        out.push_str(&format!("- [`{name}`](#{})\n", anchor(name)));
    }
    out.push('\n');

    for name in &all_names {
        let in_lean = lean_names.contains(*name);
        let in_pro = pro_names.contains(*name);
        let flavour = match (in_lean, in_pro) {
            (true, true) => "lean + full",
            (true, false) => "lean only",
            (false, true) => "full only",
            (false, false) => unreachable!("name came from the union"),
        };
        let tool = pro_by_name
            .get(*name)
            .copied()
            .or_else(|| lean_by_name.get(*name).copied())
            .expect("tool must exist in at least one server");
        emit_section(&mut out, name, flavour, tool);
    }
    out
}

fn anchor(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' | '-' => c,
            '_' => '-',
            _ => '-',
        })
        .collect()
}

fn emit_section(out: &mut String, name: &str, flavour: &str, tool: &Tool) {
    out.push_str(&format!("## `{name}`\n\n"));
    out.push_str(&format!("**Flavour:** {flavour}\n\n"));
    if let Some(desc) = &tool.description {
        out.push_str(desc);
        out.push_str("\n\n");
    }
    if let Some(annotations) = &tool.annotations {
        let line = render_annotations(annotations);
        if !line.is_empty() {
            out.push_str(&format!("**Hints:** {line}\n\n"));
        }
    }
    out.push_str("**Input schema:**\n\n```json\n");
    let schema_value = serde_json::Value::Object((*tool.input_schema).clone());
    let pretty = serde_json::to_string_pretty(&schema_value)
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&pretty);
    out.push_str("\n```\n\n");
}

fn render_annotations(a: &ToolAnnotations) -> String {
    let mut parts = Vec::new();
    if let Some(v) = a.read_only_hint {
        parts.push(format!("`read_only` = {v}"));
    }
    if let Some(v) = a.destructive_hint {
        parts.push(format!("`destructive` = {v}"));
    }
    if let Some(v) = a.idempotent_hint {
        parts.push(format!("`idempotent` = {v}"));
    }
    if let Some(v) = a.open_world_hint {
        parts.push(format!("`open_world` = {v}"));
    }
    parts.join(", ")
}
