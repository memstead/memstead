//! Tantivy tokenizer configuration — language-agnostic, no stemming.
//!
//! Pipeline: `SimpleTokenizer → LowerCaser → AsciiFoldingFilter`. One
//! tokenizer name for every text field. A single per-vault stemmer
//! misbehaves on bilingual content, so agents handle morphology by
//! enumerating variants in `Query.any` instead.

use tantivy::tokenizer::{
    AsciiFoldingFilter, LowerCaser, SimpleTokenizer, TextAnalyzer, TokenizerManager,
};

/// Tokenizer name registered on every per-vault index. Referenced from the
/// TEXT field options so callers don't have to track the string.
pub const MEMSTEAD_TOKENIZER: &str = "memstead_default";

/// Build the `TextAnalyzer` used by every indexed text field — pure split
/// on non-letters, lowercase, diacritic fold. No stemming.
pub fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build()
}

/// Register `MEMSTEAD_TOKENIZER` on an index-specific tokenizer manager. Each
/// `tantivy::Index` owns its own manager, so this runs once per index.
pub fn register(manager: &TokenizerManager) {
    manager.register(MEMSTEAD_TOKENIZER, analyzer());
}
