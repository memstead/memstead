//! Coverage-guided fuzzing of the frontmatter/markdown parser family
//! through its public entry points. The invariants mirror the seeded
//! smoke tier (`memstead-base/src/entity/adversarial.rs`); the
//! differential three-implementations property lives only there (it
//! needs `pub(crate)` seams). Findings are fixed at the parser and
//! pinned as fixture regression tests in the normal suite — never
//! closed by widening acceptance.

#![no_main]

use libfuzzer_sys::fuzz_target;
use memstead_base::entity::generator::generate_markdown;
use memstead_base::entity::parser;
use memstead_base::markdown::{mask_code_blocks, mask_code_blocks_and_spans};
use memstead_schema::{builtin_names, type_by_name};

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    // Peeks never panic, whatever the input.
    let _ = parser::peek_type_from_frontmatter(input);
    let _ = parser::peek_title_and_type(input);

    let body = parser::body_after_frontmatter(input);

    // Masking preserves byte length and every newline position — the
    // invariant all match-on-masked / slice-from-original arithmetic
    // rests on.
    for masked in [mask_code_blocks(body), mask_code_blocks_and_spans(body)] {
        assert_eq!(masked.len(), body.len(), "mask changed the byte length");
        assert!(
            body.bytes()
                .zip(masked.bytes())
                .all(|(a, b)| (a == b'\n') == (b == b'\n')),
            "mask moved a newline"
        );
    }

    let _ = parser::has_merge_conflict_markers(input);
    let _ = parser::extract_inline_links_lenient(body, "specs");

    // Full tolerant parse, then generate-idempotence: the first round
    // normalises, the second must be a fixpoint.
    let schema = type_by_name(builtin_names::SPEC).unwrap();
    let e1 = parser::parse_markdown(input, "fuzz.md", &schema, "specs")
        .expect("tolerant parse refused a string input");
    let m1 = generate_markdown(&e1.entity, &schema);
    let e2 = parser::parse_markdown(&m1, "fuzz.md", &schema, "specs")
        .expect("reparse of generated markdown refused");
    let m2 = generate_markdown(&e2.entity, &schema);
    assert_eq!(m1, m2, "parse-generate is not idempotent");
});
