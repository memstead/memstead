//! Seeded adversarial smoke over the frontmatter/markdown parser family —
//! the stable-toolchain smoke tier of the trust-boundary hardening work.
//!
//! Deterministic and bounded: a hand-rolled xorshift64 generator (the house
//! discipline — no fuzz dependency, no nightly) assembles adversarial inputs
//! from a fragment alphabet and from spliced/truncated realistic seed
//! documents. Every case exercises the pure parse entry points and asserts
//! the boundary's own invariants:
//!
//! - **No panic**: no input panics any entry point — for the tolerant
//!   parser, whose only refusal branch is I/O, crash and invariant break
//!   are the only observable failures, so this is the load-bearing check.
//! - **Boundary agreement**: the three frontmatter implementations (the
//!   tolerant [`super::parser::split_frontmatter`], the borrowing
//!   [`super::parser::body_after_frontmatter`] peek, and the strict
//!   [`crate::validator::strict::split_frontmatter_strict`]) agree on where
//!   frontmatter ends and the body begins. Strict's refusal domain maps
//!   exactly onto the tolerant path's whole-input-is-body degradation —
//!   an input strict refuses must never yield frontmatter tolerantly, and
//!   an input strict accepts must split identically everywhere.
//! - **Masking is offset-safe**: both masks preserve byte length and every
//!   newline position, which is the implicit invariant all the
//!   match-on-masked / slice-from-original arithmetic rests on.
//! - **parse→generate is idempotent**: one parse+generate round normalises;
//!   a second round must be a fixpoint (`m1 == m2`). For strictly-valid
//!   documents this is the plan's parse→generate→parse fixpoint; for
//!   arbitrary input it pins that normalisation converges immediately.
//!
//! A failure reproduces from the seed and case index printed in the panic
//! message (the offending input is printed too). Budget: 3 seeds ×
//! `CASES_PER_SEED` cases — single-digit seconds in a debug build; this is
//! the CI smoke, the coverage-guided long tier lives outside the workspace.

use std::panic::{AssertUnwindSafe, catch_unwind};

use memstead_schema::{builtin_names, type_by_name};

use super::generator::generate_markdown;
use super::parser::{
    self, mask_code_blocks, mask_code_blocks_and_spans, split_frontmatter, split_sections,
};
use crate::validator::strict::split_frontmatter_strict;

// Measured 2026-08-22 (debug build, M-series): 3 × 4000 cases ≈ 1s.
// That is the smoke tier's whole budget — deeper exploration belongs to
// the coverage-guided long tier, not to a bigger N here.
const CASES_PER_SEED: usize = 4000;

struct Xorshift(u64);
impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Fragments chosen to hit the parsers' decision points: frontmatter
/// delimiters (all line-ending flavours), heading markers, fences and
/// spans, wiki-link brackets, relationship-row anatomy, every
/// description-delimiter lookalike, YAML coercion edges, quote-strip
/// edges, multi-byte and combining characters, merge-conflict markers,
/// and a BOM.
const FRAGMENTS: &[&str] = &[
    "---",
    "---\n",
    "----\n",
    "\n---",
    "---\r\n",
    "\r\n",
    "\n",
    "\u{feff}",
    "# ",
    "## ",
    "### Heading\n",
    "#",
    "```",
    "```rust\n",
    "``",
    "`",
    "~~~\n",
    "[[",
    "]]",
    "[[]]",
    "[[x|y]]",
    "|",
    "**",
    "- **USES**: [[a]]",
    "- **uses_2**: [[b:c]]",
    " \u{2014} ",
    " \u{2014}",
    " -- ",
    " - ",
    " \u{2013} ",
    " \u{2212} ",
    "key: value",
    "type: spec",
    "type: 42",
    "tags: a, b",
    ": ",
    ":",
    "\"",
    "'",
    "\"\"",
    "''",
    "true",
    "false",
    "-",
    "0.5",
    "-0.0",
    "99999999999999999999999999",
    "-0",
    " # comment",
    "✓",
    "émj🦀",
    "\u{301}",
    "\t",
    "    indented\n",
    "<<<<<<< a\n",
    "=======\n",
    ">>>>>>> b\n",
    "a",
    " ",
    "e\u{301}",
];

/// Realistic documents the generator splices, truncates, and crosses —
/// the corpus half of the harness. Shapes: a full spec entity, a memo
/// entity, a code-block-and-links document, a CRLF document, and a
/// catch-all-bound extra section.
const SEED_DOCS: &[&str] = &[
    "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\ntags: backend, api\n---\n# Test Entity\n\n## Identity\n\nThis is a test entity.\n\n## Purpose\n\nTesting the parser.\n\n## Relationships\n\n- **USES**: [[other-entity]]\n- **PART_OF**: [[parent]] \u{2014} owns the flow\n\n## Specifies\n\nSome specification content with [[inline-link]].\n",
    "---\ntype: memo\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nstatus: active\ntags: decision, architecture\n---\n# Use Sled For Storage\n\n## Claim\n\nSled is the right embedded store.\n\n## Context\n\nWe evaluated sled, rocksdb, and sqlite.\n\n## Substance\n\nSled wins on dependency footprint.\n",
    "---\ntype: spec\n---\n# Code Test\n\n## Identity\n\nTest entity with `inline code [[not-a-link]]` span.\n\n## Specifies\n\n```\n## Not A Section\n- **USES**: [[not-a-link]]\n```\n\nReal content after code block with [[real-link]].\n",
    "---\r\ntype: spec\r\nlevel: M1\r\n---\r\n# Windows Entity\r\n\r\n## Identity\r\n\r\nCRLF body.\r\n",
    "---\ntype: spec\n---\n# Catch All\n\n## Identity\n\nBase.\n\n## My Extra Notes\n\nContent under a non-schema heading.\n",
];

fn char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn gen_case(rng: &mut Xorshift) -> String {
    match rng.pick(4) {
        // Fragment soup: pure adversarial assembly.
        0 => {
            let n = 1 + rng.pick(40);
            (0..n)
                .map(|_| FRAGMENTS[rng.pick(FRAGMENTS.len())])
                .collect()
        }
        // A realistic document with one fragment spliced in.
        1 => {
            let doc = SEED_DOCS[rng.pick(SEED_DOCS.len())];
            let at = char_boundary(doc, rng.pick(doc.len() + 1));
            let frag = FRAGMENTS[rng.pick(FRAGMENTS.len())];
            format!("{}{}{}", &doc[..at], frag, &doc[at..])
        }
        // A truncated realistic document.
        2 => {
            let doc = SEED_DOCS[rng.pick(SEED_DOCS.len())];
            let at = char_boundary(doc, rng.pick(doc.len() + 1));
            doc[..at].to_string()
        }
        // A crossover of two documents.
        _ => {
            let a = SEED_DOCS[rng.pick(SEED_DOCS.len())];
            let b = SEED_DOCS[rng.pick(SEED_DOCS.len())];
            let ai = char_boundary(a, rng.pick(a.len() + 1));
            let bi = char_boundary(b, rng.pick(b.len() + 1));
            format!("{}{}", &a[..ai], &b[bi..])
        }
    }
}

/// Run every pure entry point over one input and assert the invariants.
/// Panics (assertion or crash) are reported by the caller with the seed.
fn exercise(input: &str) {
    let schema = type_by_name(builtin_names::SPEC).unwrap();

    // Peeks never panic, whatever the input.
    let _ = parser::peek_type_from_frontmatter(input);
    let _ = parser::peek_title_and_type(input);

    let baf = parser::body_after_frontmatter(input);
    let (meta, body) = split_frontmatter(input).expect("tolerant split refused a string input");

    // Boundary agreement across the three implementations. Strict's
    // caller contract strips a BOM before the split; the tolerant family
    // must land on the same boundary for the same document.
    let stripped = input.strip_prefix('\u{feff}').unwrap_or(input);
    match split_frontmatter_strict(stripped, "fuzz.md") {
        Ok((smeta, sbody)) => {
            assert_eq!(
                meta, smeta,
                "tolerant and strict disagree on the frontmatter block"
            );
            assert_eq!(body, sbody, "tolerant and strict disagree on the body");
            assert_eq!(baf, sbody, "peek and strict disagree on the body");
        }
        Err(_) => {
            assert!(
                meta.is_empty(),
                "strict refused but the tolerant split extracted frontmatter"
            );
            assert_eq!(
                body, stripped,
                "strict refused but the tolerant split did not degrade to whole-input body"
            );
            assert_eq!(
                baf, stripped,
                "strict refused but the peek did not degrade to whole-input body"
            );
        }
    }

    // Masking preserves byte length and every newline position — the
    // invariant all match-on-masked / slice-from-original arithmetic
    // rests on.
    for masked in [mask_code_blocks(&body), mask_code_blocks_and_spans(&body)] {
        assert_eq!(masked.len(), body.len(), "mask changed the byte length");
        assert!(
            body.bytes()
                .zip(masked.bytes())
                .all(|(a, b)| (a == b'\n') == (b == b'\n')),
            "mask moved a newline"
        );
    }

    let masked = mask_code_blocks(&body);
    let _ = split_sections(&body, &masked);
    let _ = parser::has_merge_conflict_markers(input);
    let _ = parser::extract_inline_links_lenient(&body, "specs");
    let _ = parser::extract_inline_links(&body, "specs");
    let _ = parser::parse_relationships_with_warnings(&body, "specs", None);

    // Full parse, then generate-idempotence: the first round normalises,
    // the second must be a fixpoint.
    let e1 = parser::parse_markdown(input, "fuzz-case.md", &schema, "specs")
        .expect("parse_markdown refused a string input");
    let m1 = generate_markdown(&e1.entity, &schema);
    let e2 = parser::parse_markdown(&m1, "fuzz-case.md", &schema, "specs")
        .expect("reparse of generated markdown refused");
    let m2 = generate_markdown(&e2.entity, &schema);
    assert_eq!(
        m1, m2,
        "parse→generate is not idempotent (m1 != m2):\nm1: {m1:?}"
    );
}

/// Full replay of the committed shared corpus (`fuzz/corpus/frontmatter`,
/// seeds for the coverage-guided long tier) as ordinary tests. Skips
/// loudly when the corpus is absent (a packaged crate has no fuzz tree).
#[test]
fn committed_frontmatter_corpus_replays() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/frontmatter");
    if !dir.is_dir() {
        eprintln!("SKIP: shared fuzz corpus not present at {}", dir.display());
        return;
    }
    let mut replayed = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let Ok(input) = std::fs::read_to_string(&path) else {
            continue; // non-UTF-8 corpus members are for the byte-level fuzzer
        };
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| exercise(&input))) {
            let msg = payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("<non-string panic payload>");
            panic!("corpus file {}: {msg}", path.display());
        }
        replayed += 1;
    }
    assert!(replayed > 0, "corpus dir exists but replayed nothing");
}

#[test]
fn frontmatter_family_survives_adversarial_inputs() {
    for seed in [0x5eed_f001_u64, 0x5eed_f002, 0x5eed_f003] {
        let mut rng = Xorshift(seed);
        for case in 0..CASES_PER_SEED {
            let input = gen_case(&mut rng);
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| exercise(&input))) {
                let msg = payload
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("<non-string panic payload>");
                panic!("seed {seed:#x} case {case}: {msg}\ninput: {input:?}");
            }
        }
    }
}
