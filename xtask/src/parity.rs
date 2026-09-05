//! Render the Surface Parity Matrix. Each row of the matrix is a logical
//! engine operation declared in `xtask/operations.toml`; columns line up
//! the matching MCP tool name and top-level CLI subcommand. Names
//! emitted by the live extractors that the
//! registry doesn't pin land in a dedicated "unaligned" sub-table so the
//! matrix never silently drops a row when a new tool / command / method
//! appears.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use clap::CommandFactory;
use serde::Deserialize;

use crate::mcp;

#[derive(Debug, Deserialize)]
struct Operations {
    #[serde(default)]
    operation: Vec<Operation>,
}

#[derive(Debug, Deserialize)]
struct Operation {
    name: String,
    #[serde(default)]
    mcp: Option<String>,
    #[serde(default)]
    cli: Option<String>,
    /// Why this operation is deliberately absent from a surface.
    /// Optional and free-form; when present it renders as a footnote
    /// under the matrix and the row is marked. An empty cell alone
    /// cannot say whether the gap is a decision or an oversight — this
    /// is how a decision says so.
    #[serde(default)]
    rationale: Option<String>,
}

pub struct Inputs {
    pub mcp_tools: Vec<String>,
    pub cli_commands: Vec<String>,
}

pub fn collect_inputs() -> Inputs {
    Inputs {
        mcp_tools: mcp::tool_names(),
        cli_commands: subcommand_names(&memstead_cli::cli::Cli::command()),
    }
}

pub fn render(operations_toml: &str, inputs: &Inputs) -> Result<String> {
    let parsed: Operations =
        toml::from_str(operations_toml).context("parsing xtask/operations.toml")?;
    Ok(render_parsed(&parsed, inputs))
}

fn render_parsed(ops: &Operations, inputs: &Inputs) -> String {
    let mut out = String::new();
    out.push_str("# Surface Parity Matrix\n\n");
    out.push_str(
        "Every public engine operation across the two programmatic \
         surfaces (MCP, CLI). Rows are aligned by the \
         hand-maintained `xtask/operations.toml` registry; cells render \
         the surface-specific name when present and `—` when the surface \
         doesn't expose the operation. The Registry HTTP surface is its \
         own publication layer and not in this matrix.\n\n",
    );

    let mcp_set: BTreeSet<&str> = inputs.mcp_tools.iter().map(String::as_str).collect();
    let cli_set: BTreeSet<&str> = inputs.cli_commands.iter().map(String::as_str).collect();

    out.push_str("## Matrix\n\n");
    out.push_str("| Operation | MCP | CLI |\n");
    out.push_str("|-----------|-----|-----|\n");
    for op in &ops.operation {
        let mcp_cell = match &op.mcp {
            Some(name) => format!(
                "`{}`{}",
                name,
                presence_suffix(mcp_set.contains(name.as_str())),
            ),
            None => "—".to_string(),
        };
        let cli_cell = match &op.cli {
            Some(name) => format!(
                "`{}`{}",
                name,
                presence_suffix(cli_set.contains(name.as_str())),
            ),
            None => "—".to_string(),
        };
        let marker = if op.rationale.is_some() { " †" } else { "" };
        out.push_str(&format!(
            "| `{}`{} | {} | {} |\n",
            op.name, marker, mcp_cell, cli_cell,
        ));
    }
    out.push('\n');

    let explained: Vec<&Operation> = ops
        .operation
        .iter()
        .filter(|o| o.rationale.is_some())
        .collect();
    if !explained.is_empty() {
        out.push_str(
            "† These absences are decisions, not gaps. Every other empty \
             cell is simply an operation that surface does not carry.\n\n",
        );
        for op in explained {
            out.push_str(&format!(
                "- `{}` — {}\n",
                op.name,
                op.rationale.as_deref().unwrap_or_default(),
            ));
        }
        out.push('\n');
    }

    let claimed_mcp: BTreeSet<&str> = ops
        .operation
        .iter()
        .filter_map(|o| o.mcp.as_deref())
        .collect();
    let claimed_cli: BTreeSet<&str> = ops
        .operation
        .iter()
        .filter_map(|o| o.cli.as_deref())
        .collect();

    let unaligned_mcp: Vec<&str> = mcp_set
        .iter()
        .copied()
        .filter(|name| !claimed_mcp.contains(name))
        .collect();
    let unaligned_cli: Vec<&str> = cli_set
        .iter()
        .copied()
        .filter(|name| !claimed_cli.contains(name))
        .collect();
    if unaligned_mcp.is_empty() && unaligned_cli.is_empty() {
        out.push_str("## Unaligned\n\n");
        out.push_str("_(all surface entries reference an operation in the matrix above)_\n");
    } else {
        out.push_str("## Unaligned\n\n");
        out.push_str(
            "Surface entries the registry does not pin to a logical \
             operation. Either add a row to `xtask/operations.toml` or, \
             if the entry is intentionally surface-local (e.g. CLI-only \
             registry / setup commands), leave it here as a deliberate \
             gap.\n\n",
        );
        emit_unaligned_table(&mut out, "MCP", &unaligned_mcp);
        emit_unaligned_table(&mut out, "CLI", &unaligned_cli);
    }

    out
}

/// A registry row naming a surface entry the live extractor did not
/// produce is a registry defect, and the matrix says so in the cell.
fn presence_suffix(present: bool) -> &'static str {
    if present {
        ""
    } else {
        " *(declared but not exposed)*"
    }
}

fn emit_unaligned_table(out: &mut String, label: &str, items: &[&str]) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("### Unaligned — {label}\n\n"));
    for name in items {
        out.push_str(&format!("- `{name}`\n"));
    }
    out.push('\n');
}

fn subcommand_names(cmd: &clap::Command) -> Vec<String> {
    let mut names: Vec<String> = cmd
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    names.sort();
    names
}
