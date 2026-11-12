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
//! * `tool_error(...)` / `tool_error_with_payload(...)` /
//!   `tool_error_with_details(...)` callsites in
//!   `memstead-mcp` — first positional argument.
//! * `CliError::new(_, "...", _)`, `.with_code("...")`, and
//!   `pub const ..._CODE: &str = "..."` constants in `memstead-cli`.
//!
//! The Registry HTTP error envelope belongs to the private
//! `memstead-registry` crate, which documents its own routes; the public
//! docs site carries no copy of that reference, so it is not scanned here.
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
    /// The function the site sits in (the nearest `fn` header above it),
    /// so a site is placed by file and function, never by a count per
    /// file a moved site would keep.
    pub function: String,
}

pub fn render(workspace_root: &Path) -> Result<String> {
    let codes = scan(workspace_root)?;
    Ok(render_index(&codes))
}

pub fn scan(workspace_root: &Path) -> Result<BTreeMap<String, Vec<Occurrence>>> {
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
    // The engine is the base crate, the projection crate (whose
    // refusals, the binding run's, carry their own typed codes), the
    // schema crate (package authoring) and the mem-repo crate (install).
    let mut paths = Vec::new();
    for root in [
        "crates/memstead-base/src",
        "crates/memstead-projection/src",
        "crates/memstead-schema/src",
        "crates/memstead-git-branch/src",
    ] {
        paths.extend(rust_sources(&workspace_root.join(root))?);
    }
    for path in paths {
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
                enclosing_fn(&text, m.start()),
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
                    push(codes, code, Surface::Engine, &rel, (idx + 1) as u32, "code");
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
    let root = workspace_root.join("crates/memstead-cli/src");
    let mut files = Vec::new();
    for path in rust_sources(&root)? {
        files.push((
            pathdiff(workspace_root, &path),
            std::fs::read_to_string(&path)?,
        ));
    }
    scan_cli_texts(&files, codes);
    Ok(())
}

/// The CLI scan over `(path, text)` pairs: the crate's sources, or a
/// self-test's fixture. A code is named by its literal or by a
/// `*_CODE` constant (`crate::INTERNAL_CODE`), which is resolved through
/// the constants the same texts declare, so a site that names the code
/// through a constant is placed like one that spells it.
fn scan_cli_texts(files: &[(String, String)], codes: &mut BTreeMap<String, Vec<Occurrence>>) {
    // Scanned against the whole file, not per line: rustfmt wraps call
    // arguments across lines, so the code literal often sits on its own
    // line below `CliError::new(` / `.with_code(`. `\s` and the negated
    // classes match newlines; the reported line is the literal's own.
    let const_re =
        Regex::new(r#"pub const ([A-Z_]+_CODE):\s*&str\s*=\s*"([A-Z][A-Z0-9_]+)""#).unwrap();
    let with_code_re = Regex::new(r#"\.with_code\(\s*"([A-Z][A-Z0-9_]+)"\s*\)"#).unwrap();
    let new_re = Regex::new(r#"CliError::new\([^)"]*?"([A-Z][A-Z0-9_]+)""#).unwrap();
    // A `CliError { code: "...", .. }` struct literal: an unquoted `code:`
    // field (a `json!` key is quoted and never matches).
    let literal_re = Regex::new(r#"\bcode:\s*"([A-Z][A-Z0-9_]+)""#).unwrap();
    // The same two shapes naming a constant instead of a literal.
    let new_const_re =
        Regex::new(r#"CliError::new\([^)"]*?\b(?:crate::)?([A-Z][A-Z0-9_]*_CODE)\b"#).unwrap();
    let literal_const_re = Regex::new(r#"\bcode:\s*(?:crate::)?([A-Z][A-Z0-9_]*_CODE)\b"#).unwrap();
    let consts: BTreeMap<String, String> = files
        .iter()
        .flat_map(|(_, text)| {
            const_re
                .captures_iter(text)
                .map(|c| (c[1].to_string(), c[2].to_string()))
                .collect::<Vec<_>>()
        })
        .collect();
    for (rel, text) in files {
        for cap in const_re.captures_iter(text) {
            let m = cap.get(2).unwrap();
            push(
                codes,
                m.as_str().to_string(),
                Surface::Cli,
                rel,
                line_of(text, m.start()),
                enclosing_fn(text, m.start()),
            );
        }
        for re in [&with_code_re, &new_re, &literal_re] {
            for cap in re.captures_iter(text) {
                let m = cap.get(1).unwrap();
                push(
                    codes,
                    m.as_str().to_string(),
                    Surface::Cli,
                    rel,
                    line_of(text, m.start()),
                    enclosing_fn(text, m.start()),
                );
            }
        }
        for re in [&new_const_re, &literal_const_re] {
            for cap in re.captures_iter(text) {
                let m = cap.get(1).unwrap();
                if let Some(code) = consts.get(m.as_str()) {
                    push(
                        codes,
                        code.clone(),
                        Surface::Cli,
                        rel,
                        line_of(text, m.start()),
                        enclosing_fn(text, m.start()),
                    );
                }
            }
        }
    }
}

fn scan_mcp_codes(
    workspace_root: &Path,
    codes: &mut BTreeMap<String, Vec<Occurrence>>,
) -> Result<()> {
    let root = workspace_root.join("crates/memstead-mcp/src");
    let mut files = Vec::new();
    for path in rust_sources(&root)? {
        files.push((
            pathdiff(workspace_root, &path),
            std::fs::read_to_string(&path)?,
        ));
    }
    scan_mcp_texts(&files, codes);
    Ok(())
}

/// The MCP scan over `(path, text)` pairs.
fn scan_mcp_texts(files: &[(String, String)], codes: &mut BTreeMap<String, Vec<Occurrence>>) {
    // Whole-file scan for the same reason as the CLI's: rustfmt may put
    // the code literal on the line after `tool_error(`.
    let tool_re =
        // `_with_details` is the payload-carrying form; omitting it hid
        // every code only that form emits.
        Regex::new(r#"\btool_error(?:_with_payload|_with_details)?\(\s*"([A-Z][A-Z0-9_]+)"\s*,"#)
            .unwrap();
    for (rel, text) in files {
        for cap in tool_re.captures_iter(text) {
            let m = cap.get(1).unwrap();
            push(
                codes,
                m.as_str().to_string(),
                Surface::Mcp,
                rel,
                line_of(text, m.start()),
                enclosing_fn(text, m.start()),
            );
        }
    }
}

/// The name of the function a byte offset sits in: the nearest `fn`
/// header above it. A closure is not a function, so a site in a closure
/// is placed by the function that holds the closure.
fn enclosing_fn(text: &str, offset: usize) -> String {
    let fn_re = Regex::new(r#"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[<(]"#).unwrap();
    fn_re
        .captures_iter(&text[..offset])
        .last()
        .map(|c| c[1].to_string())
        .unwrap_or_default()
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
    function: impl Into<String>,
) {
    codes.entry(code).or_default().push(Occurrence {
        surface,
        source: source.to_string(),
        line,
        function: function.into(),
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
         maps from registry HTTP statuses during publish/install \
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

#[cfg(test)]
mod code_vocabulary_tests {
    //! The MCP prose names error codes. Two things can go wrong with that,
    //! and both did: a code the engine can produce goes unnamed, so an agent
    //! hitting it has no entry telling it what the refusal carries; or the
    //! prose names a code nothing can produce, so an agent is told to expect
    //! a refusal that will never arrive.
    //!
    //! Both are checked here rather than against a hand-maintained list,
    //! because the hand-maintained list is what failed. `tool_surface.rs`
    //! holds `STRUCTURED_ERROR_CODES` and asserts every entry appears in the
    //! prose; that direction cannot notice a code missing from the list
    //! itself, and on 2026-08-26 it was missing `INVALID_FIELD_VALUE` while
    //! carrying five codes with no construction site anywhere.
    //!
    //! The index these check against is the same scan the published Error
    //! Code Index is rendered from: every `fn code()` body in
    //! `memstead-base`, every `tool_error(...)` callsite in `memstead-mcp`,
    //! and the CLI's own codes.

    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits one level under the workspace root")
            .to_path_buf()
    }

    fn index(root: &Path) -> BTreeSet<String> {
        super::scan(root)
            .expect("the error-code scan must run")
            .into_keys()
            .collect()
    }

    /// Every file the MCP servers compile their prose in from.
    fn mcp_prose(root: &Path) -> String {
        let dir = root.join("crates/memstead-mcp/descriptions");
        let mut text = String::new();
        let mut files = 0usize;
        for surface in ["full"] {
            let d = dir.join(surface);
            for entry in
                std::fs::read_dir(&d).unwrap_or_else(|e| panic!("reading {}: {e}", d.display()))
            {
                let path = entry.expect("a readable dir entry").path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                text.push_str(&std::fs::read_to_string(&path).expect("a readable file"));
                text.push('\n');
                files += 1;
            }
        }
        assert!(
            files >= 20,
            "the description walk found only {files} files — a walk that reaches \
             (almost) nothing is worse than an absent one"
        );
        text
    }

    /// The codes `ValidationError::code()` can return: the schema-conformance
    /// vocabulary, derived from the engine rather than restated.
    fn conformance_codes(root: &Path) -> BTreeSet<String> {
        let src = root.join("crates/memstead-base/src/runtime_validator.rs");
        let text = std::fs::read_to_string(&src)
            .unwrap_or_else(|e| panic!("reading {}: {e}", src.display()));
        let re =
            regex::Regex::new(r#"ValidationError::\w+\s*\{[^}]*\}\s*=>\s*"([A-Z][A-Z0-9_]+)""#)
                .unwrap();
        let codes: BTreeSet<String> = re.captures_iter(&text).map(|c| c[1].to_string()).collect();
        assert!(
            codes.len() >= 5,
            "found only {} ValidationError codes in {} — the derivation broke, \
             which is a failure and not a pass",
            codes.len(),
            src.display()
        );
        codes
    }

    /// The two code lists the full server's instructions render, parsed from
    /// their own anchors so neither is restated here.
    fn instruction_lists(root: &Path) -> (Vec<String>, Vec<String>) {
        let p = root.join("crates/memstead-mcp/descriptions/full/server-instructions-head.md");
        let text =
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()));
        let split = |s: &str| -> Vec<String> {
            s.split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect()
        };
        let recovery = regex::Regex::new(r"carry recovery payloads as a fallback \(([^)]*)\)")
            .unwrap()
            .captures(&text)
            .map(|c| split(&c[1]))
            .expect("the recovery-payload list must still be findable by its anchor");
        let enumerated = regex::Regex::new(r"Error codes: ([A-Z0-9_, ]+?)\.")
            .unwrap()
            .captures(&text)
            .map(|c| split(&c[1]))
            .expect("the error-code enumeration must still be findable by its anchor");
        (recovery, enumerated)
    }

    #[test]
    fn the_instructions_name_no_code_the_engine_cannot_produce() {
        let root = workspace_root();
        let known = index(&root);
        let (recovery, enumerated) = instruction_lists(&root);
        let mut phantom: Vec<String> = Vec::new();
        for (list, code) in recovery
            .iter()
            .map(|c| ("recovery-payload list", c))
            .chain(enumerated.iter().map(|c| ("error-code enumeration", c)))
        {
            if !known.contains(code) {
                phantom.push(format!("{list}: {code}"));
            }
        }
        assert!(
            phantom.is_empty(),
            "the MCP server instructions name {} code(s) with no construction site \
             anywhere in the workspace, so an agent is told to expect a refusal that \
             cannot arrive:\n  {}",
            phantom.len(),
            phantom.join("\n  ")
        );
    }

    /// The per-tool excerpts of the conformance vocabulary, and the
    /// hand-maintained list the tool-surface gate reads.
    ///
    /// Their contract is one-directional: an excerpt may omit a code (the two
    /// capped descriptions have no room for the full set) but may never name
    /// one the workspace cannot construct, and must carry the elision marker
    /// that declares it an excerpt. The canonical list is held to both
    /// directions by the test above.
    fn vocabulary_excerpts(root: &Path) -> Vec<(String, String)> {
        let files = [
            "crates/memstead-mcp/descriptions/full/memstead_create.md",
            "crates/memstead-mcp/descriptions/full/memstead_update.md",
            "crates/memstead-mcp/src/tools/mutation.rs",
            "crates/memstead-mcp/tests/tool_surface.rs",
        ];
        let re = regex::Regex::new(
            r"(?:Schema-bound (?:failures|errors)|refuse[sd]? (?:with the IDENTICAL typed envelope a real call would return|on section/field grounds)) \(([^)]*)\)",
        )
        .unwrap();
        let hand_list =
            regex::Regex::new(r"(?s)const STRUCTURED_ERROR_CODES: &\[&str\] = &\[(.*?)\];")
                .unwrap();
        let mut out = Vec::new();
        for rel in files {
            let path = root.join(rel);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            if rel.ends_with("tool_surface.rs") {
                let list = hand_list
                    .captures(&text)
                    .map(|c| c[1].to_string())
                    .expect("STRUCTURED_ERROR_CODES must still be findable");
                out.push((rel.to_string(), list));
                continue;
            }
            for c in re.captures_iter(&text) {
                out.push((rel.to_string(), c[1].to_string()));
            }
        }
        assert!(
            out.len() >= 5,
            "the excerpt walk found only {} list(s) — a walk that finds \
             (almost) nothing is worse than an absent one",
            out.len()
        );
        out
    }

    #[test]
    fn no_vocabulary_excerpt_names_a_code_the_engine_cannot_produce() {
        let root = workspace_root();
        let known = index(&root);
        let code = regex::Regex::new(r"[A-Z][A-Z0-9_]{3,}").unwrap();
        let mut phantom = Vec::new();
        for (where_, list) in vocabulary_excerpts(&root) {
            for m in code.find_iter(&list) {
                if !known.contains(m.as_str()) {
                    phantom.push(format!("{where_}: {}", m.as_str()));
                }
            }
        }
        assert!(
            phantom.is_empty(),
            "{} code(s) are named in a vocabulary excerpt with no construction \
             site anywhere in the workspace:\n  {}",
            phantom.len(),
            phantom.join("\n  ")
        );
    }

    #[test]
    fn every_per_tool_excerpt_declares_itself_one() {
        let root = workspace_root();
        let missing: Vec<String> = vocabulary_excerpts(&root)
            .into_iter()
            // The hand-maintained list is a test fixture, not prose an agent
            // reads, so it carries no marker and needs none.
            .filter(|(w, _)| !w.ends_with("tool_surface.rs"))
            .filter(|(_, list)| !list.contains('…'))
            .map(|(w, _)| w)
            .collect();
        assert!(
            missing.is_empty(),
            "{} per-tool list(s) render the conformance vocabulary without the \
             elision marker that declares them excerpts, so they read as \
             complete sets they are not:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }

    #[test]
    fn the_recovery_payload_list_names_every_schema_conformance_code() {
        // Scoped to the ONE rendering that claims to be the vocabulary. An
        // earlier version of this check concatenated every description file
        // and asked only whether each code appeared somewhere in the blob;
        // that is satisfied by a code sitting in the `Error codes:`
        // enumeration while the recovery-payload list beside it stays
        // incomplete, which is exactly the state it failed to catch.
        //
        // The per-tool descriptions are deliberately NOT checked this way.
        // They sit against a hard byte cap, they name the codes their own
        // path most often hits, and each points at this list. Widening the
        // check to them would demand bytes that do not exist.
        let root = workspace_root();
        let (recovery, _) = instruction_lists(&root);
        let missing: Vec<String> = conformance_codes(&root)
            .into_iter()
            .filter(|c| !recovery.iter().any(|r| r == c))
            .collect();
        assert!(
            missing.is_empty(),
            "the engine can return {} schema-conformance code(s) with a recovery \
             payload that the MCP server instructions' recovery-payload list does \
             not name, so an agent hitting one is not told the refusal carries \
             `details` it can fix from:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }

    #[test]
    fn the_mcp_prose_names_every_schema_conformance_code() {
        let root = workspace_root();
        let prose = mcp_prose(&root);
        let missing: Vec<String> = conformance_codes(&root)
            .into_iter()
            .filter(|c| !prose.contains(c.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "the engine can return {} schema-conformance code(s) that no MCP \
             description or instruction names anywhere:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }
}

/// Which codes a surface (the CLI, the MCP server) may raise by itself.
///
/// The rule: a surface builds a code of its own only for a defect in the
/// syntax of its own input, or for a failure of its own operation that is
/// not workspace, mem or entity state (a network call, a key file, the
/// server process). An error about workspace, mem or entity state is the
/// engine's to raise. Every code the scan finds at a surface and not in
/// the engine is in exactly one list below; a new one fails the test until
/// it is placed, and `MOVE_TO_ENGINE` lists the state errors still built at
/// a surface, which the error-payload convergence empties.
#[cfg(test)]
mod surface_codes {
    use super::{Occurrence, Surface, scan, scan_cli_texts, scan_mcp_texts};
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;

    /// A defect in the syntax of the surface's own input.
    const INPUT_SYNTAX: &[&str] = &[
        "INVALID_CHECK_KIND",
        "INVALID_DOMAIN",
        "INVALID_IDENTITY",
        "INVALID_ROLE",
        "INVALID_VERDICT",
        "INVALID_VERSION",
        "PROJECTION_BRIEF_BINDING_REQUIRED",
        "PROJECTION_INVALID_DISPOSITIONS",
        "PROJECTION_INVALID_EXCLUSIONS",
    ];

    /// A failure of the surface's own operation, not workspace, mem or
    /// entity state: network and registry calls, signing keys, the archive
    /// file a command reads or writes, the server process, the outcome
    /// envelope of a command.
    const SURFACE_OPERATION: &[&str] = &[
        "ARCHIVE_READ_FAILED",
        "DOMAIN_KEYGEN_FAILED",
        "DOMAIN_KEY_NOT_FOUND",
        "ENGINE_LOCK_POISONED",
        "INTERNAL",
        "INTERNAL_IO_ERROR",
        "LOGIN_FAILED",
        "MEM_REPO_INIT_FAILED",
        "NETWORK_ERROR",
        "NOT_AUTHENTICATED",
        "OUTER_GITIGNORE_HOME_REFUSED",
        "REGISTRY_AUTH_CONFIG_UNAVAILABLE",
        "REGISTRY_ERROR",
        "REGISTRY_MALFORMED_RESPONSE",
        "REGISTRY_NOT_CONFIGURED",
        "TOOL_DISABLED",
    ];

    /// Workspace, mem or entity state still built at a surface: each moves
    /// into the engine.
    const MOVE_TO_ENGINE: &[&str] = &[];

    /// Engine codes a surface still builds by hand at some site instead of
    /// raising the engine's value: each site moves to the engine's value
    /// (or, for INVALID_INPUT, is a defect in the surface's own input
    /// syntax spelled with the engine's code).
    const ENGINE_CODE_BUILT_AT_SURFACE: &[&str] = &["INVALID_INPUT"];

    /// Where a surface spells the engine's `INVALID_INPUT` for a defect in
    /// its own input syntax (a flag's shape, a `--from` file, a batch
    /// envelope), by file, function and how many sites that function
    /// holds. The code alone cannot tell syntax from state, so each site
    /// is placed: a new site, or one moved into another function, fails
    /// the test below until it is listed here as syntax, or raises the
    /// engine's value.
    const INVALID_INPUT_SYNTAX_SITES: &[(&str, &str, usize)] = &[
        (
            "crates/memstead-cli/src/commands/admin.rs",
            "run_denylist",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/admin.rs",
            "run_takedown",
            2,
        ),
        ("crates/memstead-cli/src/commands/anchors.rs", "run", 2),
        (
            "crates/memstead-cli/src/commands/batch.rs",
            "parse_batch_envelope",
            5,
        ),
        (
            "crates/memstead-cli/src/commands/batch_create.rs",
            "build_create_args",
            1,
        ),
        ("crates/memstead-cli/src/commands/batch_create.rs", "run", 1),
        ("crates/memstead-cli/src/commands/batch_relate.rs", "run", 1),
        (
            "crates/memstead-cli/src/commands/batch_update.rs",
            "build_entry_parse_error",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/batch_update.rs",
            "build_update_args",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/check.rs",
            "parse_instant",
            1,
        ),
        ("crates/memstead-cli/src/commands/check.rs", "run_batch", 3),
        (
            "crates/memstead-cli/src/commands/conflicts.rs",
            "dispatch",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/create.rs",
            "parse_anchor_list",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/create.rs",
            "parse_kv_list",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/create.rs",
            "parse_relation_list",
            1,
        ),
        ("crates/memstead-cli/src/commands/create.rs", "run", 6),
        ("crates/memstead-cli/src/commands/due.rs", "run", 3),
        ("crates/memstead-cli/src/commands/export.rs", "run", 4),
        ("crates/memstead-cli/src/commands/export.rs", "run_json", 2),
        ("crates/memstead-cli/src/commands/install.rs", "run", 1),
        (
            "crates/memstead-cli/src/commands/mem.rs",
            "invalid_input_error",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/mem.rs",
            "run_set_subject",
            1,
        ),
        ("crates/memstead-cli/src/commands/mem_repo.rs", "init", 1),
        (
            "crates/memstead-cli/src/commands/mod.rs",
            "parse_filter_arg",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/projection.rs",
            "batch_invalid",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/proposal.rs",
            "run_merge",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/publish.rs",
            "run_with_root",
            3,
        ),
        (
            "crates/memstead-cli/src/commands/quickstart.rs",
            "prompt_agents",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/quickstart.rs",
            "read_agent_config",
            3,
        ),
        (
            "crates/memstead-cli/src/commands/quickstart.rs",
            "resolve_mem_name",
            1,
        ),
        ("crates/memstead-cli/src/commands/quickstart.rs", "run", 1),
        (
            "crates/memstead-cli/src/commands/quickstart.rs",
            "wire_agent",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/relate.rs",
            "resolve_slot",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/retype.rs",
            "parse_section_map",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/schema.rs",
            "resolve_builtin_ref",
            1,
        ),
        ("crates/memstead-cli/src/commands/schema.rs", "run", 1),
        (
            "crates/memstead-cli/src/commands/schema.rs",
            "scaffold_new",
            1,
        ),
        ("crates/memstead-cli/src/commands/unpublish.rs", "run", 1),
        (
            "crates/memstead-cli/src/commands/update.rs",
            "parse_anchor_unset_list",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/update.rs",
            "parse_declare_relations",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/update.rs",
            "parse_kv_list",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/update.rs",
            "parse_patch_list_combined",
            3,
        ),
        ("crates/memstead-cli/src/commands/update.rs", "run", 4),
        (
            "crates/memstead-cli/src/commands/workspace.rs",
            "set_mutations",
            1,
        ),
        ("crates/memstead-cli/src/main.rs", "run", 1),
        (
            "crates/memstead-mcp/src/server/mem_tools.rs",
            "memstead_mem_configure",
            1,
        ),
        (
            "crates/memstead-mcp/src/server/mem_tools.rs",
            "memstead_mem_create_unified",
            1,
        ),
        (
            "crates/memstead-mcp/src/server/mem_tools.rs",
            "memstead_mem_set_version",
            1,
        ),
        (
            "crates/memstead-mcp/src/server/read_tools.rs",
            "memstead_schema",
            3,
        ),
        (
            "crates/memstead-mcp/src/server/responses.rs",
            "validate_note",
            1,
        ),
        (
            "crates/memstead-mcp/src/server/write_tools.rs",
            "memstead_relate",
            1,
        ),
        (
            "crates/memstead-mcp/src/server/write_tools.rs",
            "memstead_reload",
            1,
        ),
    ];

    /// Where the CLI raises `INTERNAL` (spelled, or named through
    /// `crate::INTERNAL_CODE`): a failure of its own operation (a file it
    /// writes, a temporary file, an archive it reads), never workspace,
    /// mem or entity state. Placed by file and function like the sites
    /// above; a use anywhere else fails the test below.
    const INTERNAL_SITES: &[(&str, &str, usize)] = &[
        (
            "crates/memstead-cli/src/commands/admin.rs",
            "map_admin_error",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/export.rs",
            "make_self_contained_on_disk",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/export.rs",
            "run_mem_filesystem",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/init.rs",
            "ensure_empty",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/init.rs",
            "find_ancestor_workspace",
            1,
        ),
        ("crates/memstead-cli/src/commands/init.rs", "run", 2),
        (
            "crates/memstead-cli/src/commands/publish.rs",
            "find_filesystem_workspace_root",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/publish.rs",
            "stage_bytes_to_tempfile",
            2,
        ),
        (
            "crates/memstead-cli/src/commands/quickstart.rs",
            "default_schema_pin",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/schema.rs",
            "scaffold_new",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/unpublish.rs",
            "map_unpublish_error",
            1,
        ),
        (
            "crates/memstead-cli/src/commands/workspace.rs",
            "require_workspace_root",
            1,
        ),
        (
            "crates/memstead-cli/src/outer_gitignore.rs",
            "write_to_outer_gitignore",
            2,
        ),
    ];

    /// Every site a surface builds `code` at, by file and function, with
    /// how many sites that function holds.
    fn surface_sites(
        codes: &BTreeMap<String, Vec<Occurrence>>,
        code: &str,
    ) -> BTreeMap<(String, String), usize> {
        let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
        for o in codes.get(code).into_iter().flatten() {
            if o.surface != Surface::Engine && !o.function.is_empty() {
                *found
                    .entry((o.source.clone(), o.function.clone()))
                    .or_default() += 1;
            }
        }
        found
    }

    fn listed(sites: &[(&str, &str, usize)]) -> BTreeMap<(String, String), usize> {
        sites
            .iter()
            .map(|(f, func, n)| ((f.to_string(), func.to_string()), *n))
            .collect()
    }

    #[test]
    fn invalid_input_is_built_at_a_surface_only_at_the_placed_syntax_sites() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let codes = scan(&root).unwrap();
        assert_eq!(
            surface_sites(&codes, "INVALID_INPUT"),
            listed(INVALID_INPUT_SYNTAX_SITES),
            "an INVALID_INPUT site a surface builds is not placed: list it as input syntax, \
             or raise the engine's value if it reports workspace, mem or entity state"
        );
    }

    #[test]
    fn internal_is_raised_at_a_surface_only_at_the_placed_sites() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let codes = scan(&root).unwrap();
        assert_eq!(
            surface_sites(&codes, "INTERNAL"),
            listed(INTERNAL_SITES),
            "an INTERNAL site is not placed: list it as the surface's own operation, or raise \
             the engine's value if it reports workspace, mem or entity state"
        );
    }

    /// The placement gate fails on what it claims to catch: an
    /// `INVALID_INPUT` site moved to another function of its file, and an
    /// `INTERNAL_CODE` use at a site no list allows; and it places a site
    /// named through `crate::INTERNAL_CODE` as `INTERNAL`.
    #[test]
    fn the_placement_gate_fails_on_a_moved_site_and_an_unplaced_internal_use() {
        let lib = (
            "crates/memstead-cli/src/lib.rs".to_string(),
            "pub const INTERNAL_CODE: &str = \"INTERNAL\";\n".to_string(),
        );
        let placed_here = (
            "crates/memstead-cli/src/commands/probe.rs".to_string(),
            "fn parse_flag() {\n    CliError::new(ExitKind::Validation, \"INVALID_INPUT\", m)\n}\n\
             fn run() {\n    other();\n}\n"
                .to_string(),
        );
        let allowed = &[("crates/memstead-cli/src/commands/probe.rs", "parse_flag", 1)];
        let mut codes = BTreeMap::new();
        scan_cli_texts(&[lib.clone(), placed_here], &mut codes);
        assert_eq!(surface_sites(&codes, "INVALID_INPUT"), listed(allowed));

        let moved = (
            "crates/memstead-cli/src/commands/probe.rs".to_string(),
            "fn parse_flag() {\n    other();\n}\n\
             fn run() {\n    CliError::new(ExitKind::Validation, \"INVALID_INPUT\", m)\n}\n"
                .to_string(),
        );
        let mut codes = BTreeMap::new();
        scan_cli_texts(&[lib.clone(), moved], &mut codes);
        assert_ne!(
            surface_sites(&codes, "INVALID_INPUT"),
            listed(allowed),
            "a site moved within its file must fail the placement"
        );

        let internal = (
            "crates/memstead-cli/src/commands/probe.rs".to_string(),
            "fn run() {\n    CliError::new(ExitKind::Generic, crate::INTERNAL_CODE, m)\n}\n\
             fn other() {\n    CliError { code: crate::INTERNAL_CODE, kind, message, details: None }\n}\n"
                .to_string(),
        );
        let mut codes = BTreeMap::new();
        scan_cli_texts(&[lib, internal], &mut codes);
        let found = surface_sites(&codes, "INTERNAL");
        assert_eq!(found.len(), 2, "both constant uses are found: {found:?}");
        assert_ne!(
            found,
            listed(&[("crates/memstead-cli/src/commands/probe.rs", "run", 1)]),
            "an INTERNAL_CODE use at a site not allowed must fail the placement"
        );
        let mut mcp = BTreeMap::new();
        scan_mcp_texts(&[], &mut mcp);
        assert!(mcp.is_empty());
    }

    #[test]
    fn engine_codes_built_by_hand_at_a_surface_are_exactly_the_listed_ones() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let codes = scan(&root).unwrap();
        let found: BTreeSet<&str> = codes
            .iter()
            .filter(|(_, occ)| {
                occ.iter().any(|o| o.surface == Surface::Engine)
                    && occ.iter().any(|o| o.surface != Surface::Engine)
            })
            .map(|(c, _)| c.as_str())
            .collect();
        let listed: BTreeSet<&str> = ENGINE_CODE_BUILT_AT_SURFACE.iter().copied().collect();
        assert_eq!(
            found, listed,
            "an engine code built by hand at a surface: raise the engine's value instead"
        );
    }

    #[test]
    fn every_surface_built_code_is_placed_and_every_placed_code_exists() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let codes = scan(&root).unwrap();
        let surface_only: BTreeSet<&str> = codes
            .iter()
            .filter(|(_, occ)| {
                occ.iter().all(|o| o.surface != Surface::Engine)
                    && occ.iter().any(|o| o.surface != Surface::Engine)
            })
            .map(|(c, _)| c.as_str())
            .collect();
        let lists = [INPUT_SYNTAX, SURFACE_OPERATION, MOVE_TO_ENGINE];
        let mut placed: BTreeSet<&str> = BTreeSet::new();
        for list in lists {
            for code in list {
                assert!(placed.insert(code), "{code} is in two lists");
            }
        }
        let unplaced: Vec<_> = surface_only.difference(&placed).collect();
        assert!(
            unplaced.is_empty(),
            "a surface builds a code of its own that no list places: {unplaced:?}. \
             A defect in the surface's own input syntax goes in INPUT_SYNTAX, its own \
             operation in SURFACE_OPERATION; workspace, mem or entity state is the engine's."
        );
        let stale: Vec<_> = placed.difference(&surface_only).collect();
        assert!(
            stale.is_empty(),
            "listed codes no surface builds by itself any more (moved into the engine, \
             or gone): {stale:?}; drop them from the lists"
        );
    }
}
