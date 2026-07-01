#![cfg(feature = "vault-repo")]
//! Agent-surface polish:
//! `--quiet` parity with `--json`, the exit-code-table documenting the
//! clap usage-error code, and the `vault unregister` / `workspace` help
//! text matching what those commands actually do. Asserted at the clap
//! tree level (no subprocess) so the checks pin the declared surface the
//! `--help` renderer and the doc generator both read.

use clap::{CommandFactory, Parser};
use memstead_cli::cli::{Cli, EXIT_CODES_HELP};

/// CLI F3: `--quiet` is global like `--json` — accepted *after* the
/// subcommand, not only before it. Pre-fix this tripped clap's
/// `unexpected argument '--quiet'` usage error (exit 2).
#[test]
fn quiet_is_accepted_in_trailing_position_like_json() {
    // Trailing: both flags after the subcommand.
    let cli = Cli::try_parse_from(["memstead", "stats", "--quiet", "--json"])
        .expect("--quiet must be accepted after the subcommand (global), like --json");
    assert!(cli.quiet, "--quiet must take effect in trailing position");
    assert!(cli.json, "--json parity baseline");

    // Leading still works (no regression).
    let cli = Cli::try_parse_from(["memstead", "--quiet", "stats"])
        .expect("--quiet must still be accepted before the subcommand");
    assert!(cli.quiet);
}

/// CLI F3 doc: the exit-code table documents clap's usage-error code (2)
/// without renumbering the existing codes a programmatic caller branches
/// on.
#[test]
fn exit_code_table_documents_usage_error_two() {
    assert!(
        EXIT_CODES_HELP.contains("2  usage error"),
        "exit-code table must document the clap usage-error code; got:\n{EXIT_CODES_HELP}",
    );
    // Existing codes keep their meanings (not renumbered).
    for line in [
        "0  success",
        "1  generic failure",
        "3  not found",
        "4  hash mismatch",
        "5  validation",
    ] {
        assert!(EXIT_CODES_HELP.contains(line), "exit-code `{line}` must survive");
    }
}

/// Combined `about` + `long_about` text for a `vault <sub>` subcommand.
fn vault_sub_help(sub: &str) -> String {
    let cmd = Cli::command();
    let vault = cmd.find_subcommand("vault").expect("vault subcommand present");
    let s = vault.find_subcommand(sub).unwrap_or_else(|| panic!("vault {sub} present"));
    let about = s.get_about().map(|a| a.to_string()).unwrap_or_default();
    let long = s.get_long_about().map(|a| a.to_string()).unwrap_or_default();
    format!("{about}\n{long}")
}

/// CLI F4: `vault unregister --help` documents the
/// `VAULT_HAS_INCOMING_REFS` precondition and its recovery.
#[test]
fn vault_unregister_help_documents_incoming_refs_refusal() {
    let help = vault_sub_help("unregister");
    assert!(
        help.contains("VAULT_HAS_INCOMING_REFS"),
        "unregister help must name the incoming-refs refusal; got:\n{help}",
    );
    assert!(
        help.to_lowercase().contains("remove") && help.to_lowercase().contains("reference"),
        "unregister help must state the recovery (remove incoming references); got:\n{help}",
    );
}

/// CLI F5: the `workspace` group describes itself as introspection *and*
/// policy configuration — no longer "Read-only", which hid the
/// policy-write subcommands from an agent scanning `--help`.
#[test]
fn workspace_group_help_describes_policy_configuration() {
    let cmd = Cli::command();
    let ws = cmd.find_subcommand("workspace").expect("workspace subcommand present");
    let about = ws.get_about().map(|a| a.to_string()).unwrap_or_default();
    let long = ws.get_long_about().map(|a| a.to_string()).unwrap_or_default();
    let help = format!("{about}\n{long}");
    assert!(
        !help.contains("Read-only"),
        "workspace help must not mislabel the group Read-only; got:\n{help}",
    );
    assert!(
        help.to_lowercase().contains("configure") || help.to_lowercase().contains("policy"),
        "workspace help must surface that it configures policy; got:\n{help}",
    );
}
