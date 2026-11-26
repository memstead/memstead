//! The supported library surface of the engine crates.
//!
//! A program that links the engine as a library builds on the items
//! this module re-exports, and on nothing else in `memstead-mcp`,
//! `memstead-git-branch`, `memstead-base`, `memstead-projection` or
//! `memstead-schema`.
//!
//! # The promise
//!
//! Before 1.0 the library surface changes only in a minor release, with
//! a changelog entry under the heading "Library surface", and never in
//! a patch release. Everything else in the crates is internal and may
//! change in any release.
//!
//! # The list
//!
//! The list is closed: an item joins it only by a changelog entry under
//! that heading.
//!
//! - Booting an engine from a workspace root:
//!   [`engine_from_workspace_root`].
//! - Building the MCP server over an engine it owns: [`McpServer::new`].
//! - Building a further server instance over the same engine for another
//!   caller: [`McpServer::for_caller`] with a [`CallerKey`], made by
//!   [`CallerKey::new`].
//! - Setting a default identity and pinning an identity:
//!   [`McpServer::with_default_identity`] and
//!   [`McpServer::with_pin_identity`].
//! - Setting a default role, the one a call that declares none records:
//!   [`McpServer::with_default_role`] with a [`Role`].
//! - Filtering tools: [`McpServer::new_with_filter`].
//! - Listing the tools: [`McpServer::filtered_tool_list`].
//! - The default token budget: [`DEFAULT_TOKEN_BUDGET`].
//! - Laying a mem-repo: [`init_mem_repo`].
//! - Mounting a mem that exists as a branch: [`mount_mem`].
//! - Unregistering a mem: [`delete_mem`] with `delete_files: false`.
//!
//! The types those signatures name are re-exported beside them and are
//! part of the surface as far as the listed functions take or return
//! them. Of [`McpServer`] and [`Engine`] the promise covers the methods
//! listed above and no others.
//!
//! [`McpServer`] implements `rmcp`'s `ServerHandler`. A program serves
//! it over a transport of its own, with the `rmcp` version this crate
//! depends on; the engine ships no transport beyond the stdio binary.

pub use crate::config::DEFAULT_TOKEN_BUDGET;
pub use crate::server::McpServer;

pub use memstead_base::mem_management::{
    MemDeleteParams, MemDeleteResponse, MemMountParams, MemMountResponse, MountSource, delete_mem,
    mount_mem,
};
pub use memstead_base::vcs::{Actor, ClientId, Role};
pub use memstead_base::{BootError, CallerKey, Engine, FullEngineError, MountCapability};
pub use memstead_git_branch::workspace_store::engine_from_workspace_root;
pub use memstead_git_branch::{MemRepoInit, MemRepoInitError, init_mem_repo};
pub use memstead_schema::SchemaRef;
