//! Tool parameter structs for the agent-facing MCP tools.
//!
//! Parameter structs derive `Deserialize` + `JsonSchema` for rmcp tool routing.
//! These structs are shared between the basis `FilesystemMcpServer`
//! (here in `memstead-mcp`) and the pro `McpServer`
//! ([`crate::server::McpServer`]). Pro-only tool parameters
//! (vault-lifecycle family) live in [`crate::lifecycle`].
//!
//! Workspace-policy mutation **is** an MCP surface: the
//! `memstead_workspace_*` family (allow/revoke create, allow/revoke
//! delete, grant/revoke cross-link) edits the workspace allowlists and
//! cross-vault link policy without a process restart. The macOS app
//! additionally edits the same policy in-process via its
//! `WorkspaceService`. External agents that need to discover vaults read
//! `memstead_health.writable_vaults` and `memstead_health.read_vaults`.
//! **Vault lifecycle** (create/delete a whole vault) is a distinct
//! concept and also lives on the MCP surface as `memstead_vault_create`
//! / `memstead_vault_delete` — gated by workspace-level
//! `[vault_management]` allowlists.

pub mod admin;
pub mod graph;
pub mod mutation;
