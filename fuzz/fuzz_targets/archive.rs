//! Coverage-guided fuzzing of the archive trust boundary: raw foreign
//! bytes through `validate_and_normalize_archive`, which covers the
//! nested parsers (config, strict entity checks, schema loader, id and
//! graph validation, Louvain, the canonical re-pack) transitively.
//! Asserted per accepted input: the canonical fixpoint — an installer
//! that re-packs on every hop must converge immediately. Findings are
//! fixed at the parser, never by widening acceptance.

#![no_main]

use libfuzzer_sys::fuzz_target;
use memstead_base::validator::validate_and_normalize_archive;

fuzz_target!(|data: &[u8]| {
    if let Ok(v) = validate_and_normalize_archive(data) {
        let again = validate_and_normalize_archive(&v.canonical_bytes)
            .expect("canonical bytes of an accepted archive must re-validate");
        assert_eq!(
            again.canonical_bytes, v.canonical_bytes,
            "re-validation over canonical bytes is not a fixpoint"
        );
    }
});
