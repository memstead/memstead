//! `memstead link <scope/name>` — attach a registry-published mem to
//! this workspace as a read-only mem.
//!
//! The command owns no layout knowledge. It boots the engine, which
//! resolves whatever workspace shape it is standing in (collapsed
//! single-mem folder, repo-overlapping folder, multi-mem folder, or
//! mem-repo), and hands the fetched archive to the same cache-plus-mount
//! path `memstead install <scope>/<name>` uses. The declaration lands in
//! the engine's mount roster (`.memstead/state/mounts.json`) — the one
//! place the engine reads cross-mem attachments from — so what `link`
//! records is what the next boot mounts.
//!
//! Re-invoking `memstead link <same-ref>` re-fetches: the cache is
//! content-addressed, so identical bytes resolve to the same file and
//! changed bytes refresh the mount.
//!
//! Historical note: until 2026-08-27 this command walked for the
//! workspace root itself, read `.memstead/config.json` from that root
//! (a layout the project left behind), and appended the reference to a
//! `deps` list no engine path ever read. Both halves are gone — the
//! layout walk because the engine already knows the layout, and the
//! `deps` list because the mount roster is the single vocabulary for a
//! cross-mem attachment.

use clap::Args;

use crate::CliError;
use crate::output::ExitKind;
use crate::registry;
use crate::setup::CliContext;

/// `memstead link` arguments.
#[derive(Args, Debug)]
pub struct LinkArgs {
    /// Registry-published mem in `scope/name` form (no `@` prefix —
    /// that syntax is retired).
    #[arg(value_name = "SCOPE/NAME")]
    pub dep: String,

    /// Override the registry URL. Falls back to `MEMSTEAD_REGISTRY` then
    /// the default `https://memstead.io`.
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
}

pub fn run(ctx: &CliContext, args: LinkArgs) -> anyhow::Result<()> {
    let Some((scope, name)) = registry::parse_ref(&args.dep) else {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "invalid mem reference {value:?} — expected `<scope>/<name>`, \
                 for example `anthropic/core`",
                value = args.dep
            ),
        )
        .into());
    };

    // The engine resolves the layout. `link` keeps no second layout
    // rule and no first-attempt path of its own: an absent workspace
    // refuses here, from the shared boot seam, naming what was looked
    // for and where.
    let mut engine = ctx.cli_engine()?;

    let fetched =
        crate::commands::install::fetch_registry_archive(&scope, &name, args.registry.as_deref())?;

    crate::commands::install::install_archive(
        ctx,
        engine.base_mut(),
        fetched.file.path(),
        Some(fetched.source_url),
        "Linked",
        "memstead link",
    )
}
