//! The binding's intent says only what the destination vocabulary carries.
//!
//! An intent is prose for the agent, and an agent reads an all-caps token
//! in it (`DEPENDS_ON`, `USES`) as a relationship of the destination mem's
//! schema — an edge it may write. A token the schema does not declare is
//! therefore a fact the intent asserts about a vocabulary that does not
//! hold it: this project's own engine binding once named `PROVIDED_BY` against
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
//! What counts as a relationship-shaped token is deliberately narrow: an
//! all-caps word that carries an underscore (`DEPENDS_ON`, `PROVIDED_BY`),
//! or one that IS a relationship name of some schema this engine knows
//! (`USES`, `STORES`) even though the destination schema lacks it. A plain
//! acronym an intent names as a word (README, MCP, JSON, API, CLAUDE) is
//! prose and is never reported: the rule reads the shape of relationship
//! names, not a list of exempt words.

use serde::Serialize;

use memstead_schema::Schema;

/// The finding code — one literal, indexed by the generated error index.
pub const BINDING_INTENT_UNKNOWN_RELATIONSHIP_CODE: &str = "BINDING_INTENT_UNKNOWN_RELATIONSHIP";

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

/// Every relationship name any schema the engine knows declares: the
/// mem-pinned, the workspace-authored and the built-in catalogues. The
/// `known` side of the token rule: a single all-caps word is read as a
/// relationship claim only when it is one of these names.
pub fn known_relationship_names(engine: &crate::Engine) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let all = engine
        .schemas()
        .values()
        .chain(engine.workspace_schemas().iter())
        .chain(engine.builtin_schemas().iter());
    for schema in all {
        for name in relationship_vocabulary(schema) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names.sort();
    names
}

/// Check an intent against a destination schema's vocabulary. A
/// relationship-shaped token (see [`unknown_tokens`]) the schema does not
/// declare is a finding, reported once per token in order of first
/// appearance. `known` is the union of relationship names across every
/// schema the engine knows ([`known_relationship_names`]); `None` or an
/// empty intent yields nothing.
pub fn intent_findings(
    intent: Option<&str>,
    schema_pin: &str,
    schema: &Schema,
    known: &[String],
) -> Vec<IntentFinding> {
    let Some(intent) = intent else {
        return Vec::new();
    };
    let vocabulary = relationship_vocabulary(schema);
    let mut findings: Vec<IntentFinding> = Vec::new();
    for token in unknown_tokens(intent, &vocabulary, known) {
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
    let known = known_relationship_names(engine);
    intent_findings(intent, &pin.as_display(), &schema, &known)
}

/// The relationship-shaped tokens of `intent` that are not in
/// `vocabulary`, deduplicated in order of first appearance. A token is
/// relationship-shaped when it is all-caps (letters, digits, underscores,
/// three or more characters, starting with a letter) AND either carries
/// an underscore or is one of the `known` relationship names. Split out
/// from [`intent_findings`] so the tokenizer is testable without a schema.
pub fn unknown_tokens(intent: &str, vocabulary: &[String], known: &[String]) -> Vec<String> {
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
        if !word.contains('_') && !known.iter().any(|k| k == word) {
            // A single all-caps word that no schema declares is prose
            // (an acronym, a file stem such as README), never a claim.
            continue;
        }
        if vocabulary.iter().any(|v| v == word) {
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

    /// A phantom `PROVIDED_BY` (underscore) and a phantom `PROVIDES` (a
    /// relationship name another schema declares) are findings; a declared
    /// name, plain acronyms, file stems, a short token and a mixed-case
    /// word are not.
    #[test]
    fn tokenizer_reads_relationship_shaped_tokens_only() {
        let v = vocab(&["DEPENDS_ON", "USES"]);
        let known = vocab(&["DEPENDS_ON", "USES", "PROVIDES", "STORES"]);
        let intent = "Read the README, CLAUDE.md, VISION and GLOSSARY; the MCP tools, the \
                      JSON envelopes, the HTTP API and the SQL schema. The crate USES what \
                      it DEPENDS_ON, PROVIDED_BY nothing, and PROVIDES an API over HTTP. \
                      An ID is short; Cargo.toml is a file; `#[cfg(test)]` is code.";
        assert_eq!(
            unknown_tokens(intent, &v, &known),
            vec!["PROVIDED_BY".to_string(), "PROVIDES".to_string()]
        );
        // With no schema declaring PROVIDES anywhere, the single word is prose.
        assert_eq!(
            unknown_tokens(intent, &v, &vocab(&["DEPENDS_ON", "USES"])),
            vec!["PROVIDED_BY".to_string()]
        );
    }

    /// A token is reported once however often it appears, in order of
    /// first appearance.
    #[test]
    fn tokens_deduplicate_in_first_appearance_order() {
        let v = vocab(&[]);
        assert_eq!(
            unknown_tokens("ZED_A then ALPHA_B then ZED_A again", &v, &[]),
            vec!["ZED_A".to_string(), "ALPHA_B".to_string()]
        );
    }

    /// A schema that declares an acronym-shaped name as a relationship keeps
    /// it a relationship: the vocabulary wins, and a sentence ending in a
    /// token still reads the token.
    #[test]
    fn vocabulary_wins_and_sentence_end_reads_the_token() {
        let v = vocab(&["API"]);
        let known = vocab(&["API"]);
        assert_eq!(
            unknown_tokens("An API. Then PART_OF. Done", &v, &known),
            vec!["PART_OF".to_string()]
        );
        // The same acronym against a schema that lacks it, while another
        // schema declares it, is a claim about the wrong vocabulary.
        assert_eq!(
            unknown_tokens("An API.", &vocab(&["USES"]), &known),
            vec!["API".to_string()]
        );
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
        let known = relationship_vocabulary(&schema);
        let found = intent_findings(Some("Edges are PROVIDED_BY code."), &pin, &schema, &known);
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
        assert!(
            intent_findings(Some("A crate DEPENDS_ON another."), &pin, &schema, &known).is_empty()
        );
        assert!(intent_findings(None, &pin, &schema, &known).is_empty());
        assert!(
            intent_findings(
                Some(
                    "Model the README, VISION and GLOSSARY, the MCP tools, the JSON \
                      envelopes, the HTTP routes, the SQL schema and CLAUDE.md"
                ),
                &pin,
                &schema,
                &known
            )
            .is_empty(),
            "acronyms and file stems are prose"
        );
        let rendered = render_intent_findings(&found, "engine/graph");
        assert!(rendered.contains("BINDING_INTENT_UNKNOWN_RELATIONSHIP"));
        assert!(rendered.contains("`PROVIDED_BY`"));
        assert!(rendered.contains("projection edit engine/graph"));
        assert_eq!(render_intent_findings(&[], "engine/graph"), "");
    }
}
