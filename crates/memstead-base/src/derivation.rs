//! Derivation baselines — "my source changed" computed, never stamped
//! (agent-trust plan 12).
//!
//! When a schema declares a rel-type `derivation: true`, explicitly
//! writing such an edge records the TARGET's content hash at write
//! time as the edge's baseline. The include-gated `stale_derivations`
//! health axis compares each such edge's baseline against the
//! target's CURRENT hash: differ → the source is stale against the
//! target; no baseline recorded → `unbaselined`, distinctly — never
//! fabricated as fresh or stale.
//!
//! Baselines are engine-owned sidecar state at
//! [`DERIVATION_SIDECAR_PATH`] (`.memstead/derivations.json`) — the
//! anchors-sidecar precedent: staged through the backend's normal
//! entity-path write so it rides the SAME commit as the mutation that
//! produced it, filtered from entity listings by the `.memstead/`
//! rule, invisible in the mem's markdown, and excluded from `_hash`
//! by construction. Export/import behaviour follows the anchors
//! sidecar's decisions (the archive path carries `.memstead/` members
//! as-is).
//!
//! Baseline hashes are the engine's per-entity `content_hash` —
//! SHA-256 over the raw markdown truncated to 16 hex characters, the
//! same 64-bit form optimistic locking uses.
//!
//! Only EXPLICITLY written edges record baselines (create
//! `relations`, update `declare_relations`, `memstead_relate`) —
//! alias-synthesized body-link edges and hierarchy edges are
//! load-derived, not written, and never carry one. Rows whose edge
//! has since been removed are inert (the axis walks live edges, so an
//! orphaned row can never surface); relate-remove prunes its row
//! eagerly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Where the sidecar lives inside a mem — sibling of the anchors
/// sidecar, inside the `.memstead/` prefix every backend filters from
/// entity listings.
pub const DERIVATION_SIDECAR_PATH: &str = ".memstead/derivations.json";

/// One recorded baseline: the target's content hash when the edge was
/// (last) asserted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivationBaseline {
    pub rel_type: String,
    pub target: String,
    /// The target's `content_hash` (16-hex truncated SHA-256) at
    /// assert time. Empty when the target was a stub with no body —
    /// the real content landing later then reads as a change, which
    /// is honest: the derivation was asserted against nothing.
    pub target_hash: String,
}

/// The sidecar document: baselines keyed by source entity id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivationSidecar {
    #[serde(default = "default_version")]
    pub version: u32,
    /// source id → recorded baselines for its derivation edges.
    #[serde(default)]
    pub baselines: BTreeMap<String, Vec<DerivationBaseline>>,
}

fn default_version() -> u32 {
    1
}

impl Default for DerivationSidecar {
    /// Fresh sidecars serialize `version: 1` — aligned with the
    /// serde default for files missing the field, so a future
    /// version-gated migration never reads a fresh file as pre-1.
    fn default() -> Self {
        Self {
            version: default_version(),
            baselines: BTreeMap::new(),
        }
    }
}

impl DerivationSidecar {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("derivation sidecar serialises")
    }

    /// Record (or refresh) the baseline for one edge.
    pub fn set(&mut self, source: &str, rel_type: &str, target: &str, target_hash: &str) {
        let list = self.baselines.entry(source.to_string()).or_default();
        if let Some(existing) = list
            .iter_mut()
            .find(|b| b.rel_type == rel_type && b.target == target)
        {
            existing.target_hash = target_hash.to_string();
        } else {
            list.push(DerivationBaseline {
                rel_type: rel_type.to_string(),
                target: target.to_string(),
                target_hash: target_hash.to_string(),
            });
        }
    }

    /// The recorded baseline hash for one edge, if any.
    pub fn get(&self, source: &str, rel_type: &str, target: &str) -> Option<&str> {
        self.baselines.get(source)?.iter().find_map(|b| {
            (b.rel_type == rel_type && b.target == target).then_some(b.target_hash.as_str())
        })
    }

    /// Drop the baseline for one edge (relate-remove's eager prune).
    pub fn remove(&mut self, source: &str, rel_type: &str, target: &str) {
        if let Some(list) = self.baselines.get_mut(source) {
            list.retain(|b| !(b.rel_type == rel_type && b.target == target));
            if list.is_empty() {
                self.baselines.remove(source);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_refresh_remove_round_trip() {
        let mut s = DerivationSidecar::default();
        assert_eq!(s.get("m--a", "DERIVED_FROM", "m--b"), None);
        s.set("m--a", "DERIVED_FROM", "m--b", "hash1");
        assert_eq!(s.get("m--a", "DERIVED_FROM", "m--b"), Some("hash1"));
        // Refresh replaces in place, no duplicate row.
        s.set("m--a", "DERIVED_FROM", "m--b", "hash2");
        assert_eq!(s.get("m--a", "DERIVED_FROM", "m--b"), Some("hash2"));
        assert_eq!(s.baselines["m--a"].len(), 1);
        // Distinct edges coexist.
        s.set("m--a", "SUMMARIZES", "m--b", "hash3");
        assert_eq!(s.baselines["m--a"].len(), 2);
        let bytes = s.to_bytes();
        let back = DerivationSidecar::from_bytes(&bytes).unwrap();
        assert_eq!(back.get("m--a", "SUMMARIZES", "m--b"), Some("hash3"));
        let mut back = back;
        back.remove("m--a", "DERIVED_FROM", "m--b");
        assert_eq!(back.get("m--a", "DERIVED_FROM", "m--b"), None);
        assert_eq!(back.get("m--a", "SUMMARIZES", "m--b"), Some("hash3"));
    }
}
