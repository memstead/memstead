//! Coverage-guided fuzzing of the search path's Unicode invariance. The
//! property: whatever spelling a string arrives in, indexing it and then
//! querying it as typed, in NFC and in NFD returns the same hit count,
//! and none of the three steps panics. The seeded smoke tier is
//! `search_is_invariant_under_canonical_normalization` in
//! `memstead-base/src/search_index/query.rs`; a finding here is fixed in
//! the tokenizer (`search_index/tokenizer.rs`), never by loosening the
//! assertion, and pinned as a regression case in that test.

#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use memstead_base::ops::Query;
use memstead_base::search_index::{MemIndex, execute_on_mem};
use memstead_base::{Entity, EntityId};
use memstead_schema::Schema;
use unicode_normalization::UnicodeNormalization;

fn entity(text: &str) -> Entity {
    let mut sections = indexmap::IndexMap::new();
    sections.insert("identity".to_string(), text.to_string());
    Entity {
        id: EntityId::new("specs", "alpha"),
        title: text.to_string(),
        entity_type: "spec".into(),
        mem: "specs".into(),
        file_path: "alpha.md".into(),
        metadata: indexmap::IndexMap::new(),
        sections,
        relationships: Vec::new(),
        content_hash: String::new(),
        stub: false,
        stub_kind: None,
        heading_spans: std::collections::HashMap::new(),
        raw_section_headings: Vec::new(),
    }
}

fn hits(idx: &MemIndex, schema: &Arc<Schema>, term: &str) -> usize {
    let q = Query {
        any: vec![term.to_string()],
        ..Default::default()
    };
    execute_on_mem(idx, Some(schema), &q, 100)
        .expect("search over an in-RAM index never errors")
        .len()
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if text.chars().any(char::is_control) {
        // Control characters are refused at the title gate before any
        // text reaches the index; they are not this target's property.
        return;
    }
    let schema = Schema::builtin_default();
    let mut idx =
        MemIndex::build_in_ram("specs".into(), Some(&schema)).expect("in-RAM index builds");
    idx.index_entity(&entity(text))
        .expect("indexing never errors");
    idx.commit().expect("commit never errors");

    let as_typed = hits(&idx, &schema, text);
    let nfc: String = text.nfc().collect();
    let nfd: String = text.nfd().collect();
    assert_eq!(
        as_typed,
        hits(&idx, &schema, &nfc),
        "NFC spelling of {text:?} returns a different hit count"
    );
    assert_eq!(
        as_typed,
        hits(&idx, &schema, &nfd),
        "NFD spelling of {text:?} returns a different hit count"
    );
});
