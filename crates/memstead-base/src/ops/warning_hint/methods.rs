//! `WarningHint`'s own accessors: the stable `code`, the rendered `message`, and the per-variant structured `details`.

use super::*;

impl WarningHint {
    /// Stable UPPER_SNAKE_CASE identifier. Wire-level contract — never rename
    /// an existing value; new variants add new codes. Agents branch on this,
    /// not on [`WarningHint::message`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::InlineWikiLinkAutoStubbed { .. } => "INLINE_WIKI_LINK_AUTO_STUBBED",
            Self::CrossMemTargetMemUncreated { .. } => "CROSS_MEM_TARGET_MEM_UNCREATED",
            Self::MissingRequiredSection { .. } => "MISSING_REQUIRED_SECTION",
            Self::MissingRequiredField { .. } => "MISSING_REQUIRED_FIELD",
            Self::UndeclaredRelationshipOpen { .. } => "UNDECLARED_RELATIONSHIP_OPEN",
            Self::DuplicateRelationship { .. } => "DUPLICATE_RELATIONSHIP",
            Self::NoSuchRelationship { .. } => "NO_SUCH_RELATIONSHIP",
            Self::UnknownIncludeKey { .. } => "UNKNOWN_INCLUDE_KEY",
            Self::LimitClamped { .. } => "LIMIT_CLAMPED",
            Self::TitleNormalizedToSlugNoop { .. } => "TITLE_NORMALIZED_TO_SLUG_NOOP",
            Self::TitleCharsDroppedFromSlug { .. } => "TITLE_CHARS_DROPPED_FROM_SLUG",
            Self::UpdateNoop { .. } => "UPDATE_NOOP",
            Self::StubFilterExcludesAll { .. } => "STUB_FILTER_EXCLUDES_ALL",
            // One code per outcome:
            // a key declared on some OTHER reachable type was applied
            // with strict type-narrowing (the filter took effect — it
            // restricts the result to the declaring type(s)), so it
            // carries a distinct code from a key no schema declares
            // (which is truly ignored). A consumer branches on `code`
            // alone to learn whether its filter took effect, without
            // inspecting `declared_on_other_types`.
            Self::UnknownFilterKey {
                declared_on_other_types,
                ..
            } => {
                if declared_on_other_types.is_empty() {
                    "UNKNOWN_FILTER_KEY"
                } else {
                    "FILTER_TYPE_SCOPED"
                }
            }
            Self::FieldNotFilterable { .. } => "FIELD_NOT_FILTERABLE",
            Self::FilterValueMultiMember { .. } => "FILTER_VALUE_MULTI_MEMBER",
            Self::FilterValueNotInEnum { .. } => "INVALID_ENUM_VALUE",
            Self::NeighbourhoodCapped { .. } => "NEIGHBOURHOOD_CAPPED",
            Self::SearchResultsTruncated { .. } => "SEARCH_RESULTS_TRUNCATED",
            Self::RangeFilterKeyMalformed { .. } => "RANGE_FILTER_KEY_MALFORMED",
            Self::UnknownRangeFilterField {
                declared_on_other_types,
                ..
            } => {
                if declared_on_other_types.is_empty() {
                    "UNKNOWN_RANGE_FILTER_FIELD"
                } else {
                    "RANGE_FILTER_TYPE_SCOPED"
                }
            }
            Self::FieldNotRangeFilterable { .. } => "FIELD_NOT_RANGE_FILTERABLE",
            Self::SearchMemIndexUnavailable { .. } => "SEARCH_MEM_INDEX_UNAVAILABLE",
            Self::TitleTrimmed { .. } => "TITLE_TRIMMED",
            Self::SuspiciousNestedPrefix { .. } => "SUSPICIOUS_NESTED_PREFIX",
            Self::NoteMissing { .. } => "NOTE_MISSING",
            Self::IgnoredReadonlyField { .. } => "IGNORED_READONLY_FIELD",
            Self::OuterRepoNotIgnoringMemRepo { .. } => "OUTER_REPO_NOT_IGNORING_MEM_REPO",
            Self::MissingRequiredOutgoing { .. } => "MISSING_REQUIRED_OUTGOING",
            Self::SignalThresholdCrossed { .. } => "SIGNAL_THRESHOLD_CROSSED",
            Self::ConstraintUnsatisfied { .. } => "CONSTRAINT_UNSATISFIED",
            Self::DuplicateSectionHeading { .. } => "DUPLICATE_SECTION_HEADING",
            Self::MemReloaded { .. } => "MEM_RELOADED",
            Self::MemRosterChanged { .. } => "MEM_ROSTER_CHANGED",
            Self::ConfigWriteIntervened { .. } => "CONFIG_WRITE_INTERVENED",
            Self::ShortIdResolved { .. } => "SHORT_ID_RESOLVED",
            Self::OutOfBandEditsUndetected { .. } => "OUT_OF_BAND_EDITS_UNDETECTED",
            Self::UnanchoredMention { .. } => "UNANCHORED_MENTION",
            Self::SchemaPinMismatch { .. } => "SCHEMA_PIN_MISMATCH",
            Self::MountUnbacked { .. } => "MOUNT_UNBACKED",
            Self::EngineVersionSkew { .. } => "ENGINE_VERSION_SKEW",
            Self::SchemaGenerationsBehind { .. } => "SCHEMA_GENERATIONS_BEHIND",
            Self::SchemaHeadingRoundtripViolation { .. } => "SCHEMA_HEADING_ROUNDTRIP_VIOLATION",
            Self::SectionHeadingDivergence { .. } => "SECTION_HEADING_DIVERGENCE",
            Self::AutoStubCreated { .. } => "AUTO_STUB_CREATED",
            Self::DerivationBaselineRefreshed { .. } => "DERIVATION_BASELINE_REFRESHED",
            Self::SelfLinkIgnored { .. } => "SELF_LINK_IGNORED",
            Self::CrossSchemaLinkUndeclared { .. } => "CROSS_SCHEMA_LINK_UNDECLARED",
            Self::ParsedRelationInvalid { .. } => "PARSED_RELATION_INVALID",
            Self::ResidualStubForReadOnlyReferrers { .. } => "RESIDUAL_STUB_FOR_READONLY_REFERRERS",
            Self::MemFilesNotDeleted { .. } => "MEM_FILES_NOT_DELETED",
            Self::MemReattachedAfterUnregister { .. } => "MEM_REATTACHED_AFTER_UNREGISTER",
            Self::ReadMemsMigratedToMounts { .. } => "READ_MEMS_MIGRATED_TO_MOUNTS",
            Self::FolderMemProvenance { .. } => "FOLDER_MEM_PROVENANCE",
            Self::SchemaAuthoringSourceMissing { .. } => "SCHEMA_AUTHORING_SOURCE_MISSING",
            Self::SchemaAuthoringSourceDiverged { .. } => "SCHEMA_AUTHORING_SOURCE_DIVERGED",
            Self::SchemaUnstampedSourceRot { .. } => "SCHEMA_UNSTAMPED_SOURCE_ROT",
            Self::AmbiguousDescriptionDelimiter { .. } => "AMBIGUOUS_DESCRIPTION_DELIMITER",
            Self::ParseMissingRequiredDescription { .. } => "MISSING_REQUIRED_DESCRIPTION",
            Self::ParseDescriptionNotPermitted { .. } => "DESCRIPTION_NOT_PERMITTED",
        }
    }

    /// Human-readable message — delegates to `Display`. May change across
    /// releases; use [`WarningHint::code`] for branching.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// Mem that "owns" the warning when one can be attributed.
    /// Workspace-/request-scoped variants return `None` — `memstead_health`'s
    /// mem filter keeps those visible regardless of scope, while
    /// mem-attributable variants drop out when the filter doesn't
    /// match. The contract mirrors the data fields the same filter
    /// gates (counts, distributions, detail lists are source-mem
    /// scoped; rosters stay global).
    pub fn source_mem(&self) -> Option<&str> {
        match self {
            Self::SuspiciousNestedPrefix { from, .. } => Some(from.mem()),
            Self::DuplicateSectionHeading { entity_id, .. } => Some(entity_id.mem()),
            Self::SchemaPinMismatch { mem, .. } => Some(mem.as_str()),
            Self::MountUnbacked { mem, .. } => Some(mem.as_str()),
            Self::SchemaHeadingRoundtripViolation { mem, .. } => Some(mem.as_str()),
            Self::SectionHeadingDivergence { entity_id, .. } => Some(entity_id.mem()),
            Self::MemReloaded { mem, .. } => Some(mem.as_str()),
            Self::MemRosterChanged { .. } => None,
            Self::MemFilesNotDeleted { mem, .. } => Some(mem.as_str()),
            Self::MemReattachedAfterUnregister { mem, .. } => Some(mem.as_str()),
            Self::ReadMemsMigratedToMounts { .. } => None,
            Self::EngineVersionSkew { mem, .. } => Some(mem.as_str()),
            Self::SchemaGenerationsBehind { mem, .. } => Some(mem.as_str()),
            Self::FolderMemProvenance { mem } => Some(mem.as_str()),
            Self::MissingRequiredOutgoing { entity_id, .. } => Some(entity_id.mem()),
            Self::SignalThresholdCrossed { entity_id, .. } => Some(entity_id.mem()),
            Self::ConstraintUnsatisfied { entity_id, .. } => Some(entity_id.mem()),
            Self::DuplicateRelationship { from, .. } => Some(from.mem()),
            Self::NoSuchRelationship { from, .. } => Some(from.mem()),
            Self::InlineWikiLinkAutoStubbed { from, .. } => Some(from.mem()),
            Self::SelfLinkIgnored { id } => Some(id.mem()),
            Self::CrossMemTargetMemUncreated { from_mem, .. } => Some(from_mem.as_str()),
            Self::AutoStubCreated { stub_id, .. } => Some(stub_id.mem()),
            Self::DerivationBaselineRefreshed { from, .. } => Some(from.mem()),
            Self::UpdateNoop { id } => Some(id.mem()),
            Self::ParsedRelationInvalid { entity_id, .. } => Some(entity_id.mem()),
            Self::ResidualStubForReadOnlyReferrers { id, .. } => Some(id.mem()),
            Self::AmbiguousDescriptionDelimiter { from, .. } => Some(from.mem()),
            Self::ParseMissingRequiredDescription { from, .. } => Some(from.mem()),
            Self::ParseDescriptionNotPermitted { from, .. } => Some(from.mem()),
            // Search-mem-index unavailability is attributable to the
            // failing mem; the filter-key warnings are request-
            // derived (the agent's filter payload) and fall through
            // to `None` below to stay visible to the caller.
            Self::SearchMemIndexUnavailable { mem, .. } => Some(mem.as_str()),
            Self::OutOfBandEditsUndetected { mem } => Some(mem.as_str()),
            Self::UnanchoredMention { mem, .. } => Some(mem.as_str()),
            Self::ConfigWriteIntervened { mem, .. } => Some(mem.as_str()),
            Self::ShortIdResolved { resolved, .. } => Some(resolved.mem()),
            Self::CrossSchemaLinkUndeclared { from, .. } => Some(from.mem()),
            // Workspace- or request-scoped — no mem to attribute.
            // OuterRepoNotIgnoringMemRepo concerns the embedding repo,
            // not a specific mem; an agent should see it under any
            // filter. UnknownIncludeKey / LimitClamped / NoteMissing /
            // TitleNormalizedToSlugNoop / StubFilterExcludesAll /
            // UndeclaredRelationshipOpen / MissingRequiredSection /
            // MissingRequiredField are request-derived (mutation
            // payload or schema-level), so the mem is the
            // request's mem — `None` here keeps them visible to
            // the caller that triggered them.
            _ => None,
        }
    }

    /// Whether this warning belongs in a report scoped to `mem`: the one
    /// rule the health composer applies to every warning under a mem
    /// filter. A warning attributed to one mem (`source_mem`) concerns
    /// that mem only; a warning naming several mems (the schema-source
    /// trio) concerns each of them; a warning attributed to no mem at all
    /// (a request notice, a workspace-level roster or config condition)
    /// concerns every mem, this one included, and stays.
    pub fn concerns_mem(&self, mem: &str) -> bool {
        if let Some(source) = self.source_mem() {
            return source == mem;
        }
        match self {
            Self::SchemaAuthoringSourceMissing { mems, .. }
            | Self::SchemaAuthoringSourceDiverged { mems, .. }
            | Self::SchemaUnstampedSourceRot { mems, .. } => mems.iter().any(|m| m == mem),
            _ => true,
        }
    }

    /// One representative of every `WarningHint` variant — the single
    /// source of truth consumed by stability tests (`envelope_*`,
    /// `code_values_are_upper_snake_case`) and by the MCP description
    /// drift-guard (`every_warning_code_appears_in_a_description`).
    /// Adding a new variant without extending this list fails those tests;
    /// that's the forcing function.
    pub fn all_samples() -> Vec<WarningHint> {
        vec![
            WarningHint::EngineVersionSkew {
                mem: "m".into(),
                stamped_engine: "0.3.0".into(),
                running_engine: "0.4.0".into(),
                stamped_schema: "default@1.0.0".into(),
                direction: crate::build_info::SkewDirection::StampedOlder,
            },
            WarningHint::SchemaGenerationsBehind {
                mem: "m".into(),
                pinned: "default@1.0.0".into(),
                newest: "1.2.0".into(),
            },
            WarningHint::MissingRequiredSection {
                entity_type: "t".into(),
                key: "k".into(),
                heading: "H".into(),
                write_rules: vec![],
            },
            WarningHint::MissingRequiredField {
                entity_type: "decision".into(),
                key: "decided_on".into(),
                description: "Date the decision was accepted.".into(),
                enum_values: vec![],
            },
            WarningHint::UndeclaredRelationshipOpen {
                rel_type: "X".into(),
                message: "m".into(),
            },
            WarningHint::DuplicateRelationship {
                rel_type: "X".into(),
                from: EntityId("a".into()),
                to: EntityId("b".into()),
            },
            WarningHint::NoSuchRelationship {
                rel_type: "X".into(),
                from: EntityId("a".into()),
                to: EntityId("b".into()),
            },
            WarningHint::UnknownIncludeKey {
                key: "x".into(),
                allowed: vec![],
            },
            WarningHint::LimitClamped {
                requested: 1,
                actual: 1,
            },
            WarningHint::SearchResultsTruncated {
                kept: 12,
                budget: 12_000,
            },
            WarningHint::TitleNormalizedToSlugNoop {
                requested_title: "Hello World!".into(),
                current_slug: "hello-world".into(),
            },
            WarningHint::TitleCharsDroppedFromSlug {
                title: "Acme Inc. & Co".into(),
                dropped_chars: vec!['.', '&'],
                slug: "acme-inc-co".into(),
            },
            WarningHint::UpdateNoop {
                id: EntityId("specs--example".into()),
            },
            WarningHint::StubFilterExcludesAll {
                entity_type: "spec".into(),
            },
            // Non-empty `declared_on_other_types` → code FILTER_TYPE_SCOPED.
            WarningHint::UnknownFilterKey {
                key: "nonexistent_field".into(),
                scoped_type: Some("spec".into()),
                declared_on_other_types: vec!["decision".into()],
            },
            // Empty `declared_on_other_types` → code UNKNOWN_FILTER_KEY.
            WarningHint::UnknownFilterKey {
                key: "stauts".into(),
                scoped_type: None,
                declared_on_other_types: vec![],
            },
            WarningHint::FieldNotFilterable {
                field: "title".into(),
            },
            WarningHint::RangeFilterKeyMalformed {
                key: "weird_key".into(),
            },
            // Empty `declared_on_other_types` → code UNKNOWN_RANGE_FILTER_FIELD.
            WarningHint::UnknownRangeFilterField {
                field: "count".into(),
                key: "min_count".into(),
                scoped_type: None,
                declared_on_other_types: vec![],
            },
            // Non-empty → code RANGE_FILTER_TYPE_SCOPED.
            WarningHint::UnknownRangeFilterField {
                field: "priority".into(),
                key: "min_priority".into(),
                scoped_type: Some("spec".into()),
                declared_on_other_types: vec!["decision".into()],
            },
            WarningHint::FieldNotRangeFilterable {
                field: "tags".into(),
            },
            WarningHint::SearchMemIndexUnavailable {
                mem: "specs".into(),
                reason: "missing_index",
                error: None,
            },
            WarningHint::SuspiciousNestedPrefix {
                from: EntityId("test-mem-plugin--audit-skill".into()),
                resolved_id: EntityId("test-mem-plugin--plugin--memstead-mcp-tool-surface".into()),
                candidate_target: Some(EntityId(
                    "test-mem-plugin--memstead-mcp-tool-surface".into(),
                )),
                section: "constraints".into(),
                prefix_mounted: false,
            },
            WarningHint::MountUnbacked {
                mem: "institute".into(),
                reason: MountUnbackedReason::MissingRef,
                location: "refs/heads/institute".into(),
            },
            WarningHint::InlineWikiLinkAutoStubbed {
                from: EntityId("specs--demo".into()),
                stubs: vec![EntityId("specs--example-target".into())],
            },
            WarningHint::CrossMemTargetMemUncreated {
                from_mem: "specs".into(),
                to_mem: "memos".into(),
                target_id: EntityId("memos--example".into()),
            },
            WarningHint::NoteMissing {
                tool: "memstead_update".into(),
            },
            WarningHint::OuterRepoNotIgnoringMemRepo {
                outer_repo_root: "/repos/demo".into(),
                workspace_root: "/repos/demo/memstead".into(),
            },
            WarningHint::MissingRequiredOutgoing {
                entity_type: "decision".into(),
                entity_id: EntityId("planning--decision-x".into()),
                missing: vec![
                    MissingRequiredOutgoingBlock {
                        relationships: vec!["CHOSEN".into()],
                        cardinality: "at_least_one".into(),
                        severity: memstead_schema::ConstraintSeverity::Warn,
                        when_field: None,
                        when_value: None,
                    },
                    MissingRequiredOutgoingBlock {
                        relationships: vec!["REJECTED".into()],
                        cardinality: "at_least_one".into(),
                        severity: memstead_schema::ConstraintSeverity::Warn,
                        when_field: None,
                        when_value: None,
                    },
                ],
            },
            WarningHint::DuplicateSectionHeading {
                entity_id: EntityId("plugin--hooks-subsystem".into()),
                section_key: "realization".into(),
                heading: "Realization".into(),
                occurrences: 3,
            },
            WarningHint::ConfigWriteIntervened {
                mem: "test-mem-plugin".into(),
                fields: vec!["description".into()],
            },
            WarningHint::OutOfBandEditsUndetected {
                mem: "test-mem-plugin".into(),
            },
            WarningHint::UnanchoredMention {
                mem: "engine".into(),
                binding: "engine/graph".into(),
                entity: EntityId("engine--reload".into()),
                artifact: "../public/crates/memstead-base/src/storage/filesystem.rs".into(),
                section: "constraints".into(),
            },
            WarningHint::MemReloaded {
                mem: "test-mem-plugin".into(),
                old_head: "abc123".into(),
                new_head: "def456".into(),
                entities_loaded: 42,
            },
            WarningHint::MemRosterChanged {
                added: vec!["arrived".into()],
                removed: vec!["departed".into()],
                quarantined: vec![],
                failures: vec![],
            },
            WarningHint::AutoStubCreated {
                stub_id: EntityId("specs--future-target".into()),
                pending: false,
            },
            WarningHint::ParsedRelationInvalid {
                entity_id: EntityId("specs--example-source".into()),
                rel_type: "EXECUTES".into(),
                target: EntityId("specs--example-target".into()),
                reason: "shape".into(),
                origin: "writable".into(),
                recovery: Some(ParsedRelationRecovery::remove_explicit_relation(
                    EntityId("specs--example-source".into()),
                    EntityId("specs--example-target".into()),
                    "EXECUTES".into(),
                )),
            },
            WarningHint::ResidualStubForReadOnlyReferrers {
                id: EntityId("specs--archived-target".into()),
                referrers: vec![EntityId("archive--archived-source".into())],
            },
            WarningHint::MemFilesNotDeleted {
                mem: "plan-example".into(),
                reason: "backend_prune_failed".into(),
                path: None,
                error: Some("ref-edit transaction rejected".into()),
            },
            WarningHint::MemReattachedAfterUnregister {
                mem: "plan-example".into(),
                unregistered_at: "2026-05-17T08:43:29Z".into(),
            },
            WarningHint::FolderMemProvenance {
                mem: "plan-example".into(),
            },
            WarningHint::SchemaAuthoringSourceMissing {
                schema_ref: "authored@0.1.0".into(),
                stamped_path: "/workspace/authored".into(),
                mems: vec!["specs".into()],
            },
            WarningHint::SchemaAuthoringSourceDiverged {
                schema_ref: "authored@0.1.0".into(),
                stamped_path: "/workspace/authored".into(),
                mems: vec!["specs".into()],
                detail: "the parsed authoring package differs from the sealed copy".into(),
            },
            WarningHint::SchemaUnstampedSourceRot {
                schema_ref: "authored@0.1.0".into(),
                mems: vec!["specs".into()],
                detail: "type 'decision': `propagating_relationships` was renamed".into(),
            },
            WarningHint::AmbiguousDescriptionDelimiter {
                from: EntityId("specs--example-source".into()),
                rel_type: "OTHER".into(),
                target: EntityId("specs--example-target".into()),
                trailing: " -- legacy delimiter".into(),
            },
            WarningHint::ParseMissingRequiredDescription {
                from: EntityId("specs--example-source".into()),
                rel_type: "OTHER".into(),
                target: EntityId("specs--example-target".into()),
            },
            WarningHint::ParseDescriptionNotPermitted {
                from: EntityId("specs--example-source".into()),
                rel_type: "IMPLEMENTS".into(),
                target: EntityId("specs--example-target".into()),
            },
        ]
    }

    pub(super) fn details_payload(&self) -> serde_json::Value {
        match self {
            Self::OutOfBandEditsUndetected { mem } => serde_json::json!({ "mem": mem }),
            Self::UnanchoredMention {
                mem,
                binding,
                entity,
                artifact,
                section,
            } => serde_json::json!({
                "mem": mem,
                "binding": binding,
                "entity": entity,
                "artifact": artifact,
                "section": section,
            }),
            Self::ConfigWriteIntervened { mem, fields } => serde_json::json!({
                "mem": mem,
                "fields": fields,
            }),
            Self::ShortIdResolved { given, resolved } => serde_json::json!({
                "given": given,
                "resolved": resolved,
            }),
            Self::MissingRequiredSection {
                entity_type,
                key,
                heading,
                write_rules,
            } => serde_json::json!({
                "entity_type": entity_type,
                "key": key,
                "heading": heading,
                "write_rules": write_rules,
            }),
            Self::MissingRequiredField {
                entity_type,
                key,
                description,
                enum_values,
            } => serde_json::json!({
                "entity_type": entity_type,
                "key": key,
                "field_description": description,
                "enum_values": enum_values,
            }),
            Self::UndeclaredRelationshipOpen { rel_type, .. } => {
                serde_json::json!({ "rel_type": rel_type })
            }
            Self::DuplicateRelationship { rel_type, from, to } => {
                serde_json::json!({ "rel_type": rel_type, "from": from, "to": to })
            }
            Self::NoSuchRelationship { rel_type, from, to } => {
                serde_json::json!({ "rel_type": rel_type, "from": from, "to": to })
            }
            Self::UnknownIncludeKey { key, allowed } => {
                serde_json::json!({ "key": key, "allowed": allowed })
            }
            Self::LimitClamped { requested, actual } => {
                serde_json::json!({ "requested": requested, "actual": actual })
            }
            Self::TitleNormalizedToSlugNoop {
                requested_title,
                current_slug,
            } => serde_json::json!({
                "requested_title": requested_title,
                "current_slug": current_slug,
            }),
            Self::TitleCharsDroppedFromSlug {
                title,
                dropped_chars,
                slug,
            } => serde_json::json!({
                "title": title,
                "dropped_chars": dropped_chars,
                "slug": slug,
            }),
            Self::UpdateNoop { id } => serde_json::json!({ "id": id }),
            Self::StubFilterExcludesAll { entity_type } => {
                serde_json::json!({ "entity_type": entity_type })
            }
            Self::UnknownFilterKey {
                key,
                scoped_type,
                declared_on_other_types,
            } => serde_json::json!({
                "key": key,
                "scoped_type": scoped_type,
                "declared_on_other_types": declared_on_other_types,
            }),
            Self::FieldNotFilterable { field } => serde_json::json!({ "field": field }),
            Self::FilterValueMultiMember { key, value } => {
                serde_json::json!({ "key": key, "value": value })
            }
            Self::FilterValueNotInEnum {
                key,
                value,
                allowed,
            } => {
                serde_json::json!({ "key": key, "value": value, "allowed": allowed })
            }
            Self::NeighbourhoodCapped { kept, total } => {
                serde_json::json!({ "kept": kept, "total": total })
            }
            Self::SearchResultsTruncated { kept, budget } => {
                serde_json::json!({ "kept": kept, "budget": budget })
            }
            Self::RangeFilterKeyMalformed { key } => serde_json::json!({ "key": key }),
            Self::UnknownRangeFilterField {
                field,
                key,
                scoped_type,
                declared_on_other_types,
            } => serde_json::json!({
                "field": field,
                "key": key,
                "scoped_type": scoped_type,
                "declared_on_other_types": declared_on_other_types,
            }),
            Self::FieldNotRangeFilterable { field } => serde_json::json!({ "field": field }),
            Self::SearchMemIndexUnavailable { mem, reason, error } => serde_json::json!({
                "mem": mem,
                "reason": reason,
                "error": error,
            }),
            Self::TitleTrimmed { original, trimmed } => serde_json::json!({
                "original": original,
                "trimmed": trimmed,
            }),
            Self::SuspiciousNestedPrefix {
                from,
                resolved_id,
                candidate_target,
                section,
                prefix_mounted,
            } => serde_json::json!({
                "from": from,
                "resolved_id": resolved_id,
                "candidate_target": candidate_target,
                "section": section,
                "target_mem": resolved_id.mem(),
                "prefix_mounted": prefix_mounted,
            }),
            Self::InlineWikiLinkAutoStubbed { from, stubs } => serde_json::json!({
                "from": from,
                "stubs": stubs,
            }),
            Self::SelfLinkIgnored { id } => serde_json::json!({ "id": id }),
            Self::CrossSchemaLinkUndeclared {
                from,
                target,
                source_schema,
                target_schema,
            } => serde_json::json!({
                "from": from,
                "target": target,
                "source_schema": source_schema,
                "target_schema": target_schema,
            }),
            Self::CrossMemTargetMemUncreated {
                from_mem,
                to_mem,
                target_id,
            } => serde_json::json!({
                "from_mem": from_mem,
                "to_mem": to_mem,
                "target_id": target_id,
            }),
            Self::NoteMissing { tool } => serde_json::json!({ "tool": tool }),
            Self::IgnoredReadonlyField { field, supplied } => {
                serde_json::json!({ "field": field, "supplied": supplied })
            }
            Self::OuterRepoNotIgnoringMemRepo {
                outer_repo_root,
                workspace_root,
            } => serde_json::json!({
                "outer_repo_root": outer_repo_root,
                "workspace_root": workspace_root,
            }),
            Self::MissingRequiredOutgoing {
                entity_type,
                entity_id,
                missing,
            } => serde_json::json!({
                "entity_type": entity_type,
                "entity_id": entity_id,
                "missing": missing,
            }),
            Self::SignalThresholdCrossed {
                entity_id,
                signal,
                value,
                old_level,
                new_level,
            } => serde_json::json!({
                "entity": entity_id,
                "signal": signal,
                "value": value,
                "old_level": old_level,
                "new_level": new_level,
            }),
            Self::ConstraintUnsatisfied {
                entity_type,
                entity_id,
                violations,
            } => serde_json::json!({
                "entity_type": entity_type,
                "entity_id": entity_id,
                "violations": violations,
            }),
            Self::DuplicateSectionHeading {
                entity_id,
                section_key,
                heading,
                occurrences,
            } => serde_json::json!({
                "entity_id": entity_id,
                "section_key": section_key,
                "heading": heading,
                "occurrences": occurrences,
            }),
            Self::MemReloaded {
                mem,
                old_head,
                new_head,
                entities_loaded,
            } => serde_json::json!({
                "mem": mem,
                "old_head": old_head,
                "new_head": new_head,
                "entities_loaded": entities_loaded,
            }),
            Self::MemRosterChanged {
                added,
                removed,
                quarantined,
                failures,
            } => serde_json::json!({
                "added": added,
                "removed": removed,
                "quarantined": quarantined,
                "failures": failures,
            }),
            Self::AutoStubCreated { stub_id, .. } => serde_json::json!({ "stub_id": stub_id }),
            Self::DerivationBaselineRefreshed { from, rel_type, to } => serde_json::json!({
                "from": from,
                "rel_type": rel_type,
                "to": to,
            }),
            Self::ParsedRelationInvalid {
                entity_id,
                rel_type,
                target,
                reason,
                origin,
                recovery,
            } => {
                serde_json::json!({
                    "entity_id": entity_id,
                    "rel_type": rel_type,
                    "target": target,
                    "reason": reason,
                    "origin": origin,
                    "recovery": recovery,
                })
            }
            Self::ResidualStubForReadOnlyReferrers { id, referrers } => serde_json::json!({
                "id": id,
                "referrers": referrers,
            }),
            Self::MemFilesNotDeleted {
                mem,
                reason,
                path,
                error,
            } => serde_json::json!({
                "mem": mem,
                "reason": reason,
                "path": path,
                "error": error,
            }),
            Self::MemReattachedAfterUnregister {
                mem,
                unregistered_at,
            } => serde_json::json!({
                "mem": mem,
                "unregistered_at": unregistered_at,
            }),
            Self::EngineVersionSkew {
                mem,
                stamped_engine,
                running_engine,
                stamped_schema,
                direction,
            } => {
                serde_json::json!({
                    "mem": mem,
                    "stamped_engine": stamped_engine,
                    "running_engine": running_engine,
                    "stamped_schema": stamped_schema,
                    "direction": direction,
                })
            }
            Self::SchemaGenerationsBehind {
                mem,
                pinned,
                newest,
            } => serde_json::json!({
                "mem": mem,
                "pinned": pinned,
                "newest": newest,
            }),
            Self::ReadMemsMigratedToMounts {
                mems,
                from_host_mems,
            } => serde_json::json!({
                "mems": mems,
                "from_host_mems": from_host_mems,
            }),
            Self::FolderMemProvenance { mem } => serde_json::json!({
                "mem": mem,
                "ledger": ".memstead/changes.jsonl",
                "write_id": "synthetic token, not a commit and not a change cursor (no version control)",
                "change_cursor": "an RFC3339 timestamp — the `ts` of the last ledger entry",
                "durability": "content persists only when the surrounding repository commits it",
            }),
            Self::SchemaAuthoringSourceMissing {
                schema_ref,
                stamped_path,
                mems,
            } => serde_json::json!({
                "schema_ref": schema_ref,
                "stamped_path": stamped_path,
                "mems": mems,
            }),
            Self::SchemaAuthoringSourceDiverged {
                schema_ref,
                stamped_path,
                mems,
                detail,
            } => serde_json::json!({
                "schema_ref": schema_ref,
                "stamped_path": stamped_path,
                "mems": mems,
                "detail": detail,
            }),
            Self::SchemaUnstampedSourceRot {
                schema_ref,
                mems,
                detail,
            } => serde_json::json!({
                "schema_ref": schema_ref,
                "mems": mems,
                "detail": detail,
            }),
            Self::AmbiguousDescriptionDelimiter {
                from,
                rel_type,
                target,
                trailing,
            } => serde_json::json!({
                "from": from,
                "rel_type": rel_type,
                "target": target,
                "trailing": trailing,
            }),
            Self::ParseMissingRequiredDescription {
                from,
                rel_type,
                target,
            } => {
                serde_json::json!({ "from": from, "rel_type": rel_type, "target": target })
            }
            Self::ParseDescriptionNotPermitted {
                from,
                rel_type,
                target,
            } => {
                serde_json::json!({ "from": from, "rel_type": rel_type, "target": target })
            }
            Self::SchemaPinMismatch {
                mem,
                config_pin,
                mount_pin,
            } => {
                serde_json::json!({
                    "mem": mem,
                    "config_pin": config_pin,
                    "mount_pin": mount_pin,
                })
            }
            Self::MountUnbacked {
                mem,
                reason,
                location,
            } => serde_json::json!({
                "mem": mem,
                "reason": reason.as_str(),
                "location": location,
            }),
            Self::SchemaHeadingRoundtripViolation {
                mem,
                schema_ref,
                violations,
            } => {
                serde_json::json!({
                    "mem": mem,
                    "schema_ref": schema_ref,
                    "violations": violations,
                })
            }
            Self::SectionHeadingDivergence {
                entity_id,
                section_key,
                writing_heading,
                existing_heading,
            } => {
                serde_json::json!({
                    "entity_id": entity_id,
                    "section_key": section_key,
                    "writing_heading": writing_heading,
                    "existing_heading": existing_heading,
                })
            }
        }
    }
}
