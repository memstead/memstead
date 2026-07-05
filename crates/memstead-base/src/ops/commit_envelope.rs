//! Per-commit wire envelope and entity-change variants.
//!
//! Engine-owned value types in the `memstead-base::ops` family
//! (sibling to [`Diff`](crate::ops::Diff),
//! [`ChangeEnvelope`](crate::ops::ChangeEnvelope),
//! [`MemChangedEvent`](crate::engine::MemChangedEvent)).
//!
//! Mirrors the browser-sync JSON shapes one-to-one: the commit
//! envelope and the SSE event.
//!
//! Two producers exist today:
//! - Native embedders — walk a git-branch tree-diff to build envelopes
//!   from native repos for thin-client consumers.
//! - WASM clients — receive envelopes over the bridge wire and pass
//!   them to [`crate::Engine::apply_external_commit`] to materialize
//!   the new state in their in-memory store.
//!
//! Field order matches the spec; field names are the canonical wire
//! identifiers — do not rename or reshape without bumping the
//! wire-format version.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One commit's wire envelope. JSON example from the spec:
///
/// ```json
/// {
///   "sha": "c4f2a8...",
///   "parent": "a3f9b1...",
///   "mem": "engine",
///   "timestamp": "2026-05-18T14:23:01Z",
///   "trailers": { "Tool": "memstead_update", "Actor": "agent" },
///   "changes": [
///     { "op": "modified", "path": "engine--mem.md", "content": "..." }
///   ]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitEnvelope {
    /// Full commit SHA.
    pub sha: String,
    /// Parent commit SHA. Empty string for the first commit of a
    /// branch (no parent).
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub parent: String,
    /// Mem name this commit landed on.
    pub mem: String,
    /// Commit timestamp in RFC 3339 / ISO 8601 form (UTC, second
    /// granularity).
    pub timestamp: String,
    /// Commit-message trailers parsed via the engine's standard
    /// trailer convention. Keyed by trailer name (e.g. `Tool`,
    /// `Actor`, `Client`, `Replays`, `Integration-Run`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub trailers: BTreeMap<String, String>,
    /// Per-entity changes this commit introduced.
    pub changes: Vec<EntityChange>,
}

/// One entity-level change carried by a [`CommitEnvelope`]. Tagged
/// via the `op` discriminator so the wire shape matches the spec's
/// `{ "op": "...", ... }` envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum EntityChange {
    /// Entity newly created in this commit.
    Added {
        /// Mem-relative path on the new side (`.md` suffix
        /// included).
        path: String,
        /// Full markdown body on the new side.
        content: String,
    },
    /// Entity body changed in this commit.
    Modified { path: String, content: String },
    /// Entity removed in this commit. No content travels.
    Deleted { path: String },
    /// Entity renamed in this commit. `from` is the pre-rename
    /// path, `to` is the post-rename path; `content` is the body
    /// on the new side.
    Renamed {
        from: String,
        to: String,
        content: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_envelope_json_matches_spec_example() {
        let env = CommitEnvelope {
            sha: "c4f2a8".to_string(),
            parent: "a3f9b1".to_string(),
            mem: "engine".to_string(),
            timestamp: "2026-05-18T14:23:01Z".to_string(),
            trailers: {
                let mut m = BTreeMap::new();
                m.insert("Tool".to_string(), "memstead_update".to_string());
                m.insert("Actor".to_string(), "agent".to_string());
                m
            },
            changes: vec![
                EntityChange::Modified {
                    path: "engine--mem.md".to_string(),
                    content: "body".to_string(),
                },
                EntityChange::Deleted {
                    path: "engine--alt.md".to_string(),
                },
            ],
        };
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["sha"], "c4f2a8");
        assert_eq!(json["parent"], "a3f9b1");
        assert_eq!(json["mem"], "engine");
        assert_eq!(json["timestamp"], "2026-05-18T14:23:01Z");
        assert_eq!(json["trailers"]["Tool"], "memstead_update");
        assert_eq!(json["changes"][0]["op"], "modified");
        assert_eq!(json["changes"][0]["path"], "engine--mem.md");
        assert_eq!(json["changes"][1]["op"], "deleted");
    }

    #[test]
    fn entity_change_renamed_serialises_with_from_to() {
        let c = EntityChange::Renamed {
            from: "engine--x.md".to_string(),
            to: "engine--z.md".to_string(),
            content: "body".to_string(),
        };
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["op"], "renamed");
        assert_eq!(json["from"], "engine--x.md");
        assert_eq!(json["to"], "engine--z.md");
        assert_eq!(json["content"], "body");
    }
}
