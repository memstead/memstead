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
        // `_with_details` is the payload-carrying form; omitting it hid
        // every code only that form emits (consistency-sweep 03/04
        // grading).
        Regex::new(r#"\btool_error(?:_with_payload|_with_details)?\(\s*"([A-Z][A-Z0-9_]+)"\s*,"#)
            .unwrap();
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
