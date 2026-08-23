//! Coverage-guided fuzzing of the content-expression parser and
//! matcher. The input splits at the first NUL: the front is the
//! expression source, the tail drives an observed-block sequence, so
//! the fuzzer controls both sides of `match_blocks`. Invariants mirror
//! the seeded smoke tier (`memstead-schema/src/content_expr.rs`); the
//! NFA-size bound lives only there (it needs private fields).

#![no_main]

use libfuzzer_sys::fuzz_target;
use memstead_schema::content_expr::{ContentExpr, ObservedBlock};

fn block_from(byte: u8, lang_seed: u8) -> ObservedBlock {
    match byte % 8 {
        0 => ObservedBlock::Paragraph,
        1 => ObservedBlock::List {
            ordered: lang_seed.is_multiple_of(2),
        },
        2 => ObservedBlock::Table,
        3 => ObservedBlock::Code {
            lang: ["", "rust", "python", "lang=", "é🦀"][(lang_seed % 5) as usize].to_string(),
        },
        4 => ObservedBlock::Blockquote,
        5 => ObservedBlock::Heading {
            depth: lang_seed % 9,
        },
        6 => ObservedBlock::ThematicBreak,
        _ => ObservedBlock::Html,
    }
}

fuzz_target!(|data: &[u8]| {
    let split = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    let Ok(source) = std::str::from_utf8(&data[..split]) else {
        return;
    };
    let tail = &data[split.min(data.len())..];

    let Ok(expr) = ContentExpr::parse(source) else {
        return; // a typed refusal is a correct outcome
    };

    // parse-source-parse stability: the verbatim source re-parses to a
    // structurally identical expression.
    let again =
        ContentExpr::parse(expr.source()).expect("an accepted expression's source must re-parse");
    assert_eq!(again, expr, "parse-source-parse is not stable");

    // Matching: no panic, deterministic, coherent failure payloads.
    let blocks: Vec<ObservedBlock> = tail
        .chunks(2)
        .take(16)
        .map(|c| block_from(c[0], c.get(1).copied().unwrap_or(0)))
        .collect();
    let first = expr.match_blocks(&blocks);
    let second = expr.match_blocks(&blocks);
    assert_eq!(first, second, "matching is not deterministic");
    if let Err(f) = first {
        assert!(f.failed_at <= blocks.len(), "failed_at out of bounds");
        match &f.found {
            Some(found) => {
                assert!(f.failed_at < blocks.len(), "found set at end-of-body");
                assert_eq!(
                    *found,
                    blocks[f.failed_at].display(),
                    "found does not name the offending block"
                );
            }
            None => assert_eq!(
                f.failed_at,
                blocks.len(),
                "end-of-body failure not at the end"
            ),
        }
    }
});
