//! Tool parameter structs for the agent-facing MCP tools.
//!
//! Parameter structs derive `Deserialize` + `JsonSchema` for rmcp tool routing.
//! These structs are shared between the lean `FilesystemMcpServer`
//! (here in `memstead-mcp`) and the full `McpServer`
//! ([`crate::server::McpServer`]). Full-only tool parameters
//! (mem-lifecycle family) live in [`crate::lifecycle`].
//!
//! Workspace-policy mutation **is** an MCP surface: the
//! `memstead_workspace_*` family (allow/revoke create, allow/revoke
//! delete, grant/revoke cross-link) edits the workspace allowlists and
//! cross-mem link policy without a process restart. The macOS app
//! additionally edits the same policy in-process via its
//! `WorkspaceService`. External agents that need to discover mems read
//! `memstead_health.writable_mems` and `memstead_health.read_mems`.
//! **Mem lifecycle** (create/delete a whole mem) is a distinct
//! concept and also lives on the MCP surface as `memstead_mem_create`
//! / `memstead_mem_delete` — gated by workspace-level
//! `[mem_management]` allowlists.

pub mod admin;
pub mod graph;
pub mod mutation;
