//! The overview composer was relocated into `memstead-base` (its sole
//! engine-type parameter already is `memstead_base::Engine`, and it imports
//! only `memstead-base` + `memstead-schema` + std) so the lean
//! `memstead-mcp` build — which does not depend on `memstead-engine` — can
//! render the identical overview from one rendering authority.
//!
//! Re-exported here so every existing `memstead_engine::overview::*`
//! consumer (the full MCP server, the full CLI, serve's HTML overview,
//! engine tests) compiles unchanged.
pub use memstead_base::overview::*;
