//! Private-pattern redaction for portable authoring provenance.
//!
//! `memstead export --format mem` ships each entity's latest mutation
//! rationale in the archive's `.memstead/provenance.json`. Those notes are
//! written inside the private workspace and name what the public tree must
//! never carry: internal `dev/` plan paths, the legacy domain, absolute
//! user paths, and the rest of the classes `scripts/leak-scan.sh` refuses
//! on the public repo. The export redacts every matched span to
//! `[redacted:<class>]` and never strips the record (the decision on
//! published anchors: a stripped note is indistinguishable from one never
//! written, a sentinel keeps the rationale readable while naming nothing).
//!
//! One vocabulary. The classes below are the leak scan's `scan` lines,
//! label and pattern verbatim; the test at the bottom reads the script and
//! holds the two equal, so a class added to one without the other fails
//! naming the class. Entity bodies are not touched here: the leak scan
//! keeps guarding them, and an archive whose bodies carry a private string
//! still refuses at the seal gate.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;

/// One redaction class: the leak scan's label, its extended-regex pattern
/// verbatim, and whether the pattern's first capture group is a boundary
/// prefix (a space, quote or line start) that must survive the redaction.
#[derive(Debug, Clone, Copy)]
pub struct RedactionClass {
    pub name: &'static str,
    pub pattern: &'static str,
    /// The pattern opens with `(^|<boundary chars>)` so the match includes
    /// one character that is not private; that group is kept.
    pub keeps_leading_group: bool,
}

/// The redaction vocabulary, in the leak scan's order.
pub const REDACTION_CLASSES: &[RedactionClass] = &[
    RedactionClass {
        name: "absolute-user-paths",
        pattern: r"/Users/(dasboe|bjornbosenberg)",
        keeps_leading_group: false,
    },
    RedactionClass {
        name: "secrets",
        pattern: r"(-----BEGIN [A-Z]+ PRIVATE KEY|ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|gho_[A-Za-z0-9]{30,}|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|sk-ant-[A-Za-z0-9_-]{24,}|sk-[A-Za-z0-9]{24,})",
        keeps_leading_group: false,
    },
    RedactionClass {
        name: "private-infra",
        pattern: r"(railway\.app|\.up\.railway\.app|railway\.json)",
        keeps_leading_group: false,
    },
    RedactionClass {
        name: "internal-refs",
        pattern: r"(dev/plans|dev/strategy|dev/ci|LAUNCH\.md)",
        keeps_leading_group: false,
    },
    RedactionClass {
        name: "stale-product-name",
        pattern: r"\b[Mm]emgno\b",
        keeps_leading_group: false,
    },
    RedactionClass {
        name: "excluded-private-dirs",
        pattern: r#"(^|[[:space:]"'`(:,])(macos|websites|graph|inspector|local-ai)/"#,
        keeps_leading_group: true,
    },
    RedactionClass {
        name: "legacy-domain",
        pattern: r"(mdgv\.io|dasboe/mdgv|dasboe\.github\.io)",
        keeps_leading_group: false,
    },
];

/// The sentinel a redacted span becomes.
pub fn sentinel(class: &str) -> String {
    format!("[redacted:{class}]")
}

fn compiled() -> &'static [(RedactionClass, Regex)] {
    static COMPILED: OnceLock<Vec<(RedactionClass, Regex)>> = OnceLock::new();
    COMPILED.get_or_init(|| {
        REDACTION_CLASSES
            .iter()
            .map(|c| {
                (
                    *c,
                    Regex::new(c.pattern).unwrap_or_else(|e| {
                        panic!("redaction class {} has an invalid pattern: {e}", c.name)
                    }),
                )
            })
            .collect()
    })
}

/// Redact every private span in `text`, class by class in vocabulary
/// order; returns the redacted text and the per-class count of spans
/// replaced (classes with no match are absent).
pub fn redact(text: &str) -> (String, BTreeMap<&'static str, usize>) {
    let mut out = text.to_string();
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (class, re) in compiled() {
        let mut n = 0usize;
        let replaced = re.replace_all(&out, |caps: &regex::Captures| {
            n += 1;
            if class.keeps_leading_group {
                format!(
                    "{}{}",
                    caps.get(1).map(|m| m.as_str()).unwrap_or(""),
                    sentinel(class.name)
                )
            } else {
                sentinel(class.name)
            }
        });
        if n > 0 {
            out = replaced.into_owned();
            counts.insert(class.name, n);
        }
    }
    (out, counts)
}

/// Per-class redaction counts as the export result carries them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RedactionCount {
    pub class: String,
    pub count: usize,
}

/// Fold per-string counts into the export result's list, in vocabulary
/// order.
pub fn tally(into: &mut BTreeMap<&'static str, usize>, counts: BTreeMap<&'static str, usize>) {
    for (k, v) in counts {
        *into.entry(k).or_insert(0) += v;
    }
}

pub fn counts_to_list(counts: &BTreeMap<&'static str, usize>) -> Vec<RedactionCount> {
    REDACTION_CLASSES
        .iter()
        .filter_map(|c| {
            counts.get(c.name).map(|n| RedactionCount {
                class: c.name.to_string(),
                count: *n,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_each_class_to_its_sentinel_and_keeps_the_rest() {
        // Assembled at runtime: this file is exempt from the leak scan for
        // its patterns, but the scan's file-type-scoped class still reads
        // it, so no literal below may match a class.
        let input = format!(
            "see {}/x.md and {} from {}/w; reads a {}/ path prefix",
            ["dev", "plans"].join("/"),
            ["mdgv", "io"].join("."),
            ["/Users", "bjornbosenberg"].join("/"),
            "graph"
        );
        let (out, counts) = redact(&input);
        assert_eq!(
            out,
            "see [redacted:internal-refs]/x.md and [redacted:legacy-domain] from [redacted:absolute-user-paths]/w; reads a [redacted:excluded-private-dirs] path prefix"
        );
        let list = counts_to_list(&counts);
        assert_eq!(
            list.iter()
                .map(|c| (c.class.as_str(), c.count))
                .collect::<Vec<_>>(),
            vec![
                ("absolute-user-paths", 1),
                ("internal-refs", 1),
                ("excluded-private-dirs", 1),
                ("legacy-domain", 1)
            ]
        );
        let (clean, none) = redact("an ordinary note about engine-graph/ and memstead.io");
        assert_eq!(
            clean,
            "an ordinary note about engine-graph/ and memstead.io"
        );
        assert!(none.is_empty());
    }

    /// One vocabulary: the classes here equal the leak scan's `scan` lines,
    /// label and pattern verbatim. A class added to either side alone
    /// fails naming it.
    #[test]
    fn vocabulary_equals_the_leak_scan_classes() {
        let script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/leak-scan.sh");
        let text = std::fs::read_to_string(&script)
            .unwrap_or_else(|e| panic!("read {}: {e}", script.display()));
        // `scan "label" '<pattern>'` — the pattern is a single-quoted shell
        // word; the one class whose pattern carries a literal quote spells
        // it as '"'"' and is folded back here.
        let re = Regex::new(r#"(?m)^scan\s+"([a-z-]+)"\s+'((?:[^']|'"'"')+)'"#).unwrap();
        let scanned: Vec<(String, String)> = re
            .captures_iter(&text)
            .map(|c| (c[1].to_string(), c[2].replace("'\"'\"'", "'")))
            .collect();
        assert!(
            !scanned.is_empty(),
            "no scan lines parsed from {}",
            script.display()
        );
        let ours: Vec<(String, String)> = REDACTION_CLASSES
            .iter()
            .map(|c| (c.name.to_string(), c.pattern.to_string()))
            .collect();
        for (name, pattern) in &scanned {
            let mine = ours.iter().find(|(n, _)| n == name).unwrap_or_else(|| {
                panic!(
                    "leak-scan class `{name}` is not in the engine's redaction vocabulary (ops/redaction.rs)"
                )
            });
            assert_eq!(
                &mine.1, pattern,
                "class `{name}`: the engine's pattern differs from the leak scan's"
            );
        }
        for (name, _) in &ours {
            assert!(
                scanned.iter().any(|(n, _)| n == name),
                "engine redaction class `{name}` has no leak-scan `scan` line"
            );
        }
        assert_eq!(scanned.len(), ours.len());
    }
}
