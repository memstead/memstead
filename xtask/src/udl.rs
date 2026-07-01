//! Render the UniFFI UDL file as a Markdown reference. The UDL syntax
//! is small and line-disciplined: every top-level `namespace`,
//! `dictionary`, `interface`, or `[Enum]` / `[Error]` interface starts at
//! column zero with the keyword (or the attribute on its own line) and
//! ends with `};` at column zero. The parser collects preceding
//! line-comments as the section blurb and emits each block as its own
//! Markdown section with a code-fenced body.

use std::path::Path;

use anyhow::{Context, Result};

pub fn render_from_file(udl_path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(udl_path)
        .with_context(|| format!("reading {}", udl_path.display()))?;
    Ok(render(&text))
}

/// Method names declared on the `Engine` interface. The UDL declares
/// each method on its own line as `Type? name(args);`, optionally
/// preceded by attribute lines like `[Throws=MemsteadError]`. The
/// `constructor` is included so callers can choose to filter it out.
pub fn engine_methods(udl: &str) -> Vec<String> {
    let mut methods: Vec<String> = Vec::new();
    let mut in_engine = false;
    for line in udl.lines() {
        let trimmed = line.trim();
        if !in_engine {
            if trimmed == "interface Engine {" {
                in_engine = true;
            }
            continue;
        }
        if trimmed == "};" {
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('[') {
            continue;
        }
        if let Some(name) = method_name(trimmed) {
            methods.push(name);
        }
    }
    methods
}

fn method_name(line: &str) -> Option<String> {
    let paren = line.find('(')?;
    let before = &line[..paren];
    let last_token = before.split_whitespace().last()?;
    if last_token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(last_token.to_string())
    } else {
        None
    }
}

pub fn render(udl: &str) -> String {
    let mut out = String::new();
    out.push_str("# UniFFI surface\n\n");
    out.push_str(
        "Auto-generated from the engine's UniFFI UDL. Each top-level \
         declaration (`namespace`, `dictionary`, `interface`, `[Enum]` \
         interface, `[Error]` interface) appears below with its full body \
         and any preceding doc-comment block.\n\n",
    );

    let mut pending_comment: Vec<String> = Vec::new();
    let mut pending_attr: Option<String> = None;
    let mut block_title: Option<String> = None;
    let mut block_body: Vec<String> = Vec::new();

    for line in udl.lines() {
        let trimmed = line.trim();

        if block_title.is_none() {
            if trimmed.starts_with("//") {
                pending_comment.push(strip_comment_prefix(line));
                continue;
            }
            if trimmed.is_empty() {
                if !pending_comment.is_empty() {
                    pending_comment.push(String::new());
                }
                continue;
            }
            if trimmed == "[Error]" || trimmed == "[Enum]" {
                pending_attr = Some(trimmed.to_string());
                continue;
            }
            if let Some(name) = block_header(trimmed) {
                let title = match pending_attr.take() {
                    Some(attr) => format!("{} {}", attr, name),
                    None => name,
                };
                block_title = Some(title);
                block_body.push(line.to_string());
                continue;
            }
            // Stray line at top level: drop accumulated comment and
            // attribute to avoid attaching unrelated context to the next
            // block.
            pending_comment.clear();
            pending_attr = None;
            continue;
        }

        block_body.push(line.to_string());
        if trimmed == "};" {
            let title = block_title.take().expect("title present inside block");
            emit_section(&mut out, &title, &pending_comment, &block_body);
            pending_comment.clear();
            block_body.clear();
        }
    }

    out
}

fn strip_comment_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("// ")
        .or_else(|| trimmed.strip_prefix("//"))
        .unwrap_or(trimmed)
        .to_string()
}

fn block_header(trimmed: &str) -> Option<String> {
    for prefix in ["namespace ", "dictionary ", "interface "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let name: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '{' && *c != ';')
                .collect();
            if !name.is_empty() {
                return Some(format!("{}{}", prefix, name));
            }
        }
    }
    None
}

fn emit_section(out: &mut String, title: &str, comment: &[String], body: &[String]) {
    out.push_str("## `");
    out.push_str(title);
    out.push_str("`\n\n");

    let blurb: Vec<&String> = comment
        .iter()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with("---"))
        .collect();
    if !blurb.is_empty() {
        for line in &blurb {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("```idl\n");
    for line in body {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("```\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_namespace_dictionary_interface_and_error() {
        let udl = r#"
namespace foo {
    string version();
};

[Error]
interface FooError {
    NotFound(string message);
};

dictionary Pair {
    string key;
    string value;
};
"#;
        let md = render(udl);
        assert!(md.contains("## `namespace foo`"));
        assert!(md.contains("## `[Error] interface FooError`"));
        assert!(md.contains("## `dictionary Pair`"));
        assert!(md.contains("```idl"));
    }

    #[test]
    fn preserves_preceding_doc_comment() {
        let udl = "// Says hi.\nnamespace greet {\n    string hi();\n};\n";
        let md = render(udl);
        assert!(md.contains("Says hi."));
    }

    #[test]
    fn deterministic_across_runs() {
        let udl = include_str!("../../crates/memstead-swift/src/memstead.udl");
        assert_eq!(render(udl), render(udl));
    }

    #[test]
    fn engine_methods_lifts_method_names() {
        let udl = include_str!("../../crates/memstead-swift/src/memstead.udl");
        let methods = engine_methods(udl);
        assert!(methods.contains(&"get_entity".to_string()));
        assert!(methods.contains(&"search".to_string()));
        assert!(methods.contains(&"changes_since".to_string()));
        assert!(methods.contains(&"constructor".to_string()));
    }
}
