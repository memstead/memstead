//! Conversions between `memstead_base` types and the FFI-mirror records in
//! `crate::types`.
//!
//! Flattens insertion-ordered maps (`IndexMap<String, _>`) into
//! `sequence<Entry>` on the FFI side, widens `usize` to `u64`, stringifies
//! `EntityId`, and collapses `HashMap<String, usize>` into
//! `Vec<EdgeTypeCount>` for SwiftUI-friendly identifiability.
//!
//! No business logic — only type translation. Anything that needs to decide
//! something (e.g. how to compose `Relations`) belongs in `lib.rs`.

use memstead_base::{
    entity as core_entity, graph as core_graph, mem as core_mem, ops as core_ops,
    store as core_store,
};

use crate::types::{
    AgentNotesReport, ChangeEnvelope, ChangesReport, ClusterInfo, CommitNote, EdgeSource,
    EdgeTypeCount, Entity, HealthFinding, HealthIssue, HealthSummary, ListResult, MemSchemaOutcome,
    MetadataEntry, MetadataValue, MissingField, ParseRecoveryEntry, ParseRecoveryReport, Query,
    RelationDirection, RelationEdge, Relations, Relationship, ReloadResult, SearchHit,
    SearchResult, SearchScope, Section, StaleEntity, Stats,
};

// ---------------------------------------------------------------------------
// Entity + nested.
// ---------------------------------------------------------------------------

pub(crate) fn metadata_value_to_ffi(value: &core_entity::MetadataValue) -> MetadataValue {
    match value {
        core_entity::MetadataValue::Bool(b) => MetadataValue::BoolValue { value: *b },
        core_entity::MetadataValue::Integer(n) => MetadataValue::IntValue { value: *n },
        core_entity::MetadataValue::Float(f) => MetadataValue::FloatValue { value: *f },
        core_entity::MetadataValue::String(s) => MetadataValue::StringValue { value: s.clone() },
    }
}

pub(crate) fn entity_to_ffi(entity: &core_entity::Entity) -> Entity {
    Entity {
        id: entity.id.to_string(),
        title: entity.title.clone(),
        entity_type: entity.entity_type.clone(),
        mem: entity.mem.clone(),
        file_path: entity.file_path.clone(),
        metadata: entity
            .metadata
            .iter()
            .map(|(k, v)| MetadataEntry {
                key: k.clone(),
                value: metadata_value_to_ffi(v),
            })
            .collect(),
        sections: entity
            .sections
            .iter()
            .map(|(k, v)| Section {
                key: k.clone(),
                content: v.clone(),
            })
            .collect(),
        relationships: entity
            .relationships
            .iter()
            .map(|r| Relationship {
                rel_type: r.rel_type.clone(),
                target: r.target.to_string(),
                description: r.description.clone(),
            })
            .collect(),
        content_hash: entity.content_hash.clone(),
        stub: entity.stub,
    }
}

// ---------------------------------------------------------------------------
// Stats.
// ---------------------------------------------------------------------------

pub(crate) fn stats_to_ffi(
    stats: core_ops::Stats,
    store: &core_store::Store,
    mem_router: &core_mem::MemRouterSnapshot,
) -> Stats {
    let mut edge_types: Vec<EdgeTypeCount> = stats
        .edge_types
        .into_iter()
        .map(|(rel_type, count)| EdgeTypeCount {
            rel_type,
            count: count as u64,
        })
        .collect();
    // Stable order for SwiftUI lists — HashMap iteration is
    // nondeterministic, and Identifiable views flicker if rows reshuffle
    // between reloads.
    edge_types.sort_by(|a, b| a.rel_type.cmp(&b.rel_type));

    // `stats.entity_count` is already the non-stub count (see `Engine::stats`).
    // Derive stubs from `store.len()` so the Swift app gets the MCP-parity
    // split (total = entity_count + stub_count) without a second call.
    let real = stats.entity_count as u64;
    let stub_count = (store.len() as u64).saturating_sub(real);

    let mut writable_mems: Vec<String> = mem_router.writable_mems().iter().cloned().collect();
    writable_mems.sort();

    let writable_set: std::collections::HashSet<&String> =
        mem_router.writable_mems().iter().collect();
    let mut read_mems: Vec<String> = mem_router
        .visible_mems()
        .iter()
        .filter(|n| !writable_set.contains(*n))
        .cloned()
        .collect();
    read_mems.sort();

    Stats {
        entity_count: real,
        stub_count,
        edge_count: stats.edge_count as u64,
        edge_types,
        community_count: stats.community_count as u64,
        mem_count: stats.mem_count as u64,
        types_in_use: stats.types_in_use,
        writable_mems,
        read_mems,
    }
}

// ---------------------------------------------------------------------------
// Health.
// ---------------------------------------------------------------------------

/// Map one engine integrity finding into the FFI shape, tagging it with the
/// mem it was collected for (the engine's per-mem collectors don't repeat
/// the mem in the finding itself).
pub(crate) fn integrity_finding_to_ffi(
    finding: memstead_base::ops::integrity::IntegrityFinding,
    mem: &str,
) -> HealthFinding {
    use memstead_base::ops::integrity::IntegrityAxis;
    HealthFinding {
        id: finding.id,
        mem: mem.to_string(),
        axis: match finding.axis {
            IntegrityAxis::Conformance => "conformance".to_string(),
            IntegrityAxis::Consistency => "consistency".to_string(),
        },
        code: finding.code,
        detail_json: finding.detail.to_string(),
    }
}

pub(crate) fn health_summary_to_ffi(summary: core_ops::HealthSummary) -> HealthSummary {
    HealthSummary {
        stale_entities: summary
            .stale_entities
            .into_iter()
            .map(|s| StaleEntity {
                id: s.id.to_string(),
                title: s.title,
                days_since_modified: s.days_since_modified,
            })
            .collect(),
        missing_fields: summary
            .missing_fields
            .into_iter()
            .map(|r| MissingField {
                id: r.id.to_string(),
                title: r.title,
                score: r.score,
                issues: r
                    .issues
                    .into_iter()
                    .map(|i| HealthIssue {
                        field: i.field,
                        message: i.message,
                    })
                    .collect(),
            })
            .collect(),
        orphan_count: summary.orphan_count as u64,
        stub_count: summary.stub_count as u64,
        // Filled by the caller (`Engine::get_health`) from the per-mem
        // integrity collectors / orphan query; the core summary doesn't
        // carry them.
        findings: Vec::new(),
        orphan_ids: Vec::new(),
        collector_warnings: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// List / search.
// ---------------------------------------------------------------------------

fn query_from_ffi(q: Query) -> core_ops::Query {
    core_ops::Query {
        any: q.any_of,
        not: q.not_in,
        phrase: q.phrase,
        field: q.field,
    }
}

pub(crate) fn search_scope_from_ffi(scope: SearchScope) -> core_ops::SearchScope {
    core_ops::SearchScope {
        query: scope.query.map(query_from_ffi),
        mem: scope.mem,
        entity_type: scope.entity_type,
        limit: scope.limit.map(|v| v as usize),
        offset: scope.offset.map(|v| v as usize),
        filters: scope.filters,
        range_filters: scope.range_filters,
        edge_type: scope.edge_type,
        related_to: scope.related_to.map(core_entity::EntityId),
        depth: scope.depth.map(|v| v as usize),
        expand_via: scope.expand_via,
        expand_depth: scope.expand_depth.map(|v| v as usize),
        stub: scope.stub,
        // The in-process FFI consumer (macOS) reads over a channel with no
        // token cap, so the search token-budget guard isn't surfaced on the
        // FFI `SearchScope` — the engine default applies. Surface it here if a
        // host ever needs to tune it (would need a binding regen).
        token_budget: None,
    }
}

fn hit_to_ffi(hit: core_ops::SearchHit) -> SearchHit {
    SearchHit {
        id: hit.id.to_string(),
        title: hit.title,
        mem: hit.mem,
        entity_type: hit.entity_type,
        stub: hit.stub,
        score: hit.score,
        tokens: hit.tokens as u64,
        snippet: hit.snippet,
        sections: hit
            .sections
            .into_iter()
            .map(|(k, v)| Section { key: k, content: v })
            .collect(),
    }
}

pub(crate) fn search_result_to_ffi(result: core_ops::SearchResult) -> SearchResult {
    SearchResult {
        total: result.total as u64,
        returned: result.returned as u64,
        offset: result.offset as u64,
        hits: result.hits.into_iter().map(hit_to_ffi).collect(),
        // The engine's `SearchResult` / `ListResult` ship typed
        // `WarningHint` warnings. The Swift FFI envelope keeps its
        // string-warning shape for now — flatten each warning's
        // rendered prose so the macOS app's existing consumer
        // continues working.
        warnings: result.warnings.into_iter().map(|w| w.to_string()).collect(),
    }
}

pub(crate) fn list_result_to_ffi(result: core_ops::ListResult) -> ListResult {
    ListResult {
        total: result.total as u64,
        returned: result.returned as u64,
        offset: result.offset as u64,
        total_tokens: result.total_tokens as u64,
        hits: result.hits.into_iter().map(hit_to_ffi).collect(),
        // The engine's `SearchResult` / `ListResult` ship typed
        // `WarningHint` warnings. The Swift FFI envelope keeps its
        // string-warning shape for now — flatten each warning's
        // rendered prose so the macOS app's existing consumer
        // continues working.
        warnings: result.warnings.into_iter().map(|w| w.to_string()).collect(),
    }
}

// ---------------------------------------------------------------------------
// Relations. Composes outgoing + incoming into one record, resolving each
// neighbour's title + entity_type via the store so the caller doesn't need a
// second round trip per edge.
// ---------------------------------------------------------------------------

fn edge_source_to_ffi(source: &core_store::EdgeSource) -> EdgeSource {
    match source {
        core_store::EdgeSource::Explicit => EdgeSource::Explicit,
        core_store::EdgeSource::Hierarchy => EdgeSource::Hierarchy,
        core_store::EdgeSource::BodyLink => EdgeSource::BodyLink,
    }
}

pub(crate) fn build_relations(store: &core_store::Store, id: &core_entity::EntityId) -> Relations {
    let outgoing = store
        .outgoing(id)
        .iter()
        .map(|edge| RelationEdge {
            rel_type: edge.rel_type.clone(),
            other_id: edge.target.to_string(),
            other_title: store
                .get(&edge.target)
                .map(|e| e.title.clone())
                .unwrap_or_default(),
            other_entity_type: store
                .get(&edge.target)
                .map(|e| e.entity_type.clone())
                .unwrap_or_default(),
            direction: RelationDirection::Outgoing,
            source: edge_source_to_ffi(&edge.source),
        })
        .collect();

    let incoming = store
        .incoming(id)
        .iter()
        .map(|edge| RelationEdge {
            rel_type: edge.rel_type.clone(),
            other_id: edge.from.to_string(),
            other_title: store
                .get(&edge.from)
                .map(|e| e.title.clone())
                .unwrap_or_default(),
            other_entity_type: store
                .get(&edge.from)
                .map(|e| e.entity_type.clone())
                .unwrap_or_default(),
            direction: RelationDirection::Incoming,
            source: edge_source_to_ffi(&edge.source),
        })
        .collect();

    Relations {
        entity_id: id.to_string(),
        outgoing,
        incoming,
    }
}

// ---------------------------------------------------------------------------
// Communities. LouvainOutput stores clusters as HashMap<id, ClusterInfo>; we
// flatten to a stable-ordered Vec<ClusterInfo> with the id lifted into the
// record for SwiftUI list identifiability.
// ---------------------------------------------------------------------------

pub(crate) fn clusters_to_ffi(output: &core_graph::LouvainOutput) -> Vec<ClusterInfo> {
    let mut ids: Vec<&String> = output.clusters.keys().collect();
    ids.sort();
    ids.into_iter()
        .map(|id| {
            let info = &output.clusters[id];
            ClusterInfo {
                id: id.clone(),
                entities: info.entities.clone(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reload.
// ---------------------------------------------------------------------------

pub(crate) fn reload_result_to_ffi(result: core_ops::ReloadResult) -> ReloadResult {
    ReloadResult {
        added: result.added.into_iter().map(|id| id.to_string()).collect(),
        changed: result
            .changed
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
        removed: result
            .removed
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Per-mem commit-delta + agent-notes feed.
// ---------------------------------------------------------------------------

pub(crate) fn change_envelope_to_ffi(envelope: memstead_base::ChangeEnvelope) -> ChangeEnvelope {
    use memstead_base::ChangeEnvelope::*;
    match envelope {
        Added {
            id,
            title,
            entity_type,
        } => ChangeEnvelope::Added {
            id: id.to_string(),
            title,
            entity_type,
        },
        Updated {
            id,
            title,
            entity_type,
        } => ChangeEnvelope::Updated {
            id: id.to_string(),
            title,
            entity_type,
        },
        Removed {
            id,
            title,
            entity_type,
        } => ChangeEnvelope::Removed {
            id: id.to_string(),
            title,
            entity_type,
        },
        Renamed {
            from_id,
            to_id,
            title,
            entity_type,
        } => ChangeEnvelope::Renamed {
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            title,
            entity_type,
        },
    }
}

pub(crate) fn changes_report_to_ffi(report: memstead_base::ChangesReport) -> ChangesReport {
    // The core report's per-mem delta carries `mem`, `since`,
    // `head`, and `changes`. The FFI surface keeps the agent-notes
    // pass as a distinct method so consumers compose them explicitly.
    ChangesReport {
        mem: report.mem,
        since: report.since,
        head: report.head,
        changes: report
            .changes
            .into_iter()
            .map(change_envelope_to_ffi)
            .collect(),
    }
}

pub(crate) fn commit_note_to_ffi(note: memstead_base::ops::CommitNote) -> CommitNote {
    CommitNote {
        mem: note.mem,
        sha: note.sha,
        subject: note.subject,
        tool_verb: note.tool_verb,
        entity_id: note.entity_id,
        note: note.note,
        actor: note.actor,
        tool: note.tool,
        client: note.client,
        timestamp: note.timestamp,
    }
}

pub(crate) fn agent_notes_report_to_ffi(
    report: memstead_base::ops::AgentNotesReport,
) -> AgentNotesReport {
    AgentNotesReport {
        mem: report.mem,
        since: report.since,
        head: report.head,
        notes: report.notes.into_iter().map(commit_note_to_ffi).collect(),
        memstead_ref: report.memstead_ref,
    }
}

pub(crate) fn parse_recovery_report_to_ffi(
    report: core_ops::ParseRecoveryReport,
) -> ParseRecoveryReport {
    ParseRecoveryReport {
        entries: report
            .entries
            .into_iter()
            .map(|e| ParseRecoveryEntry {
                entity_id: e.entity_id.to_string(),
                rel_type: e.rel_type,
                target: e.target.to_string(),
                outcome: e.outcome,
                reason: e.reason,
            })
            .collect(),
        // The engine reports an empty sha when nothing was rewritten;
        // surface that as `None` (matching the CLI's omit-when-empty).
        commit_sha: Some(report.commit_sha).filter(|s| !s.is_empty()),
    }
}

pub(crate) fn set_schema_outcome_to_ffi(
    outcome: memstead_base::engine::SetSchemaOutcome,
) -> MemSchemaOutcome {
    use memstead_base::engine::SetSchemaResult;
    // Mirror the MCP wire token (serde `rename_all = "snake_case"`) so the
    // app branches on the same `outcome` string an agent would.
    let outcome_str = match outcome.outcome {
        SetSchemaResult::Noop => "noop",
        SetSchemaResult::Switched => "switched",
        SetSchemaResult::MigrationStarted => "migration_started",
        SetSchemaResult::MigrationPending => "migration_pending",
    }
    .to_string();
    MemSchemaOutcome {
        mem: outcome.mem,
        schema_pin: outcome.schema_pin,
        migration_target: outcome.migration_target,
        outcome: outcome_str,
        // Flatten integrity findings to entity ids — enough for the roster
        // to surface "N entities need repair"; the full migration-repair
        // loop is not an app surface in this plan.
        findings: outcome.findings.into_iter().map(|f| f.id).collect(),
    }
}
