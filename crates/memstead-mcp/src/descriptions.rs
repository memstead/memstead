//! The MCP tool descriptions, as text files rather than string literals.
//!
//! WHY: every agent that touches Memstead reads these before it does anything
//! else, which makes them the product's most-read prose by a wide margin. As
//! single-line literals inside a 17,000-line source file they were diffable
//! only as multi-thousand-character one-line changes, and no prose checker
//! reached them: `scripts/vocabulary-lint.py` walks `.md` and `.mdx`, so a
//! retired term here was invisible by construction.
//!
//! WHY NOT `include_str!` AT THE ATTRIBUTE: `#[tool(description = ...)]` in
//! rmcp 2.2 accepts a string literal and nothing else. `include_str!` fails
//! with "Unexpected type `macro`" and a `const` path with "Unexpected type
//! `path`". So the attribute carries no description at all, and the router
//! wrapper stamps each one in from the table below. Every consumer reaches the
//! router through that wrapper, so there is no path that serves an
//! undescribed tool.
//!
//! The files end with a trailing newline, as text files should; `text()`
//! strips exactly that and nothing else, so the served bytes are the file's
//! bytes minus the final newline. A description must not otherwise end in
//! whitespace, which `tool_surface.rs` asserts.

use rmcp::handler::server::router::tool::ToolRouter;

pub const FULL: &[(&str, &str)] = &[
    (
        "memstead_changes_since",
        include_str!("../descriptions/full/memstead_changes_since.md"),
    ),
    (
        "memstead_check",
        include_str!("../descriptions/full/memstead_check.md"),
    ),
    (
        "memstead_create",
        include_str!("../descriptions/full/memstead_create.md"),
    ),
    (
        "memstead_delete",
        include_str!("../descriptions/full/memstead_delete.md"),
    ),
    (
        "memstead_diff",
        include_str!("../descriptions/full/memstead_diff.md"),
    ),
    (
        "memstead_entity",
        include_str!("../descriptions/full/memstead_entity.md"),
    ),
    (
        "memstead_health",
        include_str!("../descriptions/full/memstead_health.md"),
    ),
    (
        "memstead_mem_configure",
        include_str!("../descriptions/full/memstead_mem_configure.md"),
    ),
    (
        "memstead_mem_create",
        include_str!("../descriptions/full/memstead_mem_create.md"),
    ),
    (
        "memstead_mem_delete",
        include_str!("../descriptions/full/memstead_mem_delete.md"),
    ),
    (
        "memstead_mem_set_schema",
        include_str!("../descriptions/full/memstead_mem_set_schema.md"),
    ),
    (
        "memstead_mem_set_version",
        include_str!("../descriptions/full/memstead_mem_set_version.md"),
    ),
    (
        "memstead_overview",
        include_str!("../descriptions/full/memstead_overview.md"),
    ),
    (
        "memstead_relate",
        include_str!("../descriptions/full/memstead_relate.md"),
    ),
    (
        "memstead_reload",
        include_str!("../descriptions/full/memstead_reload.md"),
    ),
    (
        "memstead_rename",
        include_str!("../descriptions/full/memstead_rename.md"),
    ),
    (
        "memstead_retype",
        include_str!("../descriptions/full/memstead_retype.md"),
    ),
    (
        "memstead_schema",
        include_str!("../descriptions/full/memstead_schema.md"),
    ),
    (
        "memstead_search",
        include_str!("../descriptions/full/memstead_search.md"),
    ),
    (
        "memstead_update",
        include_str!("../descriptions/full/memstead_update.md"),
    ),
];

pub const FILESYSTEM: &[(&str, &str)] = &[
    (
        "memstead_changes_since",
        include_str!("../descriptions/filesystem/memstead_changes_since.md"),
    ),
    (
        "memstead_check",
        include_str!("../descriptions/filesystem/memstead_check.md"),
    ),
    (
        "memstead_create",
        include_str!("../descriptions/filesystem/memstead_create.md"),
    ),
    (
        "memstead_delete",
        include_str!("../descriptions/filesystem/memstead_delete.md"),
    ),
    (
        "memstead_diff",
        include_str!("../descriptions/filesystem/memstead_diff.md"),
    ),
    (
        "memstead_entity",
        include_str!("../descriptions/filesystem/memstead_entity.md"),
    ),
    (
        "memstead_health",
        include_str!("../descriptions/filesystem/memstead_health.md"),
    ),
    (
        "memstead_overview",
        include_str!("../descriptions/filesystem/memstead_overview.md"),
    ),
    (
        "memstead_relate",
        include_str!("../descriptions/filesystem/memstead_relate.md"),
    ),
    (
        "memstead_rename",
        include_str!("../descriptions/filesystem/memstead_rename.md"),
    ),
    (
        "memstead_retype",
        include_str!("../descriptions/filesystem/memstead_retype.md"),
    ),
    (
        "memstead_schema",
        include_str!("../descriptions/filesystem/memstead_schema.md"),
    ),
    (
        "memstead_search",
        include_str!("../descriptions/filesystem/memstead_search.md"),
    ),
    (
        "memstead_update",
        include_str!("../descriptions/filesystem/memstead_update.md"),
    ),
];

/// The served text for one tool: the file's bytes without its final newline.
pub fn text(table: &[(&'static str, &'static str)], name: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| t.strip_suffix('\n').unwrap_or(t))
}

/// Stamp every route's description in from `table`.
///
/// A route with no entry is a hard error rather than a tool served without a
/// description: the pair (attribute, file) is the whole contract, and a tool
/// added without its text should fail loudly at the first router build, not
/// ship a blank description to an agent.
pub fn apply<S>(router: &mut ToolRouter<S>, table: &[(&'static str, &'static str)]) {
    for (name, route) in router.map.iter_mut() {
        let t = text(table, name).unwrap_or_else(|| {
            panic!(
                "no description file for MCP tool `{name}` — add \
                 crates/memstead-mcp/descriptions/<surface>/{name}.md and register \
                 it in src/descriptions.rs"
            )
        });
        route.attr.description = Some(t.into());
    }
}
