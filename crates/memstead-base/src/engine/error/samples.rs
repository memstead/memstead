//! One value of every error the engine defines, for the tests that hold the
//! surfaces to one rendering of it.
//!
//! WHY it lives in the engine: the list must be complete, and only the
//! crate that defines the errors can make the compiler say so. Each error
//! type, the engine's two and the inner ones it wraps (validation, section
//! format, title, parse, backend, anchor), has a tag enum generated from a
//! list of its variant names, with a `match` over the error that has no
//! wildcard: a variant added to an error without its name here fails the
//! build. [`missing_samples`] then names every tag no sample carries, and
//! the parity test refuses a non-empty answer, so a variant named here
//! without a sample fails the test. Lists carry one element, so an
//! element's shape is compared too.

use super::{BlockedReferrer, EngineError, MissingWikiLink, PartialMem, ReferrerInfo};
use crate::FullEngineError;
use crate::anchor::AnchorValidationError;
use crate::backend::BackendError;
use crate::entity::EntityId;
use crate::entity::id::SlugError;
use crate::entity::parser::ParseError;
use crate::runtime_validator::{
    MissingRequiredField, MissingRequiredSection, RelationshipHint, ValidationError,
};
use crate::section_format::SectionFormatViolation;

/// A tag per variant of `$ty`, the list of all tags, and the tag of a
/// value. The match has no wildcard and each arm names the variant it
/// tags, so the list cannot omit a variant or tag one as another.
macro_rules! variant_tags {
    ($tag:ident, $ty:ident, [$($v:ident),* $(,)?]) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        enum $tag {
            $($v),*
        }
        impl $tag {
            const ALL: &'static [$tag] = &[$($tag::$v),*];
            fn of(e: &$ty) -> $tag {
                match e {
                    $($ty::$v { .. } => $tag::$v),*
                }
            }
        }
    };
}

variant_tags!(
    EngineTag,
    EngineError,
    [
        DuplicateMem,
        UnknownMem,
        MemUnmounted,
        MemQuarantined,
        ReadOnlyMount,
        CheckNotRecorded,
        UnknownType,
        InvalidTitle,
        AlreadyExists,
        ConstraintUnsatisfied,
        SectionFormatRefused,
        RequiredOutgoingUnsatisfied,
        RetypeRefused,
        RetypeNoOp,
        InvalidCheckFinding,
        RetypeReferrerUnprobeable,
        NotFound,
        ExpectedHashRequired,
        HashMismatch,
        HasIncomingRefs,
        MemHasIncomingRefs,
        CrossMemLinkNotAllowed,
        CrossMemTargetNotFound,
        EntityIdMissingMem,
        CrossMemEdgeNotDeclared,
        RepairNotNeeded,
        RenameNoOp,
        EmptyUpdate,
        RenameBlockedByCrossMemPolicy,
        WikiLinkWithoutRelation,
        RelationHasBodyLinks,
        RenamePartialFailure,
        CommitNotApplied,
        StaleView,
        StubCannotRelate,
        StubNotUpdatable,
        StubNotRenamable,
        InvalidEntityId,
        EntityIdTooLong,
        InvalidWikiLinkTarget,
        InvalidWikiLinkMem,
        ConflictingSectionModes,
        RelationshipCycle,
        SetAndUnsetConflict,
        RequiredFieldUnset,
        MissingRequiredSection,
        PatchSectionEmpty,
        PatchOldNotFound,
        UnterminatedFenceInStoredBody,
        Validation,
        ParseAfterWrite,
        Parse,
        Backend,
        SchemaNotFound,
        EmbeddedSchemaInvalid,
        SchemaPackageInvalid,
        SchemaResolverInit,
        Mem,
        MemNameCollision,
        ProcessMemNotEligible,
        InvalidInput,
        AnchorsSidecarUnreadable,
        AmbiguousQuery,
        ContextNotComputable,
        MemNotReadOnly,
        NotGitBacked,
        TemplateIdentityMismatch,
        ChunkOutOfRange,
        InvalidSchemaPin,
        UnsupportedWorkspaceShape,
        NoWritableMem,
        AmbiguousMem,
        UnknownTypes,
        UnknownSchema,
        InvalidTargetSchema,
        MergeConflictUnsupportedBackend,
        NotConflicted,
        ConflictNotTimestampOnly,
        UnknownRemote,
        RemoteUnreachable,
        LocalDivergence,
        NonFastForward,
        LocalInvalidState,
        SchemaViolationInFetch,
        PushedCommitsProtected,
        BranchResetHeadMoved,
        UnknownRef,
        ProposalDispositionsIncomplete,
        ProposalConflict,
        ProposalStale,
        ProposalUnattributed,
        ProposalLinksRejected,
        RenameSimilarityOutOfRange,
        InvalidChangesCursor,
        InvalidTimestampCursor,
        ReviewMarkNotSet,
        MemConfigIncomplete,
        MissingRequiredDescription,
        DescriptionNotPermitted,
        RelationManualAuthoringForbidden,
        SearchUnavailable,
        MarkdownExportUnsupportedBackend,
        InvalidAnchor,
        InEntity,
    ]
);

variant_tags!(
    FullTag,
    FullEngineError,
    [
        Engine,
        MemPathNotAllowed,
        InvalidMemName,
        MemReferencedByPolicy,
        MemSchemaNotAllowed,
        ConfigAlreadyExists,
        MemStorageResidueDetected,
        MemNameRefConflict,
    ]
);

variant_tags!(
    ValidationTag,
    ValidationError,
    [
        UnknownSection,
        UnknownMetadata,
        InvalidEnumValue,
        ReadOnlyField,
        SectionNotUpdatable,
        InvalidRelationshipType,
        InvalidRelationshipShape,
        SectionContentInvalid,
        EmptyUndeclaredHeading,
        UnterminatedFence,
        SectionContentControlByte,
        InvalidFieldValue,
    ]
);

variant_tags!(
    SectionFormatTag,
    SectionFormatViolation,
    [
        ContentMismatch,
        ItemPatternMismatch,
        TableColumns,
        SetextReserved,
    ]
);

variant_tags!(
    SlugTag,
    SlugError,
    [IdTooLong, TitleEmpty, TitleHasControlChars,]
);

variant_tags!(
    ParseTag,
    ParseError,
    [MissingFrontmatter, InvalidFrontmatter, MissingTitle, Io,]
);

variant_tags!(
    BackendTag,
    BackendError,
    [
        Sealed,
        Io,
        HashMismatch,
        Path,
        Other,
        ParentMismatch,
        CommitNotApplied,
        FollowUpFailed,
    ]
);

variant_tags!(
    AnchorTag,
    AnchorValidationError,
    [
        UnknownClass,
        UnknownGrain,
        UnknownHashStability,
        MissingArtifact,
        HashOnNonHashClass,
        ContentAndHash,
        ContentNotAcceptedForGrain,
        UnitAbsentFromContent,
        SpanLocatorUnusable,
        SpanOutsideContent,
        DuplicateAnchorTriple,
        SpanAndHash,
        SpanEmpty,
        SpanNotAcceptedForGrain,
        SpanAbsentFromContent,
        EmptySource,
        SourceNotDeclared,
        ArtifactUnresolvable,
        GrainNamespaceUnsupported,
        PathGrainOnUrlArtifact,
    ]
);

fn s(v: &str) -> String {
    v.to_string()
}

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|i| i.to_string()).collect()
}

fn hint() -> RelationshipHint {
    RelationshipHint {
        name: s("USES"),
        when_to_use: Some(s("one thing uses another")),
    }
}

fn missing_field() -> MissingRequiredField {
    MissingRequiredField {
        entity_type: s("spec"),
        key: s("level"),
        description: s("maturity level"),
        enum_values: v(&["M0", "M1"]),
    }
}

fn referrer() -> ReferrerInfo {
    ReferrerInfo {
        from_id: s("specs--beta"),
        rel_types: v(&["USES"]),
        mem: s("specs"),
    }
}

fn validation_samples() -> Vec<ValidationError> {
    vec![
        ValidationError::UnknownSection {
            key: s("purpse"),
            entity_type: s("spec"),
            declared: v(&["identity", "purpose"]),
            suggestion: Some(s("purpose")),
        },
        ValidationError::UnknownMetadata {
            key: s("levl"),
            entity_type: s("spec"),
            declared: v(&["level"]),
            suggestion: Some(s("level")),
        },
        ValidationError::InvalidEnumValue {
            field: s("level"),
            value: s("M9"),
            allowed: v(&["M0", "M1"]),
            field_description: Some(s("maturity level")),
            suggestion: Some(s("M0")),
            type_write_rules: v(&["set a level"]),
            entity_type: s("spec"),
        },
        ValidationError::ReadOnlyField {
            field: s("created_date"),
        },
        ValidationError::SectionNotUpdatable {
            section: s("relationships"),
            entity_type: s("spec"),
        },
        ValidationError::InvalidRelationshipType {
            input: s("USE"),
            allowed: vec![hint()],
            suggestion: Some(s("USES")),
        },
        ValidationError::InvalidRelationshipShape {
            rel_type: s("USES"),
            from_type: s("spec"),
            to_type: s("memo"),
            allowed_source_types: v(&["spec"]),
            allowed_target_types: v(&["spec"]),
            suggestion: Some(hint()),
        },
        ValidationError::SectionContentInvalid {
            section: s("purpose"),
            embedded_heading: s("## Inner"),
        },
        ValidationError::EmptyUndeclaredHeading {
            section: s("purpose"),
            heading: s("Stray"),
            entity_type: s("spec"),
        },
        ValidationError::UnterminatedFence {
            section: s("purpose"),
            fence: s("```"),
        },
        ValidationError::SectionContentControlByte {
            section: s("purpose"),
            control_char: '\u{7}',
            codepoint: 7,
            byte_offset: 3,
        },
        ValidationError::InvalidFieldValue {
            field: s("decided_on"),
            value: s("yesterday"),
            expected_type: s("date"),
            expected_format: Some(s("YYYY-MM-DD")),
            field_description: Some(s("when it was decided")),
            entity_type: s("decision"),
        },
    ]
}

fn section_format_samples() -> Vec<SectionFormatViolation> {
    vec![
        SectionFormatViolation::ContentMismatch {
            section: s("steps"),
            expected: s("a list"),
            found: v(&["paragraph"]),
            failed_at: 0,
            expected_next: v(&["list"]),
            example: Some(s("- one")),
        },
        SectionFormatViolation::ItemPatternMismatch {
            section: s("steps"),
            item_index: 0,
            line: 1,
            text: s("one"),
            pattern: s("^\\d+\\."),
            groups: v(&["n"]),
            example: Some(s("1. one")),
        },
        SectionFormatViolation::TableColumns {
            section: s("table"),
            reason: s("column count"),
            expected_columns: v(&["a", "b"]),
            found_columns: v(&["a"]),
            row_line: Some(2),
            expected_cells: Some(2),
            found_cells: Some(1),
            column: Some(s("b")),
            pattern: None,
            cell: None,
            example: None,
        },
        SectionFormatViolation::SetextReserved {
            section: s("purpose"),
            line: 2,
            depth: 2,
        },
    ]
}

fn slug_samples() -> Vec<SlugError> {
    vec![
        SlugError::IdTooLong {
            input: s("x"),
            length: 300,
            max: 200,
        },
        SlugError::TitleEmpty { input: s("   ") },
        SlugError::TitleHasControlChars {
            input: s("a\nb"),
            control_chars: vec!['\n'],
            proposed_slug: s("a-b"),
        },
    ]
}

fn parse_samples() -> Vec<ParseError> {
    vec![
        ParseError::MissingFrontmatter,
        ParseError::InvalidFrontmatter(s("bad yaml")),
        ParseError::MissingTitle,
        ParseError::Io(std::io::Error::other("disk")),
    ]
}

fn backend_samples() -> Vec<BackendError> {
    vec![
        BackendError::Sealed,
        BackendError::Io(std::io::Error::other("disk")),
        BackendError::HashMismatch {
            current: s("abc123"),
        },
        BackendError::Path(s("bad path")),
        BackendError::Other(s("other")),
        BackendError::ParentMismatch {
            expected: s("a"),
            actual: s("b"),
        },
        BackendError::CommitNotApplied {
            detail: s("not applied"),
        },
        BackendError::FollowUpFailed {
            commit: s("abc123"),
            step: "provenance",
            detail: s("failed"),
        },
    ]
}

fn anchor_samples() -> Vec<AnchorValidationError> {
    const ALLOWED: &[&str] = &["anchored", "derived"];
    vec![
        AnchorValidationError::UnknownClass {
            got: Some(s("x")),
            allowed: ALLOWED,
        },
        AnchorValidationError::UnknownGrain {
            got: Some(s("x")),
            allowed: ALLOWED,
        },
        AnchorValidationError::UnknownHashStability {
            got: s("x"),
            allowed: ALLOWED,
        },
        AnchorValidationError::MissingArtifact,
        AnchorValidationError::HashOnNonHashClass { class: "authored" },
        AnchorValidationError::ContentAndHash,
        AnchorValidationError::ContentNotAcceptedForGrain { grain: "tree" },
        AnchorValidationError::UnitAbsentFromContent {
            artifact: s("a.rs"),
        },
        AnchorValidationError::SpanLocatorUnusable {
            artifact: s("a.rs"),
            reason: "bad range",
        },
        AnchorValidationError::SpanOutsideContent {
            artifact: s("a.rs"),
            lines: 3,
        },
        AnchorValidationError::DuplicateAnchorTriple {
            artifact: s("a.rs"),
            grain: "file",
            class: "anchored",
            span: None,
        },
        AnchorValidationError::SpanAndHash,
        AnchorValidationError::SpanEmpty,
        AnchorValidationError::SpanNotAcceptedForGrain { grain: "tree" },
        AnchorValidationError::SpanAbsentFromContent {
            artifact: s("a.rs"),
            span: s("words"),
        },
        AnchorValidationError::EmptySource,
        AnchorValidationError::SourceNotDeclared {
            got: s("x"),
            declared: v(&["source-tree"]),
        },
        AnchorValidationError::ArtifactUnresolvable {
            artifact: s("a.rs"),
            candidates: v(&["src/a.rs"]),
        },
        AnchorValidationError::GrainNamespaceUnsupported {
            grain: "span",
            medium_type: s("graph"),
            anchor_namespace: "entity",
        },
        AnchorValidationError::PathGrainOnUrlArtifact {
            grain: "file",
            artifact: s("https://x"),
        },
    ]
}

/// Every error, labelled by its variant (and inner variant).
pub fn every_engine_error() -> Vec<(String, EngineError)> {
    let id = || s("specs--alpha");
    let mut out: Vec<(String, EngineError)> = vec![
        ("DuplicateMem".into(), EngineError::DuplicateMem(s("specs"))),
        (
            "UnknownMem".into(),
            EngineError::UnknownMem {
                name: s("nope"),
                known_mems: v(&["specs"]),
                writable_mems: v(&["specs"]),
            },
        ),
        (
            "MemUnmounted".into(),
            EngineError::MemUnmounted { mem: s("gone") },
        ),
        (
            "MemQuarantined".into(),
            EngineError::MemQuarantined {
                mem: s("specs"),
                reason_code: s("PARSE_ERROR"),
                reason_message: s("broken file"),
            },
        ),
        (
            "MemQuarantined/MEM_REPO_MISSING".into(),
            EngineError::MemQuarantined {
                mem: s("specs"),
                reason_code: s("MEM_REPO_MISSING"),
                reason_message: crate::workspace_store::InstantiateError::MemRepoMissing {
                    mem: s("specs"),
                    gitdir: s("/ws/mem-repo/.git"),
                }
                .to_string(),
            },
        ),
        (
            "ReadOnlyMount".into(),
            EngineError::ReadOnlyMount(s("archive")),
        ),
        (
            "CheckNotRecorded".into(),
            EngineError::CheckNotRecorded { reason: s("why") },
        ),
        (
            "UnknownType".into(),
            EngineError::UnknownType {
                name: s("spc"),
                schema_ref: s("default@1.3.0"),
                declared: v(&["spec", "memo"]),
                suggestion: Some(s("spec")),
            },
        ),
        (
            "AlreadyExists".into(),
            EngineError::AlreadyExists {
                id: id(),
                existing_title: s("Alpha"),
                existing_is_stub: false,
            },
        ),
        (
            "ConstraintUnsatisfied".into(),
            EngineError::ConstraintUnsatisfied {
                entity_type: s("spec"),
                entity_id: id(),
                violations: vec![crate::ops::health::UnsatisfiedConstraint::RequiresWhen {
                    field: s("level"),
                    when_field: s("status"),
                    when_value: s("done"),
                    severity: memstead_schema::ConstraintSeverity::Block,
                }],
            },
        ),
        (
            "RequiredOutgoingUnsatisfied".into(),
            EngineError::RequiredOutgoingUnsatisfied {
                entity_type: s("plan"),
                entity_id: id(),
                missing: vec![crate::ops::MissingRequiredOutgoingBlock {
                    relationships: v(&["PART_OF"]),
                    cardinality: s("1"),
                    severity: memstead_schema::ConstraintSeverity::Block,
                    when_field: None,
                    when_value: None,
                }],
            },
        ),
        (
            "RetypeRefused".into(),
            EngineError::RetypeRefused {
                id: id(),
                from_type: s("spec"),
                to_type: s("memo"),
                problems: vec![crate::engine::outcomes::RetypeProblem::UnknownSection {
                    key: s("identity"),
                    declared: v(&["claim"]),
                    suggestion: None,
                }],
                target_sections: v(&["claim"]),
                target_catch_all: None,
                proposed_section_map: [(s("identity"), s("claim"))].into_iter().collect(),
            },
        ),
        (
            "RetypeNoOp".into(),
            EngineError::RetypeNoOp {
                id: id(),
                entity_type: s("spec"),
            },
        ),
        (
            "InvalidCheckFinding".into(),
            EngineError::InvalidCheckFinding { reason: s("bad") },
        ),
        (
            "RetypeReferrerUnprobeable".into(),
            EngineError::RetypeReferrerUnprobeable {
                id: id(),
                mem: s("lazy"),
                reason: s("unreadable"),
            },
        ),
        (
            "NotFound".into(),
            EngineError::NotFound {
                id: id(),
                suggestions: v(&["specs--alpha"]),
            },
        ),
        (
            "ExpectedHashRequired".into(),
            EngineError::ExpectedHashRequired { id: id() },
        ),
        (
            "HashMismatch".into(),
            EngineError::HashMismatch {
                id: id(),
                current: s("abc"),
                is_stub: false,
            },
        ),
        (
            "HasIncomingRefs".into(),
            EngineError::HasIncomingRefs {
                id: id(),
                referrers: vec![referrer()],
            },
        ),
        (
            "MemHasIncomingRefs".into(),
            EngineError::MemHasIncomingRefs {
                mem: s("specs"),
                referrers: vec![referrer()],
            },
        ),
        (
            "CrossMemLinkNotAllowed".into(),
            EngineError::CrossMemLinkNotAllowed {
                from_mem: s("a"),
                to_mem: s("b"),
            },
        ),
        (
            "CrossMemTargetNotFound".into(),
            EngineError::CrossMemTargetNotFound {
                target_id: s("b--x"),
                target_mem: s("b"),
            },
        ),
        (
            "EntityIdMissingMem".into(),
            EngineError::EntityIdMissingMem {
                id: s("alpha"),
                candidates: v(&["a--alpha", "b--alpha"]),
            },
        ),
        (
            "CrossMemEdgeNotDeclared".into(),
            EngineError::CrossMemEdgeNotDeclared {
                source_schema: s("default@1.3.0"),
                target_schema: s("planning@0.7.0"),
                rel_type: s("USES"),
                from_id: id(),
                to_id: s("p--x"),
            },
        ),
        (
            "RepairNotNeeded".into(),
            EngineError::RepairNotNeeded {
                id: id(),
                recovery: s("use relate"),
            },
        ),
        (
            "RenameNoOp".into(),
            EngineError::RenameNoOp {
                id: id(),
                new_title: s("Alpha"),
            },
        ),
        ("EmptyUpdate".into(), EngineError::EmptyUpdate { id: id() }),
        (
            "RenameBlockedByCrossMemPolicy".into(),
            EngineError::RenameBlockedByCrossMemPolicy {
                from_mem: s("a"),
                blocked_referrers: vec![BlockedReferrer {
                    from_mem: s("b"),
                    to_mem: s("a"),
                    count: 1,
                }],
            },
        ),
        (
            "WikiLinkWithoutRelation".into(),
            EngineError::WikiLinkWithoutRelation {
                from_id: id(),
                missing: vec![MissingWikiLink {
                    section_key: s("purpose"),
                    target_id: s("specs--beta"),
                }],
            },
        ),
        (
            "RelationHasBodyLinks".into(),
            EngineError::RelationHasBodyLinks {
                from_id: id(),
                to_id: s("specs--beta"),
                rel_type: s("USES"),
                body_links: v(&["purpose"]),
            },
        ),
        (
            "RenamePartialFailure".into(),
            EngineError::RenamePartialFailure {
                committed_mems: v(&["a"]),
                failed_mem: s("b"),
                failure_cause: s("storage"),
                mems: vec![PartialMem {
                    mem: s("a"),
                    landed: true,
                    error: None,
                    items: vec![super::PartialItem {
                        id: s("a--x"),
                        landed: true,
                        current_hash: Some(s("h")),
                    }],
                }],
            },
        ),
        (
            "CommitNotApplied".into(),
            EngineError::CommitNotApplied {
                mem: s("specs"),
                detail: s("d"),
            },
        ),
        (
            "StaleView".into(),
            EngineError::StaleView {
                mem: s("specs"),
                cause: crate::engine::StaleCause::Drift,
                detail: s("d"),
            },
        ),
        (
            "StaleView/out_of_service".into(),
            EngineError::StaleView {
                mem: s("specs"),
                cause: crate::engine::StaleCause::OutOfService,
                detail: s("SCHEMA_NOT_FOUND: d"),
            },
        ),
        (
            "StaleView/lock_unavailable".into(),
            EngineError::StaleView {
                mem: s("specs"),
                cause: crate::engine::StaleCause::LockUnavailable,
                detail: s("d"),
            },
        ),
        (
            "StaleView/discard_failed".into(),
            EngineError::StaleView {
                mem: s("specs"),
                cause: crate::engine::StaleCause::DiscardFailed,
                detail: s("d"),
            },
        ),
        (
            "StubCannotRelate".into(),
            EngineError::StubCannotRelate { id: id() },
        ),
        (
            "StubNotUpdatable".into(),
            EngineError::StubNotUpdatable { id: id() },
        ),
        (
            "StubNotRenamable".into(),
            EngineError::StubNotRenamable { id: id() },
        ),
        (
            "InvalidEntityId".into(),
            EngineError::InvalidEntityId {
                id: s("A B"),
                reason: s("space"),
            },
        ),
        (
            "EntityIdTooLong".into(),
            EngineError::EntityIdTooLong {
                id: s("x"),
                length: 600,
                max: 512,
            },
        ),
        (
            "InvalidWikiLinkTarget".into(),
            EngineError::InvalidWikiLinkTarget {
                raw: s("[[A B]]"),
                suggested: Some(s("a-b")),
                section: s("purpose"),
                link_source: s("body"),
                reason: s("space"),
            },
        ),
        (
            "InvalidWikiLinkMem".into(),
            EngineError::InvalidWikiLinkMem {
                raw: s("[[x:y]]"),
                section: s("purpose"),
                reason: s("unknown mem"),
            },
        ),
        (
            "ConflictingSectionModes".into(),
            EngineError::ConflictingSectionModes {
                section: s("purpose"),
                modes: v(&["sections", "append_sections"]),
            },
        ),
        (
            "RelationshipCycle".into(),
            EngineError::RelationshipCycle {
                rel_type: s("PART_OF"),
                from: EntityId(s("specs--a")),
                to: EntityId(s("specs--b")),
                existing_path: vec![EntityId(s("specs--b")), EntityId(s("specs--a"))],
                path_truncated: false,
                acyclic_set: Some(v(&["PART_OF"])),
                existing_path_rel_types: Some(v(&["PART_OF"])),
            },
        ),
        (
            "SetAndUnsetConflict".into(),
            EngineError::SetAndUnsetConflict {
                keys: v(&["level"]),
            },
        ),
        (
            "RequiredFieldUnset".into(),
            EngineError::RequiredFieldUnset {
                field: s("level"),
                entity_type: s("spec"),
                field_description: Some(s("maturity level")),
                enum_values: v(&["M0"]),
                type_write_rules: v(&["set a level"]),
                on_create: true,
                missing: vec![missing_field()],
            },
        ),
        (
            "MissingRequiredSection".into(),
            EngineError::MissingRequiredSection {
                entity_type: s("spec"),
                missing_count: 1,
                sections: vec![MissingRequiredSection {
                    entity_type: s("spec"),
                    key: s("purpose"),
                    heading: s("Purpose"),
                    write_rules: v(&["say why"]),
                }],
                type_guidance: [(s("purpose"), v(&["say why"]))].into_iter().collect(),
                pre_announced_missing_fields: vec![missing_field()],
            },
        ),
        (
            "PatchSectionEmpty".into(),
            EngineError::PatchSectionEmpty {
                section: s("purpose"),
            },
        ),
        (
            "PatchOldNotFound".into(),
            EngineError::PatchOldNotFound {
                section: s("purpose"),
                current_content: s("text"),
                truncated: false,
                found_in_sections: v(&["identity"]),
            },
        ),
        (
            "UnterminatedFenceInStoredBody".into(),
            EngineError::UnterminatedFenceInStoredBody {
                id: id(),
                section: s("purpose"),
                fence: s("```"),
                swallowed: v(&["identity"]),
            },
        ),
        (
            "ParseAfterWrite".into(),
            EngineError::ParseAfterWrite(s("bad")),
        ),
        (
            "SchemaNotFound".into(),
            EngineError::SchemaNotFound {
                mem: s("specs"),
                pin: s("nope@1.0.0"),
                sources: vec![super::SchemaSourceDiagnostic {
                    source: "builtin",
                    versions_found: v(&["0.9.0"]),
                    pinned_version_match: false,
                    status: None,
                }],
                install_hint: Some(s("memstead schema install")),
            },
        ),
        (
            "EmbeddedSchemaInvalid".into(),
            EngineError::EmbeddedSchemaInvalid {
                mem: s("specs"),
                pin: s("x@1.0.0"),
                reason: s("bad"),
            },
        ),
        (
            "SchemaPackageInvalid".into(),
            EngineError::SchemaPackageInvalid {
                name: s("x"),
                version: s("1.0.0"),
                message: s("bad"),
            },
        ),
        (
            "SchemaResolverInit".into(),
            EngineError::SchemaResolverInit(s("bad")),
        ),
        ("Mem".into(), EngineError::Mem(s("bad"))),
        (
            "MemNameCollision".into(),
            EngineError::MemNameCollision {
                name: s("specs"),
                source_origin: s("archive"),
            },
        ),
        (
            "ProcessMemNotEligible".into(),
            EngineError::ProcessMemNotEligible {
                mem: s("specs"),
                process_mem: s("p"),
                reason: "schema",
                schema: Some(s("default@1.3.0")),
            },
        ),
        ("InvalidInput".into(), EngineError::InvalidInput(s("bad"))),
        (
            "UnknownTypes".into(),
            EngineError::UnknownTypes {
                schema_ref: s("software@0.4.0"),
                unknown: v(&["galaxy-brain"]),
                declared: v(&["actor", "spec"]),
            },
        ),
        (
            "UnknownSchema".into(),
            EngineError::UnknownSchema {
                name: s("nope"),
                available: v(&["default", "planning"]),
            },
        ),
        (
            "InvalidTargetSchema".into(),
            EngineError::InvalidTargetSchema {
                raw: s("x@"),
                reason: s("no version"),
            },
        ),
        (
            "MergeConflictUnsupportedBackend".into(),
            EngineError::MergeConflictUnsupportedBackend { mem: s("specs") },
        ),
        (
            "NotConflicted".into(),
            EngineError::NotConflicted { id: id() },
        ),
        (
            "ConflictNotTimestampOnly".into(),
            EngineError::ConflictNotTimestampOnly {
                id: id(),
                key: None,
                section: Some(s("Purpose")),
            },
        ),
        (
            "UnknownRemote".into(),
            EngineError::UnknownRemote(s("origin")),
        ),
        (
            "RemoteUnreachable".into(),
            EngineError::RemoteUnreachable {
                remote: s("origin"),
                url: s("https://example.invalid/r.git"),
                detail: s("fatal: unable to access"),
            },
        ),
        (
            "LocalDivergence".into(),
            EngineError::LocalDivergence {
                mem: s("specs"),
                remote_ref: s("origin/specs"),
            },
        ),
        (
            "NonFastForward".into(),
            EngineError::NonFastForward {
                mem: s("specs"),
                remote: s("origin"),
            },
        ),
        (
            "LocalInvalidState".into(),
            EngineError::LocalInvalidState {
                mem: s("specs"),
                remote: s("origin"),
                detail: s("d"),
            },
        ),
        (
            "SchemaViolationInFetch".into(),
            EngineError::SchemaViolationInFetch {
                mem: s("specs"),
                ref_name: s("origin/specs"),
                violations: v(&["bad"]),
            },
        ),
        (
            "PushedCommitsProtected".into(),
            EngineError::PushedCommitsProtected {
                mem: s("specs"),
                target_sha: s("a"),
                pushed_shas: v(&["b"]),
            },
        ),
        (
            "BranchResetHeadMoved".into(),
            EngineError::BranchResetHeadMoved {
                mem: s("specs"),
                expected: s("a"),
                current: s("b"),
            },
        ),
        ("UnknownRef".into(), EngineError::UnknownRef(s("refs/x"))),
        (
            "ProposalDispositionsIncomplete".into(),
            EngineError::ProposalDispositionsIncomplete {
                fork: s("f"),
                missing: v(&["a"]),
                unexpected: v(&["b"]),
            },
        ),
        (
            "ProposalConflict".into(),
            EngineError::ProposalConflict {
                fork: s("f"),
                slug: s("a"),
                kind: s("both"),
            },
        ),
        (
            "ProposalStale".into(),
            EngineError::ProposalStale {
                fork: s("f"),
                side: s("target"),
                recorded: s("a"),
                current: s("b"),
            },
        ),
        (
            "ProposalUnattributed".into(),
            EngineError::ProposalUnattributed {
                fork: s("f"),
                slug: s("a"),
                sha: s("c"),
            },
        ),
        (
            "ProposalLinksRejected".into(),
            EngineError::ProposalLinksRejected {
                fork: s("f"),
                slug: s("a"),
                rel_type: s("USES"),
                target: s("b"),
            },
        ),
        (
            "RenameSimilarityOutOfRange".into(),
            EngineError::RenameSimilarityOutOfRange {
                requested: 2.0,
                allowed_min: 0.0,
                allowed_max: 1.0,
            },
        ),
        (
            "InvalidChangesCursor".into(),
            EngineError::InvalidChangesCursor {
                mem: s("specs"),
                since: s("zzz"),
            },
        ),
        (
            "InvalidTimestampCursor".into(),
            EngineError::InvalidTimestampCursor {
                mem: s("specs"),
                since: s("zzz"),
            },
        ),
        (
            "ReviewMarkNotSet".into(),
            EngineError::ReviewMarkNotSet { mem: s("specs") },
        ),
        (
            "MemConfigIncomplete".into(),
            EngineError::MemConfigIncomplete {
                mem: s("specs"),
                missing_fields: v(&["version"]),
            },
        ),
        (
            "MissingRequiredDescription".into(),
            EngineError::MissingRequiredDescription {
                rel_type: s("USES"),
                from_id: id(),
                to_id: s("specs--beta"),
            },
        ),
        (
            "DescriptionNotPermitted".into(),
            EngineError::DescriptionNotPermitted {
                rel_type: s("USES"),
                from_id: id(),
                to_id: s("specs--beta"),
            },
        ),
        (
            "RelationManualAuthoringForbidden".into(),
            EngineError::RelationManualAuthoringForbidden {
                rel_type: s("REFERENCES"),
                from_id: id(),
                to_id: s("specs--beta"),
                guidance: s("write a body link"),
            },
        ),
        ("SearchUnavailable".into(), EngineError::SearchUnavailable),
        ("NoWritableMem".into(), EngineError::NoWritableMem),
        (
            "AnchorsSidecarUnreadable".into(),
            EngineError::AnchorsSidecarUnreadable {
                mem: s("specs"),
                reason: s("bad json"),
            },
        ),
        (
            "AmbiguousQuery".into(),
            EngineError::AmbiguousQuery {
                query: s("alp"),
                candidates: vec![serde_json::json!({ "id": "specs--alpha" })],
            },
        ),
        (
            "ContextNotComputable".into(),
            EngineError::ContextNotComputable { id: id() },
        ),
        (
            "MemNotReadOnly".into(),
            EngineError::MemNotReadOnly { mem: s("specs") },
        ),
        (
            "UnsupportedWorkspaceShape".into(),
            EngineError::UnsupportedWorkspaceShape { path: s("/ws") },
        ),
        (
            "InvalidSchemaPin".into(),
            EngineError::InvalidSchemaPin {
                pin: s("default@latest"),
                reason: s("exact semver versions only"),
                available: v(&["default"]),
            },
        ),
        (
            "ChunkOutOfRange".into(),
            EngineError::ChunkOutOfRange { chunk: 3, total: 1 },
        ),
        (
            "TemplateIdentityMismatch".into(),
            EngineError::TemplateIdentityMismatch {
                id: s("specs--alpha"),
                field: s("title"),
                template: s("Beta"),
                current: s("Alpha"),
            },
        ),
        (
            "NotGitBacked".into(),
            EngineError::NotGitBacked {
                mem: Some(s("specs")),
                operation: s("memstead push"),
            },
        ),
        (
            "AmbiguousMem".into(),
            EngineError::AmbiguousMem {
                mems: v(&["a", "b"]),
            },
        ),
        (
            "MarkdownExportUnsupportedBackend".into(),
            EngineError::MarkdownExportUnsupportedBackend {
                mem: s("specs"),
                active_backend: s("git-branch"),
                supported_backends: v(&["folder"]),
            },
        ),
        (
            "InEntity(NotFound)".into(),
            EngineError::NotFound {
                id: id(),
                suggestions: v(&["specs--alpha"]),
            }
            .in_entity("specs/alpha.md", "amend"),
        ),
    ];
    for e in validation_samples() {
        out.push((
            format!("Validation::{}", label(&e)),
            EngineError::Validation(e),
        ));
    }
    for e in section_format_samples() {
        out.push((
            format!("SectionFormatRefused::{}", label(&e)),
            EngineError::SectionFormatRefused {
                entity_type: s("spec"),
                entity_id: id(),
                violation: e,
            },
        ));
    }
    for e in slug_samples() {
        out.push((
            format!("InvalidTitle::{}", label(&e)),
            EngineError::InvalidTitle(e),
        ));
    }
    for e in parse_samples() {
        out.push((format!("Parse::{}", label(&e)), EngineError::Parse(e)));
    }
    for e in backend_samples() {
        out.push((format!("Backend::{}", label(&e)), EngineError::Backend(e)));
    }
    for e in anchor_samples() {
        out.push((
            format!("InvalidAnchor::{}", label(&e)),
            EngineError::InvalidAnchor(e),
        ));
    }
    out
}

/// The variant name from the `Debug` rendering.
fn label(e: &impl std::fmt::Debug) -> String {
    let d = format!("{e:?}");
    d.split([' ', '(', '{'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Every mem-management error (the engine's second error type), labelled.
pub fn every_full_engine_error() -> Vec<(String, crate::FullEngineError)> {
    use crate::FullEngineError as F;
    let out = vec![
        (
            "Full::Engine(NotFound)".to_string(),
            F::Engine(EngineError::NotFound {
                id: s("specs--alpha"),
                suggestions: v(&["specs--alpha"]),
            }),
        ),
        (
            "Full::MemPathNotAllowed".to_string(),
            F::MemPathNotAllowed {
                attempted: std::path::PathBuf::from("/x/y"),
                candidate: s("y"),
                patterns: v(&["mems/*"]),
                reason: "no_match",
                policy_table: "mem_management.create",
            },
        ),
        (
            "Full::InvalidMemName".to_string(),
            F::InvalidMemName {
                name: s("A B"),
                reason: "space",
            },
        ),
        (
            "Full::MemReferencedByPolicy".to_string(),
            F::MemReferencedByPolicy {
                name: s("specs"),
                referring_mems: v(&["notes"]),
            },
        ),
        (
            "Full::MemSchemaNotAllowed".to_string(),
            F::MemSchemaNotAllowed {
                candidate: s("y"),
                matched_pattern: s("mems/*"),
                requested_schema: s("default@1.3.0"),
                allowed_schemas: v(&["planning@0.7.0"]),
            },
        ),
        (
            "Full::ConfigAlreadyExists".to_string(),
            F::ConfigAlreadyExists {
                path: std::path::PathBuf::from("/x/.memstead/config.json"),
            },
        ),
        (
            "Full::MemStorageResidueDetected".to_string(),
            F::MemStorageResidueDetected {
                branch_ref: s("refs/heads/y"),
                config_blob: Some(s("abc")),
                entity_count: 3,
            },
        ),
        (
            "Full::MemNameRefConflict".to_string(),
            F::MemNameRefConflict {
                name: s("a/b"),
                branch_ref: s("refs/heads/a/b"),
                conflicting_branches: v(&["refs/heads/a"]),
                suggestion: Some(s("a-b")),
            },
        ),
    ];
    out
}

/// The tags of `all` no sample in `samples` carries, as `Type::Variant`.
fn missing<T: Copy + Ord + std::fmt::Debug>(
    ty: &str,
    all: &[T],
    samples: impl IntoIterator<Item = T>,
) -> Vec<String> {
    let have: std::collections::BTreeSet<T> = samples.into_iter().collect();
    all.iter()
        .filter(|t| !have.contains(t))
        .map(|t| format!("{ty}::{t:?}"))
        .collect()
}

/// Every variant, of the engine's errors and of the inner errors they wrap,
/// that no sample carries. Empty when the lists are complete; the parity
/// test refuses anything else.
pub fn missing_samples() -> Vec<String> {
    let mut out = Vec::new();
    out.extend(missing(
        "EngineError",
        EngineTag::ALL,
        every_engine_error().iter().map(|(_, e)| EngineTag::of(e)),
    ));
    out.extend(missing(
        "FullEngineError",
        FullTag::ALL,
        every_full_engine_error()
            .iter()
            .map(|(_, e)| FullTag::of(e)),
    ));
    out.extend(missing(
        "ValidationError",
        ValidationTag::ALL,
        validation_samples().iter().map(ValidationTag::of),
    ));
    out.extend(missing(
        "SectionFormatViolation",
        SectionFormatTag::ALL,
        section_format_samples().iter().map(SectionFormatTag::of),
    ));
    out.extend(missing(
        "SlugError",
        SlugTag::ALL,
        slug_samples().iter().map(SlugTag::of),
    ));
    out.extend(missing(
        "ParseError",
        ParseTag::ALL,
        parse_samples().iter().map(ParseTag::of),
    ));
    out.extend(missing(
        "BackendError",
        BackendTag::ALL,
        backend_samples().iter().map(BackendTag::of),
    ));
    out.extend(missing(
        "AnchorValidationError",
        AnchorTag::ALL,
        anchor_samples().iter().map(AnchorTag::of),
    ));
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_variant_of_every_error_has_a_sample() {
        assert_eq!(super::missing_samples(), Vec::<String>::new());
    }
}

/// Each `MEM_VIEW_STALE` cause renders its own message and its own
/// `details.cause`; only drift says the mem changed in storage.
#[cfg(test)]
mod stale_view_causes {
    use crate::engine::{EngineError, StaleCause};

    #[test]
    fn each_cause_names_its_state_and_only_drift_says_storage_changed() {
        let cases = [
            (StaleCause::Drift, "drift", "changed in storage"),
            (
                StaleCause::OutOfService,
                "out_of_service",
                "is out of service",
            ),
            (
                StaleCause::LockUnavailable,
                "lock_unavailable",
                "write lock cannot be taken",
            ),
            (
                StaleCause::DiscardFailed,
                "discard_failed",
                "could not be discarded",
            ),
        ];
        for (cause, wire, phrase) in cases {
            let e = EngineError::StaleView {
                mem: "specs".to_string(),
                cause,
                detail: "d".to_string(),
            };
            assert_eq!(e.code(), "MEM_VIEW_STALE");
            assert_eq!(e.details()["cause"], wire);
            let message = e.to_string();
            assert!(message.contains(phrase), "{wire}: {message}");
            assert_eq!(
                message.contains("changed in storage"),
                cause == StaleCause::Drift,
                "{wire}: {message}"
            );
        }
    }
}
