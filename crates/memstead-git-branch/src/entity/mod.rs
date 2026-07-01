//! Re-export shim over `memstead_base::entity` plus the git-touching
//! `git_tree_source` submodule that stays in this crate.
//!
//! Downstream consumers (memstead-mcp, memstead-cli, memstead-swift) reach the
//! entity surface through `memstead_git_branch::entity::*` exactly as before;
//! the inner types come from `memstead-base`.

pub use memstead_base::entity::*;

pub mod git_tree_source;
