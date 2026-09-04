//! Tool parameter structs for the agent-facing MCP tools.
//!
//! Parameter structs derive `Deserialize` + `JsonSchema` for rmcp tool routing.
//! The entity-level tool parameters live here; the mem-lifecycle
//! family's live in [`crate::lifecycle`]. Both are served by
//! [`crate::server::McpServer`].
//!
//! Workspace-policy mutation is **not** an MCP surface. Which mems may
//! be created or deleted, and which cross-mem links are granted, is the
//! operator deciding what an agent is allowed to do — putting those
//! switches on the agent's own tool surface would hand the constrained
//! party the keys to its constraints. Policy is edited on operator
//! surfaces — `memstead workspace <action>` and the
//! operator-authenticated web API — and a policy-gated mutation refuses
//! with the exact command to report. External agents that need to
//! discover mems read `memstead_health.writable_mems` and
//! `memstead_health.read_mems`.
//! **Mem lifecycle** (create/delete a whole mem) is a distinct
//! concept and also lives on the MCP surface as `memstead_mem_create`
//! / `memstead_mem_delete` — gated by workspace-level
//! `[mem_management]` allowlists.

pub mod admin;
pub mod graph;
pub mod mutation;
