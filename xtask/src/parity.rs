//! Render the Surface Parity Matrix. Each row of the matrix is a logical
//! engine operation declared in `xtask/operations.toml`; columns line up
//! the matching MCP tool name, top-level CLI subcommand, UniFFI `Engine`
//! method, and WASM JS-visible entry point. Names emitted by the live
//! extractors that the registry doesn't pin land in a dedicated
//! "unaligned" sub-table so the matrix never silently drops a row when
//! a new tool / command / method appears.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use clap::CommandFactory;
use serde::Deserialize;

use crate::mcp;
use crate::udl;

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
    #[serde(default)]
    uniffi: Option<String>,
    #[serde(default)]
    wasm: Option<String>,
}

pub struct Inputs {
    pub mcp_basis: Vec<String>,
    pub mcp_pro: Vec<String>,
    pub cli_basis: Vec<String>,
    pub cli_pro: Vec<String>,
    pub uniffi_methods: Vec<String>,
    pub wasm_methods: Vec<String>,
}

/// Subcommands compiled only into the full `memstead` build (the
/// `mem-repo` feature). `xtask` links `memstead-cli` with that
/// feature on, so `Cli::command()` yields the full set; the lean
/// surface is that set minus these. Kept in sync with the
/// `#[cfg(feature = "mem-repo")]` variants in
/// `memstead-cli/src/cli.rs`.
const CLI_MEM_REPO_ONLY: &[&str] = &[
    "install",
    "batch-update",
    "recover",
    "mem",
    "mem-repo",
    "workspace",
];

pub fn collect_inputs(udl_source: &str, wasm_methods: Vec<String>) -> Inputs {
    let (mcp_basis, mcp_pro) = mcp::tool_names();
    // One CLI crate now. `xtask` links it with `mem-repo` on, so
    // `Cli::command()` is the full surface; the lean surface drops the
    // mem-repo-only subcommands.
    let cli_pro = subcommand_names(&memstead_cli::cli::Cli::command());
    let cli_basis: Vec<String> = cli_pro
        .iter()
        .filter(|n| !CLI_MEM_REPO_ONLY.contains(&n.as_str()))
        .cloned()
        .collect();
    let uniffi_methods = udl::engine_methods(udl_source)
        .into_iter()
        .filter(|m| m != "constructor")
        .collect();
    Inputs {
        mcp_basis,
        mcp_pro,
        cli_basis,
        cli_pro,
        uniffi_methods,
        wasm_methods,
    }
}

pub fn render(operations_toml: &str, inputs: &Inputs) -> Result<String> {
    let parsed: Operations = toml::from_str(operations_toml)
        .context("parsing xtask/operations.toml")?;
    Ok(render_parsed(&parsed, inputs))
}

fn render_parsed(ops: &Operations, inputs: &Inputs) -> String {
    let mut out = String::new();
    out.push_str("# Surface Parity Matrix\n\n");
    out.push_str(
        "Every public engine operation across the four programmatic \
         surfaces (MCP, CLI, UniFFI, WASM). Rows are aligned by the \
         hand-maintained `xtask/operations.toml` registry; cells render \
         the surface-specific name when present and `—` when the surface \
         doesn't expose the operation. The Registry HTTP surface is its \
         own publication layer and not in this matrix.\n\n",
    );

    let mcp_basis_set: BTreeSet<&str> =
        inputs.mcp_basis.iter().map(String::as_str).collect();
    let mcp_pro_set: BTreeSet<&str> =
        inputs.mcp_pro.iter().map(String::as_str).collect();
    let cli_basis_set: BTreeSet<&str> =
        inputs.cli_basis.iter().map(String::as_str).collect();
    let cli_pro_set: BTreeSet<&str> =
        inputs.cli_pro.iter().map(String::as_str).collect();
    let uniffi_set: BTreeSet<&str> =
        inputs.uniffi_methods.iter().map(String::as_str).collect();
    let wasm_set: BTreeSet<&str> =
        inputs.wasm_methods.iter().map(String::as_str).collect();

    out.push_str("## Matrix\n\n");
    out.push_str("| Operation | MCP | CLI | UniFFI | WASM |\n");
    out.push_str("|-----------|-----|-----|--------|------|\n");
    for op in &ops.operation {
        let mcp_cell = match &op.mcp {
            Some(name) => format!(
                "`{}`{}",
                name,
                flavour_suffix(
                    mcp_basis_set.contains(name.as_str()),
                    mcp_pro_set.contains(name.as_str()),
                ),
            ),
            None => "—".to_string(),
        };
        let cli_cell = match &op.cli {
            Some(name) => format!(
                "`{}`{}",
                name,
                flavour_suffix(
                    cli_basis_set.contains(name.as_str()),
                    cli_pro_set.contains(name.as_str()),
                ),
            ),
            None => "—".to_string(),
        };
        let uniffi_cell = match &op.uniffi {
            Some(name) => format!("`{}`", name),
            None => "—".to_string(),
        };
        let wasm_cell = match &op.wasm {
            Some(name) => format!("`{}`", name),
            None => "—".to_string(),
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            op.name, mcp_cell, cli_cell, uniffi_cell, wasm_cell,
        ));
    }
    out.push('\n');

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
    let claimed_uniffi: BTreeSet<&str> = ops
        .operation
        .iter()
        .filter_map(|o| o.uniffi.as_deref())
        .collect();
    let claimed_wasm: BTreeSet<&str> = ops
        .operation
        .iter()
        .filter_map(|o| o.wasm.as_deref())
        .collect();

    let unaligned_mcp: Vec<&str> = mcp_pro_set
        .iter()
        .chain(mcp_basis_set.iter())
        .copied()
        .filter(|name| !claimed_mcp.contains(name))
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .collect();
    let unaligned_cli: Vec<&str> = cli_pro_set
        .iter()
        .chain(cli_basis_set.iter())
        .copied()
        .filter(|name| !claimed_cli.contains(name))
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .collect();
    let unaligned_uniffi: Vec<&str> = uniffi_set
        .iter()
        .copied()
        .filter(|name| !claimed_uniffi.contains(name))
        .collect();
    let unaligned_wasm: Vec<&str> = wasm_set
        .iter()
        .copied()
        .filter(|name| !claimed_wasm.contains(name))
        .collect();

    if unaligned_mcp.is_empty()
        && unaligned_cli.is_empty()
        && unaligned_uniffi.is_empty()
        && unaligned_wasm.is_empty()
    {
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
        emit_unaligned_table(&mut out, "UniFFI", &unaligned_uniffi);
        emit_unaligned_table(&mut out, "WASM", &unaligned_wasm);
    }

    out
}

fn flavour_suffix(in_basis: bool, in_pro: bool) -> &'static str {
    match (in_basis, in_pro) {
        (true, true) => " *(lean + full)*",
        (true, false) => " *(lean only)*",
        (false, true) => " *(full only)*",
        (false, false) => " *(declared but not exposed)*",
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
