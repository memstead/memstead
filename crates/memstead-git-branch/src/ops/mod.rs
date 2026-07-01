//! Re-export shim over `memstead_base::ops` (request/response types,
//! `WarningHint`, plus the gix-free `health` and `search` submodules)
//! plus the git-touching operation submodules that stay in this crate.

pub use memstead_base::ops::*;

pub mod agent_notes;
#[cfg(feature = "git-object-storage")]
pub mod branch_reset;
pub mod changes;
#[cfg(feature = "git-object-storage")]
pub mod diff;
pub mod export;
#[cfg(feature = "git-object-storage")]
pub mod transport;
