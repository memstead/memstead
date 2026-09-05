//! The binding's intent says only what the destination vocabulary carries.
//!
//! An intent is prose for the agent, and an agent reads an all-caps token
//! in it (`DEPENDS_ON`, `USES`) as a relationship of the destination mem's
//! schema — an edge it may write. A token the schema does not declare is
//! therefore a fact the intent asserts about a vocabulary that does not
//! hold it: the dogfood engine binding once named `PROVIDED_BY` against
//! the software schema and a sync agent read it as an edge to write. The
//! rule here is generic: the vocabulary is read from whatever schema the
//! destination mem pins, never from a built-in list.
//!
//! Two postures, one rule ([`intent_findings`]):
//!
//! * **Load reports.** A binding that already carries an unknown token keeps
//!   loading; every brief and the verify report carry the finding
//!   ([`BINDING_INTENT_UNKNOWN_RELATIONSHIP_CODE`]) so the defect surfaces
//!   without breaking the pipeline.
//! * **Write refuses.** `projection init` and `projection edit` refuse to
//!   write an intent that names one, with the same code.
//!
//! Two token shapes are prose, never relationship claims, and are exempt: a
//! file name (`CLAUDE.md`, `Cargo.toml` — the extension that follows says
//! so) and the protocol / format acronyms in [`PROSE_ACRONYMS`], which an
//! intent names as words. A vocabulary relationship always wins over the
//! exemption: a schema that declares `API` as a relationship gets it
//! recognised as one.

use serde::Serialize;

use memstead_schema::Schema;

/// The finding code — one literal, indexed by the generated error index.
pub const BINDING_INTENT_UNKNOWN_RELATIONSHIP_CODE: &str = "BINDING_INTENT_UNKNOWN_RELATIONSHIP";

/// Protocol and format acronyms an intent names as prose. Not a
/// relationship vocabulary: a token here is exempt from the rule only when
/// the destination schema does not declare it as a relationship.
pub const PROSE_ACRONYMS: &[&str] = &[
    "API", "CLI", "CSV", "HTML", "HTTP", "HTTPS", "JSON", "LLM", "MCP", "SSE", "TOML", "URL",
    "YAML",
];

/// One unknown relationship token in a binding's intent, with the
/// vocabulary it was checked against so the reader can see the mismatch
/// without opening the schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IntentFinding {
    /// Always [`BINDING_INTENT_UNKNOWN_RELATIONSHIP_CODE`].
    pub code: &'static str,
    /// The token as written in the intent.
    pub token: String,
    /// The destination schema pin (`software@0.5.0`) whose vocabulary was read.
    pub schema: String,
    /// The relationship names that schema declares — main vocabulary and
    /// cross-mem blocks alike, in declaration order.
    pub vocabulary: Vec<String>,
}

impl std::fmt::Display for IntentFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: the intent names `{}`, which is not a relationship of the destination \
             schema `{}` (vocabulary: {})",
            self.code,
            self.token,
            self.schema,
            self.vocabulary.join(", ")
        )
    }
}

/// Every relationship name a schema declares: the main vocabulary and each
/// cross-mem block, in declaration order, deduplicated. Only names an
/// intent could actually write are listed: the loader injects a synthetic
/// `_default` definition that no token can ever match, and it would be
/// noise in every finding.
pub fn relationship_vocabulary(schema: &Schema) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let defs = schema.manifest.relationships.definitions.iter().chain(
        schema
            .manifest
            .cross_mem_relationships
            .iter()
            .flat_map(|block| block.definitions.iter()),
    );
    for def in defs {
        if is_relationship_shaped(&def.name) && !names.contains(&def.name) {
            names.push(def.name.clone());
        }
    }
    names
}

/// Check an intent against a destination schema's vocabulary. Every
/// all-caps token of three or more characters (letters, digits and
/// underscores, starting with a letter) is read as a relationship name;
/// one the schema does not declare is a finding, reported once per token
/// in order of first appearance. `None` or an empty intent yields nothing.
pub fn intent_findings(
    intent: Option<&str>,
    schema_pin: &str,
    schema: &Schema,
) -> Vec<IntentFinding> {
    let Some(intent) = intent else {
        return Vec::new();
    };
    let vocabulary = relationship_vocabulary(schema);
    let mut findings: Vec<IntentFinding> = Vec::new();
    for token in unknown_tokens(intent, &vocabulary) {
        findings.push(IntentFinding {
            code: BINDING_INTENT_UNKNOWN_RELATIONSHIP_CODE,
            token,
            schema: schema_pin.to_string(),
            vocabulary: vocabulary.clone(),
        });
    }
    findings
}

/// The findings for a binding as loaded: its intent against the schema the
/// destination mem pins in this engine. A destination that is not mounted,
/// or carries no schema, has no vocabulary to read, so nothing is reported
/// — the brief's absent-destination note carries that case.
pub fn binding_intent_findings(
    engine: &crate::Engine,
    destination_mem: &str,
    intent: Option<&str>,
) -> Vec<IntentFinding> {
    let Some(pin) = engine.schema_pin(destination_mem) else {
        return Vec::new();
    };
    let Some(schema) = engine.schema_for(destination_mem) else {
        return Vec::new();
    };
    intent_findings(intent, &pin.as_display(), &schema)
}

/// The all-caps tokens of `intent` that are neither in `vocabulary` nor
/// exempt as prose, deduplicated in order of first appearance. Split out
/// from [`intent_findings`] so the tokenizer is testable without a schema.
pub fn unknown_tokens(intent: &str, vocabulary: &[String]) -> Vec<String> {
    let bytes = intent.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if !is_token_byte(c) {
            i += 1;
            continue;
        }
        // Consume one maximal run of token bytes: a word boundary on both
        // sides, so `PROVIDED_BY` is one token and `memstead-pro` none.
        let start = i;
        while i < bytes.len() && is_token_byte(bytes[i]) {
            i += 1;
        }
        let word = &intent[start..i];
        if !is_relationship_shaped(word) {
            continue;
        }
        if vocabulary.iter().any(|v| v == word) {
            continue;
        }
        // A file name: the token is followed by `.<lowercase extension>`.
        if is_file_name(&intent[i..]) {
            continue;
        }
        if PROSE_ACRONYMS.contains(&word) {
            continue;
        }
        if !out.iter().any(|t| t == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// A token byte: what a `\w`-class word is made of. The run is split on
/// anything else, so hyphens and dots end a token.
fn is_token_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `[A-Z][A-Z0-9_]{2,}` — three or more characters, upper-case letters,
/// digits and underscores only, starting with a letter.
fn is_relationship_shaped(word: &str) -> bool {
    let mut chars = word.chars();
    let first = chars.next();
    word.len() >= 3
        && first.is_some_and(|c| c.is_ascii_uppercase())
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Whether the text right after a token reads as a file extension:
/// `.` followed by one or more lower-case ASCII letters, then a
/// non-token byte or the end. `CLAUDE.md` and `Cargo.toml` qualify;
/// `PART_OF.` at a sentence end does not.
fn is_file_name(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    if bytes.first() != Some(&b'.') {
        return false;
    }
    let mut n = 1;
    while n < bytes.len() && bytes[n].is_ascii_lowercase() {
        n += 1;
    }
    n > 1 && (n == bytes.len() || !is_token_byte(bytes[n]))
}

/// Render the findings as the Markdown callout every brief carries next to
/// the intent. Empty when there is nothing to report, so a clean binding's
/// brief is byte-identical to before the rule.
pub fn render_intent_findings(findings: &[IntentFinding], binding_id: &str) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    for f in findings {
        lines.push(format!(
            "> **[binding] `{}`** — the intent names `{}`, which is not a relationship of \
             the destination schema `{}`. Read it as prose, never as an edge to write. \
             Vocabulary: {}.",
            f.code,
            f.token,
            f.schema,
            f.vocabulary
                .iter()
                .map(|v| format!("`{v}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.push(format!(
        "> Fix the intent with `memstead projection edit {binding_id} --patch \
         '{{\"intent\": \"...\"}}'`; the edit refuses an intent that still names an \
         unknown token."
    ));
    format!("{}\n\n", lines.join("\n>\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// A phantom `PROVIDED_BY` and a phantom single-word `PROVIDES` are both
    /// findings; a declared name, a file name, a prose acronym, a short
    /// token and a mixed-case word are not.
    #[test]
    fn tokenizer_reads_all_caps_as_relationships_with_prose_exempt() {
        let v = vocab(&["DEPENDS_ON", "USES"]);
        let intent = "Read CLAUDE.md and DATABASE.md; the crate USES what it DEPENDS_ON, \
                      PROVIDED_BY nothing, and PROVIDES an API over HTTP. An ID is short; \
                      Cargo.toml is a file; `#[cfg(test)]` is code.";
        assert_eq!(
            unknown_tokens(intent, &v),
            vec!["PROVIDED_BY".to_string(), "PROVIDES".to_string()]
        );
    }

    /// A token is reported once however often it appears, in order of
    /// first appearance.
    #[test]
    fn tokens_deduplicate_in_first_appearance_order() {
        let v = vocab(&[]);
        assert_eq!(
            unknown_tokens("ZED then ALPHA then ZED again", &v),
            vec!["ZED".to_string(), "ALPHA".to_string()]
        );
    }

    /// A schema that declares a prose acronym as a relationship keeps it a
    /// relationship: the vocabulary wins over the exemption, and a sentence
    /// ending in a token is not a file name.
    #[test]
    fn vocabulary_wins_over_exemption_and_sentence_end_is_not_a_file() {
        let v = vocab(&["API"]);
        assert_eq!(
            unknown_tokens("An API. Then PART_OF. Done", &v),
            vec!["PART_OF".to_string()]
        );
        assert!(is_file_name(".md and more"));
        assert!(is_file_name(".toml"));
        assert!(!is_file_name(". Then"));
        assert!(!is_file_name(".MD"));
    }

    /// The finding against a real schema names the token, the pin, and the
    /// vocabulary; a legal intent yields nothing; an absent intent yields
    /// nothing.
    #[test]
    fn findings_against_the_software_schema() {
        let registry = memstead_schema::SchemaRegistry::builtin();
        let schema = registry
            .resolve_by_name("software")
            .ok()
            .flatten()
            .or_else(|| {
                // Several versions coexist in the builtin registry; take the
                // newest for this check.
                registry
                    .available_versions("software")
                    .into_iter()
                    .max()
                    .and_then(|v| registry.get("software", &v))
            })
            .expect("the builtin software schema loads");
        let pin = format!("software@{}", schema.version);
        let found = intent_findings(Some("Edges are PROVIDED_BY code."), &pin, &schema);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, BINDING_INTENT_UNKNOWN_RELATIONSHIP_CODE);
        assert_eq!(found[0].token, "PROVIDED_BY");
        assert_eq!(found[0].schema, pin);
        assert!(found[0].vocabulary.iter().any(|v| v == "DEPENDS_ON"));
        assert!(
            found[0].vocabulary.iter().all(|v| !v.starts_with('_')),
            "the loader's synthetic `_default` never rides a finding: {:?}",
            found[0].vocabulary
        );
        assert!(
            found
                .iter()
                .all(|f| f.code == "BINDING_INTENT_UNKNOWN_RELATIONSHIP"),
            "the code literal is the constant"
        );
        assert!(intent_findings(Some("A crate DEPENDS_ON another."), &pin, &schema).is_empty());
        assert!(intent_findings(None, &pin, &schema).is_empty());
        let rendered = render_intent_findings(&found, "engine/graph");
        assert!(rendered.contains("BINDING_INTENT_UNKNOWN_RELATIONSHIP"));
        assert!(rendered.contains("`PROVIDED_BY`"));
        assert!(rendered.contains("projection edit engine/graph"));
        assert_eq!(render_intent_findings(&[], "engine/graph"), "");
    }
}
