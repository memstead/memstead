//! Tantivy tokenizer configuration — language-agnostic, no stemming.
//!
//! Pipeline: `NfcTokenizer (NFC → SimpleTokenizer) → LowerCaser →
//! AsciiFoldingFilter`. One tokenizer name for every text field. A single
//! per-mem stemmer misbehaves on bilingual content, so agents handle
//! morphology by enumerating variants in `Query.any` instead.
//!
//! The NFC step is the one place the search path normalises Unicode, and
//! it sits BEFORE word splitting on purpose: `SimpleTokenizer` splits on
//! every non-alphanumeric char, and a combining mark (U+0308 in the
//! decomposed spelling of `ä`) is not alphanumeric — so decomposed input
//! used to split `Änderung` into `a` + `nderung` while the composed
//! spelling folded to `anderung`, and the two never met. Indexing and
//! query parsing share this analyzer through `tokenizer_for_field`, so
//! both sides see the same canonical form whatever the caller's keyboard,
//! clipboard or filesystem produced. The id grammar
//! (`entity::id::title_to_slug`) applies the same normalisation to slugs;
//! the snippet renderer folds combining marks away for the same reason.
use tantivy::tokenizer::{
    AsciiFoldingFilter, LowerCaser, SimpleTokenStream, SimpleTokenizer, TextAnalyzer, Tokenizer,
    TokenizerManager,
};
use unicode_normalization::UnicodeNormalization;

/// Tokenizer name registered on every per-mem index. Referenced from the
/// TEXT field options so callers don't have to track the string.
pub const MEMSTEAD_TOKENIZER: &str = "memstead_default";

/// `SimpleTokenizer` over the NFC form of its input. Owns the normalised
/// buffer so the returned stream can borrow it for the call's lifetime;
/// the buffer is reused across calls.
#[derive(Clone, Default)]
pub struct NfcTokenizer {
    inner: SimpleTokenizer,
    buf: String,
}

impl Tokenizer for NfcTokenizer {
    type TokenStream<'a> = SimpleTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> SimpleTokenStream<'a> {
        self.buf.clear();
        self.buf.extend(text.nfc());
        self.inner.token_stream(&self.buf)
    }
}

/// Build the `TextAnalyzer` used by every indexed text field — NFC
/// normalise, pure split on non-letters, lowercase, diacritic fold. No
/// stemming.
pub fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(NfcTokenizer::default())
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build()
}

/// Register `MEMSTEAD_TOKENIZER` on an index-specific tokenizer manager. Each
/// `tantivy::Index` owns its own manager, so this runs once per index.
pub fn register(manager: &TokenizerManager) {
    manager.register(MEMSTEAD_TOKENIZER, analyzer());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<String> {
        let mut analyzer = analyzer();
        let mut stream = analyzer.token_stream(text);
        let mut out = Vec::new();
        while let Some(t) = stream.next() {
            out.push(t.text.clone());
        }
        out
    }

    #[test]
    fn composed_and_decomposed_spellings_tokenize_identically() {
        let composed = "Große Änderung"; // ä as one scalar (NFC)
        let decomposed: String = composed.nfd().collect(); // a + U+0308
        assert_ne!(
            composed, decomposed,
            "the two spellings must differ as bytes"
        );
        assert_eq!(tokens(composed), vec!["grosse", "anderung"]);
        assert_eq!(tokens(&decomposed), tokens(composed));
    }

    #[test]
    fn a_combining_mark_no_longer_splits_a_word() {
        let decomposed = "A\u{0308}nderung";
        assert_eq!(tokens(decomposed), vec!["anderung"]);
    }
}
