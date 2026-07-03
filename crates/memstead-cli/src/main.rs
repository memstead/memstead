//! `memstead` — command-line interface for the Memstead graph engine.
//!
//! Subcommands mirror the MCP tool surface. Output defaults to markdown
//! (same text MCP returns); `--json` emits structured content matching
//! the MCP `structured_content` payload.
//!
//! One crate, two build configs. The default (`mem-repo`) build is
//! the full `memstead`: every subcommand, including the multi-mem /
//! mem-repo lifecycle (`mem`, `mem-repo`, `workspace`, `install`,
//! `batch-update`, `recover`). `--no-default-features` drops the
//! git-branch backend and those subcommands — a CI / wasm-adjacent
//! config, not shipped.

use std::process::ExitCode;

use clap::Parser;

use memstead_cli::CliError;
use memstead_cli::cli::{Cli, Command};
use memstead_cli::commands;
use memstead_cli::output::{ExitKind, print_cli_error};
use memstead_cli::setup;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json_mode = cli.json;

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let cli_err = e.downcast_ref::<CliError>();
            let kind = cli_err.map(|c| c.kind).unwrap_or(ExitKind::Generic);
            let code = cli_err.map(|c| c.effective_code()).unwrap_or("INTERNAL");
            let details = cli_err.and_then(|c| c.details.as_ref());
            print_cli_error(code, &e.to_string(), kind, json_mode, details);
            ExitCode::from(kind as u8)
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let ctx = setup::CliContext {
        json: cli.json,
        quiet: cli.quiet,
    };

    match cli.command {
        Command::Stats => commands::stats::run(&ctx),
        Command::Entity(args) => commands::entity::run(&ctx, args),
        Command::Relations(args) => commands::relations::run(&ctx, args),
        Command::Search(args) => commands::search::run(&ctx, args),
        Command::List(args) => commands::list::run(&ctx, args),
        Command::Context(args) => commands::context::run(&ctx, args),
        Command::Overview(args) => commands::overview::run(&ctx, args),
        Command::Type(args) => commands::type_cmd::run(&ctx, args),
        Command::Health(args) => commands::health::run(&ctx, args),
        Command::Export(args) => commands::export::run(&ctx, args),
        Command::Init(args) => commands::init::run(&ctx, args),
        Command::Quickstart(args) => commands::quickstart::run(&ctx, args),
        #[cfg(feature = "mem-repo")]
        Command::Install(args) => commands::install::run(&ctx, args),
        Command::Link(args) => commands::link::run(&ctx, args),
        Command::Publish(args) => commands::publish::run(&ctx, args),
        Command::Unpublish(args) => commands::unpublish::run(&ctx, args),
        Command::Domain { action } => commands::domain::run(&ctx, action),
        Command::Admin { action } => match action {
            commands::admin::AdminAction::Takedown(args) => {
                commands::admin::run_takedown(&ctx, args)
            }
            commands::admin::AdminAction::Denylist(args) => {
                commands::admin::run_denylist(&ctx, args)
            }
        },
        Command::Login(args) => commands::login::run(&ctx, args),
        Command::Logout(args) => commands::logout::run(&ctx, args),
        Command::Create(args) => commands::create::run(&ctx, args),
        Command::Update(args) => commands::update::run(&ctx, args),
        Command::Relate(args) => commands::relate::run(&ctx, args),
        Command::Delete(args) => commands::delete::run(&ctx, args),
        Command::Rename(args) => commands::rename::run(&ctx, args),
        #[cfg(feature = "mem-repo")]
        Command::BatchUpdate(args) => commands::batch_update::run(&ctx, args),
        #[cfg(feature = "mem-repo")]
        Command::Recover(args) => commands::recover::run(&ctx, args),
        Command::Changes(args) => commands::changes::run(&ctx, args),
        Command::Reload(args) => commands::reload::run(&ctx, args),
        #[cfg(feature = "mem-repo")]
        Command::Fetch(args) => commands::transport::run_fetch(&ctx, args),
        #[cfg(feature = "mem-repo")]
        Command::Pull(args) => commands::transport::run_pull(&ctx, args),
        #[cfg(feature = "mem-repo")]
        Command::Push(args) => commands::transport::run_push(&ctx, args),
        #[cfg(feature = "mem-repo")]
        Command::BranchReset(args) => commands::branch_reset::run(&ctx, args),
        #[cfg(feature = "mem-repo")]
        Command::Mem { action } => match action {
            commands::mem::MemAction::Init(args) => commands::mem::run(&ctx, args),
            commands::mem::MemAction::Unregister(args) => {
                commands::mem::run_unregister(&ctx, args)
            }
            commands::mem::MemAction::Delete(args) => commands::mem::run_delete(&ctx, args),
            commands::mem::MemAction::SetVersion(args) => {
                commands::mem::run_set_version(&ctx, args)
            }
            commands::mem::MemAction::SetSchema(args) => {
                commands::mem::run_set_schema(&ctx, args)
            }
            commands::mem::MemAction::SetDescription(args) => {
                commands::mem::run_set_description(&ctx, args)
            }
            commands::mem::MemAction::SetSyncState(args) => {
                commands::mem::run_set_sync_state(&ctx, args)
            }
            commands::mem::MemAction::List(args) => commands::mem::run_list(&ctx, args),
        },
        #[cfg(feature = "mem-repo")]
        Command::MemRepo { action } => commands::mem_repo::run(&ctx, action),
        #[cfg(feature = "mem-repo")]
        Command::Workspace { action } => commands::workspace::run(&ctx, action),
        Command::Schema(args) => commands::schema::run(&ctx, args),
        Command::Pipeline(args) => commands::pipeline::run(&ctx, args),
    }
}
