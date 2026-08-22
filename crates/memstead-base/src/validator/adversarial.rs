//! Seeded adversarial smoke over the archive trust boundary: foreign
//! bytes through [`validate_and_normalize_archive`], which covers the
//! nested parsers (config, strict entity checks, schema loader, id and
//! graph validation, Louvain, the canonical re-pack) transitively; one
//! byte-slice exercises them all.
//!
//! Same discipline as the frontmatter harness
//! (`crate::entity::adversarial`): deterministic hand-rolled xorshift64,
//! stable toolchain, no new dependency, failures reproduce from the seed
//! and case index in the panic message. Per case it asserts:
//!
//! - **No panic**: whatever the bytes (zip-level corruption, truncation,
//!   splices, inner-content mutations, hostile extra entries), the
//!   validator returns `Ok` or a typed error, never a crash.
//! - **Canonical fixpoint**: every accepted archive's canonical bytes
//!   re-validate to the same canonical bytes. An installer that
//!   re-packs on every hop must converge immediately.
//!
//! One-shot properties asserted over the seeds themselves:
//!
//! - every seed archive validates (the corpus is never dead weight);
//! - the deliberate forward-compat tolerance (an unrecognised member
//!   under `.memstead/`, size-capped then ignored) never influences the
//!   canonical bytes: with and without the member, canonical output is
//!   byte-identical.

use std::io::{Cursor, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use zip::write::SimpleFileOptions;

use super::validate_and_normalize_archive;

// Measured 2026-08-22 (debug build, M-series): 3 x 1500 cases plus the
// seed properties in well under a second (3 x 150 ran in 0.07s);
// accepted mutants pay a full pipeline (Louvain + re-pack), refused
// ones are cheap. This is the smoke tier's whole budget; deeper
// exploration belongs to the coverage-guided long tier.
const CASES_PER_SEED: usize = 1500;

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

fn build_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let cursor = Cursor::new(&mut buf);
        let mut w = zip::ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            // Tolerant on purpose: the zip WRITER refuses some hostile
            // shapes (e.g. a duplicate filename) before the validator
            // could ever see them; a refused entry is simply dropped so
            // the case still exercises the boundary with the rest.
            if w.start_file(*name, options).is_ok() {
                let _ = w.write_all(content);
            }
        }
        w.finish().unwrap();
    }
    buf
}

const CONFIG: &str = r#"{"format":4,"name":"adv-mem","version":"0.1.0","schema":"default@1.0.0"}"#;

const ALPHA: &str = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-01-15
level: M0
---
# Alpha Entity

## Identity

A meaningful identity line.

## Purpose

Why it exists.

## Specifies

What it covers.

## Constraints

Its limits.

## Rationale

Design notes.
";

const BETA: &str = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-01-15
level: M0
---
# Beta Entity

## Identity

Links to [[alpha]] inline.

## Purpose

Why it exists.

## Specifies

What it covers.

## Relationships

- **USES**: [[alpha]] \u{2014} exercised by the harness

## Constraints

Its limits.

## Rationale

Design notes.
";

/// The seed corpus: every archive here must validate. Shapes: minimal
/// single-entity, two entities with an explicit relationship and an
/// inline link, and a config carrying title + subject.
fn seed_archives() -> Vec<Vec<u8>> {
    let subject_config = r#"{"format":4,"name":"adv-mem","version":"0.1.0","schema":"default@1.0.0","title":"Adversarial Seed","subject":{"scope":"the harness itself","exclusions":[]}}"#;
    vec![
        build_archive(&[
            (".memstead/config.json", CONFIG.as_bytes()),
            ("alpha.md", ALPHA.as_bytes()),
        ]),
        build_archive(&[
            (".memstead/config.json", CONFIG.as_bytes()),
            ("alpha.md", ALPHA.as_bytes()),
            ("beta.md", BETA.as_bytes()),
        ]),
        build_archive(&[
            (".memstead/config.json", subject_config.as_bytes()),
            ("alpha.md", ALPHA.as_bytes()),
        ]),
    ]
}

/// Text fragments spliced into inner files (config JSON and entity
/// markdown) before re-zipping: JSON structure breakers, coercion
/// edges, markdown decision points, a BOM.
const INNER_FRAGMENTS: &[&str] = &[
    "\"",
    "{",
    "}",
    "[",
    "]",
    ":",
    ",",
    "null",
    "-1",
    "0.5",
    "99999999999999999999999999",
    "\u{feff}",
    "```",
    "## ",
    "# ",
    "[[",
    "]]",
    "- **USES**: [[",
    "\u{2014}",
    "type: spec",
    "format",
    "\\",
    "\n",
    "\r\n",
    "é🦀",
];

/// Hostile paths for injected extra entries: traversal shapes, meta-dir
/// members, schema-tree members, duplicates, deep and long paths.
const EVIL_PATHS: &[&str] = &[
    "evil.md",
    ".memstead/extra.bin",
    ".memstead/schema/x.yaml",
    ".memstead/config.json",
    "../escape.md",
    "a/../../b.md",
    "dir/",
    "a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q.md",
    "..\\win.md",
    ".memstead/anchors.json",
];

/// The three inner files a seed is (re)assembled from, mirroring
/// `seed_archives`' first two shapes.
fn inner_entries(rng: &mut Xorshift) -> Vec<(String, Vec<u8>)> {
    let mut entries = vec![
        (
            ".memstead/config.json".to_string(),
            CONFIG.as_bytes().to_vec(),
        ),
        ("alpha.md".to_string(), ALPHA.as_bytes().to_vec()),
    ];
    if rng.pick(2) == 0 {
        entries.push(("beta.md".to_string(), BETA.as_bytes().to_vec()));
    }
    entries
}

fn char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn gen_case(rng: &mut Xorshift, seeds: &[Vec<u8>]) -> Vec<u8> {
    match rng.pick(5) {
        // Zip-level bit flips.
        0 => {
            let mut bytes = seeds[rng.pick(seeds.len())].clone();
            let flips = 1 + rng.pick(8);
            for _ in 0..flips {
                let at = rng.pick(bytes.len());
                let bit = rng.pick(8);
                bytes[at] ^= 1 << bit;
            }
            bytes
        }
        // Truncation.
        1 => {
            let seed = &seeds[rng.pick(seeds.len())];
            seed[..rng.pick(seed.len() + 1)].to_vec()
        }
        // Splice of two archives.
        2 => {
            let a = &seeds[rng.pick(seeds.len())];
            let b = &seeds[rng.pick(seeds.len())];
            let mut out = a[..rng.pick(a.len() + 1)].to_vec();
            out.extend_from_slice(&b[rng.pick(b.len() + 1)..]);
            out
        }
        // Inner-content mutation: splice a fragment into one inner file,
        // re-zip properly so the bytes reach the nested parsers instead
        // of dying at extraction.
        3 => {
            let mut entries = inner_entries(rng);
            let idx = rng.pick(entries.len());
            let text = String::from_utf8_lossy(&entries[idx].1).into_owned();
            let at = char_boundary(&text, rng.pick(text.len() + 1));
            let frag = INNER_FRAGMENTS[rng.pick(INNER_FRAGMENTS.len())];
            entries[idx].1 = format!("{}{}{}", &text[..at], frag, &text[at..]).into_bytes();
            let borrowed: Vec<(&str, &[u8])> = entries
                .iter()
                .map(|(n, c)| (n.as_str(), c.as_slice()))
                .collect();
            build_archive(&borrowed)
        }
        // A hostile extra entry alongside valid content.
        _ => {
            let mut entries = inner_entries(rng);
            let path = EVIL_PATHS[rng.pick(EVIL_PATHS.len())];
            let mut payload = vec![b'x'; 1 + rng.pick(64)];
            if rng.pick(2) == 0 {
                payload = ALPHA.as_bytes().to_vec();
            }
            entries.push((path.to_string(), payload));
            let borrowed: Vec<(&str, &[u8])> = entries
                .iter()
                .map(|(n, c)| (n.as_str(), c.as_slice()))
                .collect();
            build_archive(&borrowed)
        }
    }
}

/// Validate one byte-slice and, when accepted, assert the canonical
/// fixpoint. Panics (crash or assertion) are reported by the caller.
fn exercise(bytes: &[u8]) {
    if let Ok(v) = validate_and_normalize_archive(bytes) {
        let again = validate_and_normalize_archive(&v.canonical_bytes)
            .expect("canonical bytes of an accepted archive must re-validate");
        assert_eq!(
            again.canonical_bytes, v.canonical_bytes,
            "re-validation over canonical bytes is not a fixpoint"
        );
    }
}

#[test]
fn archive_boundary_survives_adversarial_bytes() {
    let seeds = seed_archives();

    // The corpus is alive: every seed validates and is already at its
    // canonical fixpoint check via `exercise`.
    for (i, seed) in seeds.iter().enumerate() {
        assert!(
            validate_and_normalize_archive(seed).is_ok(),
            "seed archive {i} must validate"
        );
        exercise(seed);
    }

    // The deliberate forward-compat tolerance never influences the
    // canonical bytes: an unrecognised `.memstead/` member is ignored,
    // not woven into the output.
    let plain = build_archive(&[
        (".memstead/config.json", CONFIG.as_bytes()),
        ("alpha.md", ALPHA.as_bytes()),
    ]);
    let with_future_meta = build_archive(&[
        (".memstead/config.json", CONFIG.as_bytes()),
        ("alpha.md", ALPHA.as_bytes()),
        (
            ".memstead/future-payload.bin",
            b"opaque forward-compat blob",
        ),
    ]);
    let plain_canonical = validate_and_normalize_archive(&plain)
        .expect("plain seed validates")
        .canonical_bytes;
    let tolerated_canonical = validate_and_normalize_archive(&with_future_meta)
        .expect("tolerated-member seed validates")
        .canonical_bytes;
    assert_eq!(
        plain_canonical, tolerated_canonical,
        "a tolerated forward-compat member must never influence canonical bytes"
    );

    for seed in [0x5eed_a001_u64, 0x5eed_a002, 0x5eed_a003] {
        let mut rng = Xorshift(seed);
        for case in 0..CASES_PER_SEED {
            let bytes = gen_case(&mut rng, &seeds);
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| exercise(&bytes))) {
                let msg = payload
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("<non-string panic payload>");
                panic!(
                    "seed {seed:#x} case {case}: {msg} ({} archive bytes)",
                    bytes.len()
                );
            }
        }
    }
}
