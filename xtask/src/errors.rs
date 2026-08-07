//! Scan the engine workspace for typed error codes and render the
//! cross-surface Error Code Index. Codes are sourced from:
//!
//! * Every `fn code(&self) -> &'static str` body in `memstead-base` —
//!   the whole crate is swept, so `EngineError::code()` /
//!   `ValidationError::code()` / `OpsError::code()` are covered along
//!   with delegated violation `code()` impls (e.g.
//!   `SectionFormatViolation` in `section_format.rs`, reached through
//!   `EngineError::SectionFormatRefused`) — every variant returns an
//!   `UPPER_SNAKE_CASE` literal there. `pub const ..._CODE: &str`
//!   constants in `memstead-base` are also indexed, for `code()` impls
//!   that return a named constant instead of a literal (e.g.
//!   `INVALID_ANCHOR_CODE` in `anchor.rs`).
//! * `tool_error(...)` / `tool_error_with_payload(...)` callsites in
//!   `memstead-mcp` — first positional argument.
//! * `CliError::new(_, "...", _)`, `.with_code("...")`, and
//!   `pub const ..._CODE: &str = "..."` constants in `memstead-cli`.
//!
//! The Registry HTTP error envelope is documented separately by the
//! private `memstead-registry` crate (per-route `ApiError` variants live
//! in its own `registry.md`), so it is not scanned here.
//!
//! Output is a sorted index keyed on the code string with one row per
//! distinct source location, so a code emitted from multiple sites
//! still shows them all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Surface {
    Engine,
    Cli,
    Mcp,
}

impl Surface {
    fn label(self) -> &'static str {
        match self {
            Surface::Engine => "engine",
            Surface::Cli => "CLI",
            Surface::Mcp => "MCP",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Occurrence {
    pub surface: Surface,
    pub source: String,
    pub line: u32,
}

pub fn render(workspace_root: &Path) -> Result<String> {
    let codes = scan(workspace_root)?;
    Ok(render_index(&codes))
}

fn scan(workspace_root: &Path) -> Result<BTreeMap<String, Vec<Occurrence>>> {
    let mut codes: BTreeMap<String, Vec<Occurrence>> = BTreeMap::new();

    scan_engine_codes(workspace_root, &mut codes)?;
    scan_cli_codes(workspace_root, &mut codes)?;
    scan_mcp_codes(workspace_root, &mut codes)?;

    for entries in codes.values_mut() {
        entries.sort();
        entries.dedup();
    }
    Ok(codes)
}

fn scan_engine_codes(
    workspace_root: &Path,
    codes: &mut BTreeMap<String, Vec<Occurrence>>,
) -> Result<()> {
    let arm_re = Regex::new(r#"=>\s*"([A-Z][A-Z0-9_]+)""#).unwrap();
    let bare_lit_re = Regex::new(r#"^\s*"([A-Z][A-Z0-9_]+)"\s*,?\s*(?://.*)?$"#).unwrap();
    let header_re = Regex::new(r#"\bfn code\(&self\)\s*->\s*&'static\s*str"#).unwrap();
    // Same const style the CLI scan matches — catches `code()` impls
    // that return a named constant instead of a string literal.
    let const_re =
        Regex::new(r#"pub const [A-Z_]+_CODE:\s*&str\s*=\s*"([A-Z][A-Z0-9_]+)""#).unwrap();
    // Sweep the whole crate rather than a hand-kept file list: the
    // `fn code(&self) -> &'static str` header gate means only typed-code
    // bodies contribute, so delegated violation `code()` impls (e.g.
    // `section_format.rs`, reached via `EngineError::SectionFormatRefused`)
    // are indexed without anyone remembering to register the file.
    let root = workspace_root.join("crates/memstead-base/src");
    for path in rust_sources(&root)? {
        let rel = pathdiff(workspace_root, &path);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        for cap in const_re.captures_iter(&text) {
            let m = cap.get(1).unwrap();
            push(
                codes,
                m.as_str().to_string(),
                Surface::Engine,
                &rel,
                line_of(&text, m.start()),
            );
        }
        let mut in_code_fn = false;
        let mut depth: i32 = 0;
        for (idx, line) in text.lines().enumerate() {
            if !in_code_fn && header_re.is_match(line) {
                in_code_fn = true;
                depth = 0;
            }
            if in_code_fn {
                depth += line.matches('{').count() as i32;
                depth -= line.matches('}').count() as i32;
                for cap in arm_re
                    .captures_iter(line)
                    .chain(bare_lit_re.captures_iter(line))
                {
                    let code = cap.get(1).unwrap().as_str().to_string();
                    push(codes, code, Surface::Engine, &rel, (idx + 1) as u32);
                }
                if depth <= 0 {
                    in_code_fn = false;
                }
            }
        }
    }
    Ok(())
}

fn scan_cli_codes(
    workspace_root: &Path,
    codes: &mut BTreeMap<String, Vec<Occurrence>>,
) -> Result<()> {
    // Scanned against the whole file, not per line: rustfmt wraps call
    // arguments across lines, so the code literal often sits on its own
    // line below `CliError::new(` / `.with_code(`. `\s` and the negated
    // classes match newlines; the reported line is the literal's own.
    let const_re =
        Regex::new(r#"pub const [A-Z_]+_CODE:\s*&str\s*=\s*"([A-Z][A-Z0-9_]+)""#).unwrap();
    let with_code_re = Regex::new(r#"\.with_code\(\s*"([A-Z][A-Z0-9_]+)"\s*\)"#).unwrap();
    let new_re = Regex::new(r#"CliError::new\([^)"]*?"([A-Z][A-Z0-9_]+)""#).unwrap();
    for crate_dir in ["crates/memstead-cli/src"] {
        let root = workspace_root.join(crate_dir);
        for path in rust_sources(&root)? {
            let rel = pathdiff(workspace_root, &path);
            let text = std::fs::read_to_string(&path)?;
            for re in [&const_re, &with_code_re, &new_re] {
                for cap in re.captures_iter(&text) {
                    let m = cap.get(1).unwrap();
                    push(
                        codes,
                        m.as_str().to_string(),
                        Surface::Cli,
                        &rel,
                        line_of(&text, m.start()),
                    );
                }
            }
        }
    }
    Ok(())
}

fn scan_mcp_codes(
    workspace_root: &Path,
    codes: &mut BTreeMap<String, Vec<Occurrence>>,
) -> Result<()> {
    // Whole-file scan for the same reason as `scan_cli_codes`: rustfmt
    // may put the code literal on the line after `tool_error(`.
    let tool_re =
        Regex::new(r#"\btool_error(?:_with_payload)?\(\s*"([A-Z][A-Z0-9_]+)"\s*,"#).unwrap();
    for crate_dir in ["crates/memstead-mcp/src"] {
        let root = workspace_root.join(crate_dir);
        for path in rust_sources(&root)? {
            let rel = pathdiff(workspace_root, &path);
            let text = std::fs::read_to_string(&path)?;
            for cap in tool_re.captures_iter(&text) {
                let m = cap.get(1).unwrap();
                push(
                    codes,
                    m.as_str().to_string(),
                    Surface::Mcp,
                    &rel,
                    line_of(&text, m.start()),
                );
            }
        }
    }
    Ok(())
}

/// 1-based line number of a byte offset, for whole-file regex scans.
fn line_of(text: &str, offset: usize) -> u32 {
    (text[..offset].bytes().filter(|&b| b == b'\n').count() + 1) as u32
}

fn push(
    codes: &mut BTreeMap<String, Vec<Occurrence>>,
    code: String,
    surface: Surface,
    source: &str,
    line: u32,
) {
    codes.entry(code).or_default().push(Occurrence {
        surface,
        source: source.to_string(),
        line,
    });
}

fn render_index(codes: &BTreeMap<String, Vec<Occurrence>>) -> String {
    let mut out = String::new();
    out.push_str("# Error Code Index\n\n");
    out.push_str(
        "Typed error codes the static scan finds in the engine, the CLI \
         (`memstead-cli`), and the MCP server (`memstead-mcp`). Each \
         row lists the code, the surfaces that emit it, and the source \
         locations. Not indexed here: the registry-relayed codes the CLI \
         maps from memstead.io HTTP statuses during publish/install \
         (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, \
         `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, \
         `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and \
         `memstead-cli/src/commands/publish.rs`).\n\n",
    );
    out.push_str(&format!("**Distinct codes:** {}\n\n", codes.len()));
    out.push_str("| Code | Surfaces | Source locations |\n");
    out.push_str("|------|----------|------------------|\n");
    for (code, occurrences) in codes {
        let mut surfaces: Vec<Surface> = occurrences.iter().map(|o| o.surface).collect();
        surfaces.sort();
        surfaces.dedup();
        let surfaces_str: Vec<&str> = surfaces.iter().map(|s| s.label()).collect();
        let locations: Vec<String> = occurrences
            .iter()
            .map(|o| format!("`{}:{}`", o.source, o.line))
            .collect();
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            code,
            surfaces_str.join(", "),
            locations.join("<br>"),
        ));
    }
    out
}

fn rust_sources(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    visit(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn visit(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            visit(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn pathdiff(root: &Path, target: &Path) -> String {
    target
        .strip_prefix(root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| target.display().to_string())
}
