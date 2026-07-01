//! Authoring-provenance payload carried inside a sealed `.mem` archive.
//!
//! Memstead records a one-sentence authoring rationale (the agent's
//! `note`) on the large majority of mutating commits — the project's
//! headline trust signal. That signal lives author-side in git history
//! (git-branch backend) or `.memstead/changes.jsonl` (folder backend) and
//! is thrown away at the registry boundary: the published archive ships
//! current entity state with no record of *why* any entity says what it
//! says. This payload makes the per-entity rationale **portable** so a
//! consumer who installs a third-party vault can judge "why should I
//! believe this?" without the original repository.
//!
//! ## Wire shape (the archive contract)
//!
//! Lives at [`crate::config::ARCHIVE_PROVENANCE_PATH`]
//! (`.memstead/provenance.json`) inside the archive. The payload is the
//! source of truth — it travels with the `.mem` whether installed from the
//! registry or shared out-of-band.
//!
//! ```json
//! {
//!   "format": 1,
//!   "history": "summarised",
//!   "entities": {
//!     "vault:slug": { "rationale": "why this entity exists", "kind": "create", "timestamp": "2026-06-24T11:32:02Z", "actor": "agent" }
//!   }
//! }
//! ```
//!
//! ## Design commitments
//!
//! - **Additive & forward-compatible.** The whole member is optional; an
//!   archive that predates provenance omits it, and an engine that does
//!   not recognise the member tolerates it as an unknown meta member. New
//!   fields are added optionally so an older reader skips them.
//! - **Per-entity, not the commit DAG.** Each entry carries the entity's
//!   *current* authoring rationale (the most recent mutation note) plus
//!   light metadata — not the full commit history. The omission is
//!   explicit: [`History::Summarised`] tells the consumer the full trail
//!   is not shipped, so absence of history is observable, never implied to
//!   be present.
//! - **No fabricated provenance.** An entity authored without any rationale
//!   is simply absent from `entities`; a reader reports it as
//!   provenance-absent rather than substituting a default. There is no
//!   placeholder rationale.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Current `format` integer of the provenance payload. Bumped only on a
/// breaking shape change; additive fields do not bump it.
pub const ARCHIVE_PROVENANCE_FORMAT: u32 = 1;

/// Whether the archive ships the full commit history or only a per-entity
/// summary. Serialised as a lowercase string so the "history not shipped"
/// decision is observable on the wire (the refusal-complement: a consumer
/// can tell history is summarised rather than silently assume it is
/// present). `#[serde(other)]` on [`Self::Unknown`] keeps an older reader
/// forward-compatible with a future disposition it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum History {
    /// Only per-entity current rationale travels; the full commit DAG does
    /// not. The launch default.
    Summarised,
    /// The full commit history travels (reserved; not produced today).
    Full,
    /// A disposition a newer writer used that this reader does not know —
    /// treat as "not the full trail" (i.e. like `Summarised`).
    #[serde(other)]
    Unknown,
}

/// One entity's portable authoring provenance. Every field is optional so
/// the shape grows additively; a record present in `entities` with a
/// `None` rationale is still meaningful (the entity was touched but carried
/// no note), distinct from an entity absent from the map entirely.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityProvenance {
    /// The agent-authored one-sentence rationale — the 97% trust signal.
    /// `None` when the underlying mutation carried no note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// Mutation kind that last set this rationale (`create`, `update`, …),
    /// as the stable kebab-case wire token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// RFC-3339 timestamp of that mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Actor that authored the mutation (`agent`, `human`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

/// The archive-borne provenance payload. Keyed by entity id (the
/// `vault:slug` form the changelog/commit trailers record). An entity not
/// present in `entities` has provenance reported as absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveProvenance {
    pub format: u32,
    pub history: History,
    #[serde(default)]
    pub entities: BTreeMap<String, EntityProvenance>,
}

impl ArchiveProvenance {
    /// A payload carrying the given per-entity records, marked
    /// [`History::Summarised`] (the launch disposition — full history is
    /// not shipped).
    pub fn summarised(entities: BTreeMap<String, EntityProvenance>) -> Self {
        Self {
            format: ARCHIVE_PROVENANCE_FORMAT,
            history: History::Summarised,
            entities,
        }
    }

    /// The provenance for one entity id, or `None` when absent — the
    /// no-fabrication read contract. A returned record may still carry a
    /// `None` rationale (touched-but-unnoted).
    pub fn entity(&self, id: &str) -> Option<&EntityProvenance> {
        self.entities.get(id)
    }

    /// Serialise to the canonical archive bytes (pretty JSON, trailing
    /// newline) for embedding at [`crate::config::ARCHIVE_PROVENANCE_PATH`].
    pub fn to_archive_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s.into_bytes())
    }

    /// Parse from archive bytes. A malformed payload is an error the
    /// caller may downgrade to "provenance absent" rather than fail the
    /// whole install — the member is additive.
    pub fn from_archive_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_archive_bytes() {
        let mut entities = BTreeMap::new();
        entities.insert(
            "specs:alpha".to_string(),
            EntityProvenance {
                rationale: Some("first draft".to_string()),
                kind: Some("create".to_string()),
                timestamp: Some("2026-06-24T11:32:02Z".to_string()),
                actor: Some("agent".to_string()),
            },
        );
        let payload = ArchiveProvenance::summarised(entities);
        let bytes = payload.to_archive_bytes().unwrap();
        let back = ArchiveProvenance::from_archive_bytes(&bytes).unwrap();
        assert_eq!(payload, back);
        assert_eq!(back.history, History::Summarised);
        assert_eq!(
            back.entity("specs:alpha").and_then(|e| e.rationale.as_deref()),
            Some("first draft")
        );
    }

    #[test]
    fn absent_entity_reports_none_not_a_default() {
        let payload = ArchiveProvenance::summarised(BTreeMap::new());
        assert!(payload.entity("specs:missing").is_none());
    }

    #[test]
    fn unknown_history_disposition_is_forward_compatible() {
        // A future writer emits a disposition this reader does not know;
        // it must parse (as Unknown), not fail — the member is additive.
        let json = r#"{"format":1,"history":"per_section","entities":{}}"#;
        let back = ArchiveProvenance::from_archive_bytes(json.as_bytes()).unwrap();
        assert_eq!(back.history, History::Unknown);
    }

    #[test]
    fn touched_but_unnoted_entry_is_distinct_from_absent() {
        let mut entities = BTreeMap::new();
        entities.insert("specs:beta".to_string(), EntityProvenance::default());
        let payload = ArchiveProvenance::summarised(entities);
        // Present in the map (touched) but no rationale — distinct from a
        // missing key. No fabricated value.
        let rec = payload.entity("specs:beta").expect("present");
        assert!(rec.rationale.is_none());
        assert!(payload.entity("specs:gamma").is_none());
    }
}
