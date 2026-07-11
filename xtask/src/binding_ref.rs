//! Render the **binding** (`projection`) format reference as Markdown.
//!
//! Two authoritative sources, never hand-copied field lists:
//! - the committed v1 binding JSON Schema
//!   (`docs/schemas/memstead-plugin/v1/binding.schema.json`)
//!   supplies every field, its type, allowed values, and required-ness;
//! - the engine's own capability matrix
//!   ([`memstead_base::binding::medium_capabilities`]) supplies which fields
//!   are legal per medium.
//!
//! Deterministic: the same schema + capability matrix render byte-identically,
//! so the page is drift-gated like the other generated references.

use std::path::Path;

use anyhow::{Context, Result};
use memstead_base::MediumType;
use memstead_base::binding::{medium_capabilities, prune_guarantee_for_medium};
use serde_json::Value;

/// The media rendered in the capability matrix, in a fixed order.
const MEDIA: [MediumType; 5] = [
    MediumType::Codebase,
    MediumType::Filesystem,
    MediumType::Git,
    MediumType::Graph,
    MediumType::Web,
];

/// Preferred top-level field order (declaration first, operations last). Any
/// schema property not listed here is appended in sorted order, so a new field
/// still renders — the page never silently omits a field.
const FIELD_ORDER: &[&str] = &[
    "version",
    "intent",
    "source_facets",
    "reference_mems",
    "destination_mem",
    "deny_paths",
    "coverage_semantics",
    "rules",
    "prune",
    "operations",
];

pub fn render_from_file(schema_path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(schema_path)
        .with_context(|| format!("reading {}", schema_path.display()))?;
    let schema: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", schema_path.display()))?;
    Ok(render(&schema))
}

fn render(schema: &Value) -> String {
    let mut md = String::new();

    md.push_str(
        "A **binding** (stored as a `projection` file at `.memstead/projections/<mem>/<name>.json`) \
         is one versioned record per source→mem obligation: the declaration plus an \
         `operations { build, sync, verify }` block. It collapses the retired projection + ingest \
         pair into a single file.\n\n",
    );
    md.push_str(
        "> This page is generated from the v1 binding JSON Schema and the engine's \
         medium-capability matrix. Do not edit it by hand — regenerate with \
         `cargo run -p xtask -- generate-docs`.\n\n",
    );

    // --- Top-level fields ---
    md.push_str("## Fields\n\n");
    let required = string_set(&schema["required"]);
    if let Some(props) = schema["properties"].as_object() {
        md.push_str("| Field | Type | Required | Allowed values | Description |\n");
        md.push_str("| --- | --- | --- | --- | --- |\n");
        for key in ordered_keys(props) {
            render_field_row(
                &mut md,
                &key,
                &props[&key],
                required.contains(key.as_str()),
                schema,
            );
        }
        md.push('\n');
    }

    // --- Operations ---
    md.push_str("## Operations\n\n");
    md.push_str(
        "Each operation under `operations` is optional. An absent **build** or **sync** makes \
         that *mutating* operation refuse at run time with a `projection enable <op>` remedy; an \
         absent **verify** means engine defaults (verify is read-only, never a refusal).\n\n",
    );
    for (op_key, def_name) in [
        ("build", "buildOperation"),
        ("sync", "syncOperation"),
        ("verify", "verifyOperation"),
    ] {
        let def = &schema["$defs"][def_name];
        md.push_str(&format!("### `{op_key}`\n\n"));
        if let Some(desc) = def["description"].as_str() {
            md.push_str(desc);
            md.push_str("\n\n");
        }
        if let Some(props) = def["properties"].as_object() {
            let req = string_set(&def["required"]);
            md.push_str("| Field | Type | Required | Allowed values | Description |\n");
            md.push_str("| --- | --- | --- | --- | --- |\n");
            for key in ordered_keys(props) {
                render_field_row(
                    &mut md,
                    &key,
                    &props[&key],
                    req.contains(key.as_str()),
                    schema,
                );
            }
            md.push('\n');
        }
    }

    // --- Per-medium capability matrix ---
    md.push_str("## Per-medium capability matrix\n\n");
    md.push_str(
        "Which fields and operations a binding may legally declare depends on the **medium** its \
         source facets resolve to. The engine derives this from the capability matrix below and \
         refuses an illegal combination at **binding-validation** time (never at run time).\n\n",
    );
    md.push_str(
        "| Medium | Enumerable | Change signal | Base retrievable | Anchor namespace | Glob `deny_paths` | Prune guarantee |\n",
    );
    md.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");
    for medium in MEDIA {
        let caps = medium_capabilities(medium);
        md.push_str(&format!(
            "| `{}` | {} | {} | {} | `{}` | {} | `{}` |\n",
            medium_wire(medium),
            yes_no(caps.enumerable),
            yes_no(caps.change_signal),
            yes_no(caps.base_version_retrievable),
            caps.anchor_namespace,
            yes_no(caps.glob_deny_legal),
            prune_guarantee_for_medium(medium).as_wire(),
        ));
    }
    md.push('\n');
    md.push_str(
        "- **Glob `deny_paths`** are legal only over a path-shaped namespace — declaring them over \
         a medium whose **Glob `deny_paths`** column is *no* is refused at binding validation.\n",
    );
    md.push_str(
        "- The **Prune guarantee** column is the strongest guarantee the medium can *support*: \
         `never-clobber` (full three-way merge) only where a base version is retrievable, otherwise \
         `conflict-flag`. Requesting a stronger guarantee than the medium supports is refused at \
         binding validation.\n",
    );

    md
}

/// Render one field as a table row (top-level or operation property).
fn render_field_row(md: &mut String, key: &str, prop: &Value, required: bool, schema: &Value) {
    let resolved = resolve_ref(prop, schema);
    let ty = resolved["type"].as_str().unwrap_or("object");
    let allowed = allowed_values(&resolved);
    let desc = prop["description"]
        .as_str()
        .or_else(|| resolved["description"].as_str())
        .unwrap_or("");
    md.push_str(&format!(
        "| `{}` | {} | {} | {} | {} |\n",
        key,
        ty,
        if required { "yes" } else { "no" },
        allowed,
        escape_pipes(desc),
    ));
}

/// Follow a single `$ref` into `#/$defs/...`; return the value unchanged if it
/// is not a ref.
fn resolve_ref(prop: &Value, schema: &Value) -> Value {
    if let Some(r) = prop["$ref"].as_str()
        && let Some(name) = r.strip_prefix("#/$defs/")
    {
        return schema["$defs"][name].clone();
    }
    prop.clone()
}

/// The allowed-values cell: an `enum` list, a `const`, or `—`.
fn allowed_values(v: &Value) -> String {
    if let Some(arr) = v["enum"].as_array() {
        return arr
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(", ");
    }
    if let Some(c) = v.get("const") {
        return format!("`{c}`");
    }
    if let Some(min) = v["minimum"].as_i64() {
        return format!("≥ {min}");
    }
    "—".to_string()
}

/// Field keys in [`FIELD_ORDER`] first, then any remaining keys sorted — so the
/// output is deterministic and never omits a property the schema adds later.
fn ordered_keys(props: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = props.keys().cloned().collect();
    keys.sort();
    let mut out: Vec<String> = FIELD_ORDER
        .iter()
        .filter(|k| props.contains_key(**k))
        .map(|k| k.to_string())
        .collect();
    for k in keys {
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

fn string_set(v: &Value) -> std::collections::BTreeSet<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn medium_wire(m: MediumType) -> &'static str {
    match m {
        MediumType::Codebase => "codebase",
        MediumType::Filesystem => "filesystem",
        MediumType::Git => "git",
        MediumType::Graph => "graph",
        MediumType::Web => "web",
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn escape_pipes(s: &str) -> String {
    s.replace('|', "\\|")
}
