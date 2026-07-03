use std::collections::HashMap;

use clap::Parser;

use memstead_base::EntityId;
use memstead_base::ops::{Query, SearchScope};
use memstead_base::render;

use crate::output::{print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Find entities by text or graph proximity.
#[derive(Parser, Debug)]
#[command(after_long_help = super::FILTER_HELP)]
pub struct Args {
    /// Free-text query. Omit for a pure structural filter.
    pub text: Option<String>,

    #[arg(long)]
    pub mem: Option<String>,

    #[arg(long = "type")]
    pub entity_type: Option<String>,

    /// Restrict text matching to a single field (title or section key).
    /// Maps to `Query.field` — narrows `any`, `not`, and `phrase` for the
    /// query. Replaces the former repeatable plural form, which was orphaned
    /// at the engine level.
    #[arg(long = "field")]
    pub field: Option<String>,

    /// Exclude entities whose text matches this token. Repeatable —
    /// `--exclude OAuth --exclude SAML` drops every hit driven by
    /// either. Maps to `Query.not`. When combined with `--field`, the
    /// exclude scopes to that field via the engine's existing
    /// `Query.field` semantics.
    ///
    /// Example: `memstead search auth --exclude OAuth` returns
    /// "auth"-matching entities that are not driven by an `OAuth`
    /// match.
    #[arg(long = "exclude", value_name = "TOKEN")]
    pub exclude: Vec<String>,

    /// Restrict hits to entities containing this exact phrase
    /// (adjacency-sensitive). Maps to `Query.phrase`. Composable with
    /// `--field` (narrows the phrase match to one field) and
    /// `--exclude` (drops phrase-matching hits that also match the
    /// excluded token). Shell quoting is stripped before the binary
    /// sees the positional text argument — use this flag rather than
    /// quoting in the positional to express adjacency.
    #[arg(long = "phrase", value_name = "TEXT")]
    pub phrase: Option<String>,

    /// Filter by edge type (e.g. USES, IMPLEMENTS).
    #[arg(long)]
    pub edge_type: Option<String>,

    /// Only entities within `--depth` hops of this ID.
    #[arg(long)]
    pub related_to: Option<String>,

    #[arg(long)]
    pub depth: Option<usize>,

    #[arg(long)]
    pub limit: Option<usize>,

    #[arg(long)]
    pub offset: Option<usize>,

    #[arg(long)]
    pub level: Option<String>,

    #[arg(long)]
    pub status: Option<String>,

    /// Equality filter on any schema-declared filterable field:
    /// repeatable `--filter KEY=VALUE`. The four named-flag
    /// shortcuts (`--type` / `--level` / `--status` / `--edge-type`)
    /// handle their common cases; every other `filterable: equality`
    /// field (e.g. `tags`, `scope`) is reachable via this generic
    /// flag. Unknown keys are dropped and surface as engine
    /// warnings. There is no `--confidence` shortcut: a field reached
    /// only when a schema declares it goes through
    /// `--filter <field>=<value>` rather than a dedicated flag.
    #[arg(long = "filter", value_name = "KEY=VALUE")]
    pub filter: Vec<String>,

    /// Return only stub entities (conflicts with --no-stub).
    #[arg(long, conflicts_with = "no_stub")]
    pub stub: bool,

    /// Return only real (non-stub) entities (conflicts with --stub).
    #[arg(long, conflicts_with = "stub")]
    pub no_stub: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut filters = HashMap::new();
    if let Some(level) = args.level {
        filters.insert("level".to_string(), level);
    }
    if let Some(status) = args.status {
        filters.insert("status".to_string(), status);
    }
    for raw in &args.filter {
        let (key, value) = super::parse_filter_arg(raw)?;
        filters.insert(key, value);
    }

    // Wrap a positional CLI text argument into the flat Query shape. Each
    // whitespace-separated token becomes an `any` term (OR semantics). Empty
    // or missing `text` falls through to the metadata-only filter path.
    // `--field` (when set) narrows the query to a single field via
    // `Query.field`. `--exclude` (repeatable) routes each token into
    // `Query.not` for the engine's exclude predicate. `--phrase` routes
    // into `Query.phrase` for adjacency-sensitive matching. Either positional
    // text or `--phrase` triggers Query construction; pure `--field` or
    // `--exclude` without a text/phrase predicate fall through to the
    // metadata-only filter path.
    let any: Vec<String> = args
        .text
        .as_deref()
        .map(|t| t.split_whitespace().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let query = if any.is_empty() && args.phrase.is_none() {
        None
    } else {
        Some(Query {
            any,
            not: args.exclude.clone(),
            field: args.field.clone(),
            phrase: args.phrase.clone(),
        })
    };

    let stub = match (args.stub, args.no_stub) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    };

    let scope = SearchScope {
        query,
        mem: args.mem,
        entity_type: args.entity_type,
        limit: args.limit,
        offset: args.offset,
        filters,
        range_filters: HashMap::new(),
        edge_type: args.edge_type,
        related_to: args.related_to.map(EntityId),
        depth: args.depth,
        expand_via: None,
        expand_depth: None,
        stub,
        token_budget: None,
    };

    let result = match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => {
            if let Some(name) = scope.mem.as_deref()
                && engine.mount(name).is_none()
            {
                return Err(super::list::unknown_mem_error(name, &engine).into());
            }
            engine.search(&scope)?
        }
        CliEngine::Filesystem(engine) => {
            if let Some(name) = scope.mem.as_deref()
                && engine.mount(name).is_none()
            {
                return Err(super::list::unknown_mem_error(name, &engine).into());
            }
            engine.search(&scope)?
        }
    };
    let offset = scope.offset.unwrap_or(0);

    if ctx.json {
        let envelope = render::build_search_envelope(&result, offset);
        print_json(&envelope)?;
    } else {
        print_markdown(&render::render_search_markdown(&result, offset));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// There is no `--confidence` flag: the CLI parser doesn't
    /// recognise it — agents pass `--filter confidence=<value>`
    /// instead, which works for any schema that declares the field.
    #[test]
    fn search_rejects_removed_confidence_flag() {
        let parsed = Args::try_parse_from(["search", "--confidence", "high"]);
        assert!(
            parsed.is_err(),
            "--confidence must be removed from the CLI parser",
        );
        let err = parsed.unwrap_err();
        // clap's "unknown argument" diagnostic shape — substrings
        // present across clap minor versions.
        let msg = err.to_string();
        assert!(
            msg.contains("--confidence") || msg.contains("unexpected"),
            "expected clap unknown-argument diagnostic, got: {msg}",
        );
    }

    /// The four remaining named-flag shortcuts still parse.
    #[test]
    fn search_accepts_remaining_named_shortcuts() {
        let parsed = Args::try_parse_from([
            "search",
            "--type",
            "spec",
            "--level",
            "M0",
            "--status",
            "active",
            "--edge-type",
            "USES",
        ]);
        assert!(
            parsed.is_ok(),
            "remaining shortcuts must still parse: {:?}",
            parsed.err()
        );
    }

    /// `--filter confidence=high` parses via the generic filter
    /// path, which covers any schema that declares the field.
    #[test]
    fn search_filter_confidence_still_parses() {
        let parsed = Args::try_parse_from(["search", "--filter", "confidence=high"]);
        assert!(
            parsed.is_ok(),
            "--filter confidence=high must parse: {:?}",
            parsed.err()
        );
        let args = parsed.unwrap();
        assert!(
            args.filter.iter().any(|f| f.contains("confidence")),
            "filter list must carry the generic confidence pair: {:?}",
            args.filter,
        );
    }
}
