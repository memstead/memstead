//! Aggregate-signal evaluation — the one computation behind every
//! surface that serves a declared signal (entity reads, the `signals`
//! health axis, and the `SIGNAL_THRESHOLD_CROSSED` mutation warning).
//!
//! A signal is an exact, parameter-free count with declared
//! thresholds: nothing multiplies, averages, or decays, and values
//! are computed at read time in O(degree) per signal — never stored,
//! never metadata, never part of `_hash`. The evidence (contributing
//! entity ids) ships with the number, always.

use crate::entity::EntityId;
use crate::store::Store;
use memstead_schema::{ReachDirection, SignalDef, SignalLevel, TypeDefinition};

/// One evaluated signal on one entity.
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedSignal {
    pub name: String,
    /// The exact edge count (each qualifying edge counts once).
    pub value: u64,
    /// `None` below the first threshold — wire level `none`.
    pub level: Option<SignalLevel>,
    /// The counterpart entity of every counted edge, deduplicated and
    /// sorted for deterministic payloads.
    pub contributors: Vec<EntityId>,
}

impl ComputedSignal {
    pub fn level_wire(&self) -> &'static str {
        match self.level {
            None => "none",
            Some(SignalLevel::Notice) => "notice",
            Some(SignalLevel::Warn) => "warn",
        }
    }
}

impl serde::Serialize for ComputedSignal {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("ComputedSignal", 4)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("value", &self.value)?;
        s.serialize_field("level", self.level_wire())?;
        let contributors: Vec<String> = self.contributors.iter().map(|c| c.to_string()).collect();
        s.serialize_field("contributors", &contributors)?;
        s.end()
    }
}

/// Wire form shared by the structured entity envelope and the health
/// axis: `[{name, value, level, contributors}]`.
pub fn signals_json(signals: &[ComputedSignal]) -> serde_json::Value {
    serde_json::Value::Array(
        signals
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "value": s.value,
                    "level": s.level_wire(),
                    "contributors": s.contributors.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// Evaluate every signal the type declares for one entity, in
/// declaration order. Counts edges of the declared relation set in
/// the declared direction; with a neighbour pair declared, an edge
/// counts only when its counterpart entity holds the declared value
/// (a counterpart lacking the field, holding another value, or being
/// a stub simply does not count).
pub fn compute_signals(store: &Store, td: &TypeDefinition, id: &EntityId) -> Vec<ComputedSignal> {
    td.signals
        .iter()
        .map(|sig| compute_one(store, sig, id))
        .collect()
}

/// Per-entity signal levels captured before a write applies — the
/// baseline the crossing detection diffs against, keyed by the
/// canonical id string (BTreeMap for deterministic warning order).
/// An id with no snapshot entry (an entity being created) reads as
/// all-`none`.
pub type SignalSnapshot = std::collections::BTreeMap<String, Vec<(String, Option<SignalLevel>)>>;

/// Capture the current signal levels of the candidate entities a
/// write may move. Entities that do not exist yet, are stubs, or
/// whose type declares no signals snapshot as an empty list (every
/// later level then diffs against `none`).
pub fn snapshot_levels<'a>(
    store: &Store,
    schemas: &std::collections::HashMap<String, std::sync::Arc<memstead_schema::Schema>>,
    ids: impl IntoIterator<Item = &'a EntityId>,
) -> SignalSnapshot {
    let mut snap = SignalSnapshot::new();
    for id in ids {
        let levels = signal_levels_of(store, schemas, id);
        snap.entry(id.0.clone()).or_insert(levels);
    }
    snap
}

fn signal_levels_of(
    store: &Store,
    schemas: &std::collections::HashMap<String, std::sync::Arc<memstead_schema::Schema>>,
    id: &EntityId,
) -> Vec<(String, Option<SignalLevel>)> {
    let Some(entity) = store.get(id).filter(|e| !e.stub) else {
        return Vec::new();
    };
    let Some(schema) = schemas.get(entity.mem.as_str()) else {
        return Vec::new();
    };
    let Some(td) = schema.types.get(entity.entity_type.as_str()) else {
        return Vec::new();
    };
    if td.signals.is_empty() {
        return Vec::new();
    }
    compute_signals(store, td, id)
        .into_iter()
        .map(|s| (s.name, s.level))
        .collect()
}

/// Diff the snapshot against the post-write state and emit one
/// `SIGNAL_THRESHOLD_CROSSED` warning per signal whose level changed,
/// in either direction. Rides the out-of-band warning channel beside
/// the success payload; a write that crosses nothing emits nothing.
pub fn crossing_warnings(
    store: &Store,
    schemas: &std::collections::HashMap<String, std::sync::Arc<memstead_schema::Schema>>,
    before: &SignalSnapshot,
) -> Vec<crate::ops::WarningHint> {
    let wire = |level: Option<SignalLevel>| -> String {
        match level {
            None => "none".to_string(),
            Some(SignalLevel::Notice) => "notice".to_string(),
            Some(SignalLevel::Warn) => "warn".to_string(),
        }
    };
    let mut out = Vec::new();
    for (id_str, old_levels) in before {
        let id = EntityId(id_str.clone());
        let Some(entity) = store.get(&id).filter(|e| !e.stub) else {
            continue;
        };
        let Some(schema) = schemas.get(entity.mem.as_str()) else {
            continue;
        };
        let Some(td) = schema.types.get(entity.entity_type.as_str()) else {
            continue;
        };
        if td.signals.is_empty() {
            continue;
        }
        for after in compute_signals(store, td, &id) {
            let old = old_levels
                .iter()
                .find(|(name, _)| name == &after.name)
                .map(|(_, level)| *level)
                .unwrap_or(None);
            if old != after.level {
                out.push(crate::ops::WarningHint::SignalThresholdCrossed {
                    entity_id: id.clone(),
                    signal: after.name.clone(),
                    value: after.value,
                    old_level: wire(old),
                    new_level: wire(after.level),
                });
            }
        }
    }
    out
}

fn compute_one(store: &Store, sig: &SignalDef, id: &EntityId) -> ComputedSignal {
    // (counterpart, qualifies) per candidate edge of the set.
    let counterparts: Vec<EntityId> = match sig.direction {
        ReachDirection::Out => store
            .outgoing(id)
            .iter()
            .filter(|e| sig.relationships.iter().any(|n| n == &e.rel_type))
            .map(|e| e.target.clone())
            .collect(),
        ReachDirection::In => store
            .incoming(id)
            .iter()
            .filter(|e| sig.relationships.iter().any(|n| n == &e.rel_type))
            .map(|e| e.from.clone())
            .collect(),
    };
    let qualifying: Vec<EntityId> =
        if let (Some(field), Some(value)) = (&sig.neighbour_field, &sig.neighbour_value) {
            counterparts
                .into_iter()
                .filter(|c| {
                    store.get(c).is_some_and(|e| {
                        !e.stub
                            && e.metadata
                                .get(field.as_str())
                                .is_some_and(|v| v.to_frontmatter_string() == *value)
                    })
                })
                .collect()
        } else {
            counterparts
        };
    let value = qualifying.len() as u64;
    let mut contributors = qualifying;
    contributors.sort_by(|a, b| a.0.cmp(&b.0));
    contributors.dedup();
    ComputedSignal {
        name: sig.name.clone(),
        value,
        level: sig.level_for(value),
        contributors,
    }
}
