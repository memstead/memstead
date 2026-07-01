//! Render the WASM surface as a Markdown reference. The
//! `memstead-wasm` crate exposes its JS-visible entry points via
//! `#[wasm_bindgen]` attributes; this module parses the source file
//! line-by-line, captures every annotated free function and every
//! method inside a `#[wasm_bindgen] impl …` block, and emits both:
//!
//! - a flat list of JS-visible names (`method_names`) for the
//!   Surface Parity Matrix
//! - a structured Markdown reference (`render`) listing each entry
//!   with its JS name, Rust signature, and doc comment
//!
//! The parser is intentionally narrow: it recognises the syntactic
//! shape `memstead-wasm/src/lib.rs` uses today (single `impl Engine`
//! block, `js_name = "…"` overrides, `///` doc comments). New
//! `#[wasm_bindgen]` patterns may require parser extensions.

use std::path::Path;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct WasmEntry {
    pub js_name: String,
    pub rust_name: String,
    pub signature: String,
    pub doc: Vec<String>,
    pub kind: EntryKind,
    /// Optional `#[cfg(feature = "X")]` gate captured immediately
    /// before the `#[wasm_bindgen]` attribute.
    pub cfg_feature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// Top-level free function.
    Function,
    /// Method declared inside a `#[wasm_bindgen] impl Engine` block.
    Method,
}

/// JS-visible names from the WASM surface — feeds the parity matrix.
/// The free `setPanicHook` is included; it's still a JS-visible entry
/// point even though it doesn't map to an engine operation.
pub fn method_names_from_file(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(parse(&text)
        .into_iter()
        .map(|e| e.js_name)
        .collect())
}

pub fn render_from_file(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(render(&parse(&text)))
}

/// Parse `memstead-wasm/src/lib.rs` into the list of JS-visible entries.
/// The walk tracks three pieces of state: (1) any pending `///` doc
/// lines, (2) an optional pending `#[cfg(feature = "…")]` gate, and
/// (3) whether we're currently inside `impl Engine`. When a
/// `#[wasm_bindgen]` attribute starts, we read it (possibly spanning
/// multiple lines) and then read the next `pub fn …` declaration —
/// signature lines until the opening `{` — to capture name + sig.
pub fn parse(src: &str) -> Vec<WasmEntry> {
    let mut out: Vec<WasmEntry> = Vec::new();
    let mut doc: Vec<String> = Vec::new();
    let mut cfg_feature: Option<String> = None;
    let mut in_engine_impl = false;
    let mut engine_impl_depth: i32 = 0;

    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Track entry into / exit from `impl Engine` so we can
        // distinguish methods from free functions.
        if in_engine_impl {
            engine_impl_depth += brace_delta(line);
            if engine_impl_depth <= 0 {
                in_engine_impl = false;
                engine_impl_depth = 0;
            }
        }

        // Doc-comment collection.
        if let Some(text) = strip_doc_prefix(trimmed) {
            doc.push(text.to_string());
            i += 1;
            continue;
        }

        // `#[cfg(feature = "…")]` gate immediately preceding a
        // `#[wasm_bindgen]` attribute is captured so we can render
        // the conditional.
        if let Some(feat) = parse_cfg_feature(trimmed) {
            cfg_feature = Some(feat);
            i += 1;
            continue;
        }

        // Start of a `#[wasm_bindgen]` attribute. May span multiple
        // lines (e.g. `#[wasm_bindgen(\n    js_name = …,\n)]`).
        if trimmed.starts_with("#[wasm_bindgen") {
            let (attr, next_i) = collect_attribute(&lines, i);
            let js_override = parse_js_name(&attr);

            // Walk forward to the next non-blank, non-attribute
            // line — that's either `pub fn …`, `pub struct …`, or
            // `impl …`.
            let mut j = next_i;
            while j < lines.len() {
                let t = lines[j].trim();
                if t.is_empty() || t.starts_with("//") || t.starts_with("#[") {
                    j += 1;
                    continue;
                }
                break;
            }
            if j >= lines.len() {
                break;
            }
            let target = lines[j].trim();

            // `impl Engine` — record we're entering it and skip.
            if target.starts_with("impl Engine") || target.starts_with("impl ") {
                in_engine_impl = target.contains("Engine");
                engine_impl_depth = brace_delta(lines[j]);
                i = j + 1;
                doc.clear();
                cfg_feature = None;
                continue;
            }

            // `pub struct` — not an entry point we render; just
            // consume any pending doc/cfg and move on.
            if target.starts_with("pub struct") {
                i = j + 1;
                doc.clear();
                cfg_feature = None;
                continue;
            }

            // `pub fn` — the entry point we care about. Walk
            // through the signature until we hit the opening brace.
            if target.starts_with("pub fn") {
                let (signature, end_i) = collect_signature(&lines, j);
                let rust_name = parse_fn_name(&signature)
                    .unwrap_or_else(|| "<unknown>".to_string());
                let js_name = js_override
                    .clone()
                    .unwrap_or_else(|| snake_to_camel(&rust_name));
                let kind = if in_engine_impl {
                    EntryKind::Method
                } else {
                    EntryKind::Function
                };
                out.push(WasmEntry {
                    js_name,
                    rust_name,
                    signature,
                    doc: doc.clone(),
                    kind,
                    cfg_feature: cfg_feature.clone(),
                });
                doc.clear();
                cfg_feature = None;

                // Account for braces on every line we skip past
                // (intermediate attribute / comment lines, plus
                // the signature span ending with the body's `{`).
                // Without this catch-up the iter-start depth
                // tracker would miss the fn body's opening brace
                // and prematurely exit `in_engine_impl` when the
                // body's closing brace appears.
                if in_engine_impl {
                    for k in (i + 1)..=end_i {
                        engine_impl_depth += brace_delta(lines[k]);
                    }
                    if engine_impl_depth <= 0 {
                        in_engine_impl = false;
                        engine_impl_depth = 0;
                    }
                }
                i = end_i + 1;
                continue;
            }

            // Unknown follow-up — consume and continue.
            i = j + 1;
            doc.clear();
            cfg_feature = None;
            continue;
        }

        // Any other non-blank line clears pending doc/cfg.
        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            doc.clear();
            cfg_feature = None;
        }
        i += 1;
    }

    out
}

fn collect_attribute(lines: &[&str], start: usize) -> (String, usize) {
    let mut attr = String::new();
    let mut depth: i32 = 0;
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        attr.push_str(line);
        attr.push('\n');
        for ch in line.chars() {
            match ch {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
        if depth <= 0 {
            break;
        }
    }
    (attr, i)
}

fn collect_signature(lines: &[&str], start: usize) -> (String, usize) {
    let mut sig = String::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if let Some(idx) = line.find('{') {
            sig.push_str(line[..idx].trim_end());
            return (sig.trim().to_string(), i);
        }
        sig.push_str(line.trim_end());
        sig.push(' ');
        i += 1;
    }
    (sig.trim().to_string(), i)
}

fn parse_fn_name(signature: &str) -> Option<String> {
    let after_fn = signature.split_once("fn ")?.1;
    let name: String = after_fn
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn parse_js_name(attr: &str) -> Option<String> {
    let key = "js_name";
    let pos = attr.find(key)?;
    let after = &attr[pos + key.len()..];
    let after = after.trim_start_matches(|c: char| c == ' ' || c == '=');
    // The value can be either an identifier (`js_name = setPanicHook`) or
    // a string literal (`js_name = "setPanicHook"`).
    if let Some(stripped) = after.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let name: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }
}

fn parse_cfg_feature(trimmed: &str) -> Option<String> {
    let prefix = "#[cfg(feature = \"";
    let rest = trimmed.strip_prefix(prefix)?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn strip_doc_prefix(trimmed: &str) -> Option<&str> {
    if let Some(rest) = trimmed.strip_prefix("/// ") {
        Some(rest)
    } else if trimmed == "///" {
        Some("")
    } else {
        None
    }
}

fn brace_delta(line: &str) -> i32 {
    let mut delta: i32 = 0;
    for ch in line.chars() {
        match ch {
            '{' => delta += 1,
            '}' => delta -= 1,
            _ => {}
        }
    }
    delta
}

fn snake_to_camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper_next = false;
    for ch in name.chars() {
        if ch == '_' {
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn render(entries: &[WasmEntry]) -> String {
    let mut out = String::new();
    out.push_str("# WASM surface\n\n");
    out.push_str(
        "Auto-generated from `engine/crates/memstead-wasm/src/lib.rs`. \
         Every entry point annotated with `#[wasm_bindgen]` is listed \
         below with the JS-visible name, the underlying Rust signature, \
         and the doc comment captured from the source.\n\n",
    );
    out.push_str(
        "The WASM surface is the **read-side** of the browser-sync \
         architecture — writes happen server-side and flow back through \
         `applyCommit`. Full-text search is intentionally unavailable in \
         the WASM build; the method exists as a typed-refusal stub so \
         JS call sites can branch on the stable error code \
         (`SEARCH_UNAVAILABLE_IN_WASM`) instead of cfg-style imports.\n\n",
    );

    let functions: Vec<&WasmEntry> = entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Function))
        .collect();
    let methods: Vec<&WasmEntry> = entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Method))
        .collect();

    if !functions.is_empty() {
        out.push_str("## Free functions\n\n");
        for entry in &functions {
            render_entry(&mut out, entry, "");
        }
    }

    if !methods.is_empty() {
        out.push_str("## `Engine` class\n\n");
        out.push_str(
            "The `Engine` class owns the in-memory store. One instance \
             per `.mem` snapshot the client hydrates.\n\n",
        );
        for entry in &methods {
            render_entry(&mut out, entry, "Engine.");
        }
    }

    out
}

fn render_entry(out: &mut String, entry: &WasmEntry, js_prefix: &str) {
    out.push_str(&format!("### `{}{}`", js_prefix, entry.js_name));
    if let Some(feat) = &entry.cfg_feature {
        out.push_str(&format!(" *(feature: `{feat}`)*"));
    }
    out.push_str("\n\n");
    if entry.rust_name != entry.js_name {
        out.push_str(&format!(
            "*Underlying Rust function: `{}`*\n\n",
            entry.rust_name,
        ));
    }
    out.push_str("```rust\n");
    out.push_str(&entry.signature);
    out.push_str("\n```\n\n");
    if !entry.doc.is_empty() {
        for line in &entry.doc {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_free_fn_with_js_name() {
        let src = r#"
/// Install the panic hook.
#[cfg(feature = "panic-hook")]
#[wasm_bindgen(js_name = setPanicHook)]
pub fn set_panic_hook() {
    console_error_panic_hook::set_once();
}
"#;
        let entries = parse(src);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.js_name, "setPanicHook");
        assert_eq!(e.rust_name, "set_panic_hook");
        assert_eq!(e.kind, EntryKind::Function);
        assert_eq!(e.cfg_feature.as_deref(), Some("panic-hook"));
        assert_eq!(e.doc, vec!["Install the panic hook.".to_string()]);
    }

    #[test]
    fn parses_method_with_js_name_override() {
        let src = r#"
#[wasm_bindgen]
pub struct Engine {
    inner: BaseEngine,
}

#[wasm_bindgen]
impl Engine {
    /// Hydrate from snapshot.
    #[wasm_bindgen(js_name = fromSnapshot)]
    pub fn from_snapshot(bytes: Vec<u8>) -> Result<Engine, JsValue> {
        unimplemented!()
    }

    /// Health summary.
    #[wasm_bindgen]
    pub fn health(&self) -> Result<JsValue, JsValue> {
        unimplemented!()
    }
}
"#;
        let entries = parse(src);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].js_name, "fromSnapshot");
        assert_eq!(entries[0].rust_name, "from_snapshot");
        assert_eq!(entries[0].kind, EntryKind::Method);
        assert_eq!(entries[1].js_name, "health");
        assert_eq!(entries[1].rust_name, "health");
        assert_eq!(entries[1].kind, EntryKind::Method);
    }

    #[test]
    fn snake_to_camel_conversion() {
        assert_eq!(snake_to_camel("mem_names"), "memNames");
        assert_eq!(snake_to_camel("apply_commit"), "applyCommit");
        assert_eq!(snake_to_camel("health"), "health");
        assert_eq!(snake_to_camel("from_archive_bytes"), "fromArchiveBytes");
    }
}
