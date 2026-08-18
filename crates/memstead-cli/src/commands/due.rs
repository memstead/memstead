//! `memstead due` — render the due-brief: every open entity whose
//! schema-declared due date falls inside the window, overdue first,
//! across every mem whose schema declares the axis (read-only mounts
//! labelled as third-party quoted data). The renderer is the shared
//! engine entry point `Engine::render_due_brief`, so every consuming
//! surface serves byte-identical content — the projection-brief
//! precedent. There is deliberately no MCP tool (briefs are the
//! CLI family); the MCP server instructions name this verb as the
//! CLI companion.

use clap::Args as ClapArgs;
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Relative window against today: `<N>d` days, `<N>m` calendar
    /// months, `<N>y` calendar years (e.g. 90d, 6m, 2y). Everything
    /// already overdue is always included. Default: 90d.
    #[arg(long, default_value = memstead_base::engine::due::DEFAULT_DUE_WINDOW)]
    pub within: String,

    /// Restrict the brief to one mem (default: every mounted mem whose
    /// schema declares a due axis).
    #[arg(long)]
    pub mem: Option<String>,

    /// Override the current date (YYYY-MM-DD). Testing hook — the
    /// brief is deterministic given the store and this date.
    #[arg(long, hide = true)]
    pub today: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let window = memstead_base::engine::due::parse_due_window(&args.within)
        .map_err(|msg| CliError::new(ExitKind::Validation, "INVALID_INPUT", msg))?;
    let today = match &args.today {
        Some(t) => t.clone(),
        None => current_date(),
    };
    let engine = ctx.cli_engine()?;
    let brief = engine
        .base()
        .render_due_brief(&today, &window, args.mem.as_deref())
        .map_err(|msg| CliError::new(ExitKind::Validation, "INVALID_INPUT", msg))?;
    if ctx.json {
        print_json(&json!({
            "today": today,
            "within": args.within,
            "mem": args.mem,
            "brief": brief,
        }))?;
    } else {
        print_markdown(&brief);
    }
    Ok(())
}

/// Today's date as ISO `YYYY-MM-DD` (UTC — the engine's date
/// convention throughout). Taken once per invocation.
fn current_date() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}
