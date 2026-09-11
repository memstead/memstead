//! Resolve phase of a create: put the inputs into their canonical
//! forms, locate the mount and gate on its capability, and look up
//! the schema and the type definition every later phase reads.
//! Nothing here looks at the entity's content.

use std::sync::Arc;

use memstead_schema::{Schema, TypeDefinition};

use crate::ops::WarningHint;
use crate::workspace::MountCapability;

use super::super::unknown_type_error;
use super::{CreateEntityArgs, Engine, EngineError};

/// A create after resolution: the mount, the schema and the type the
/// validate phase gates against, plus the inputs with their canonical
/// forms applied.
pub(super) struct ResolvedCreate {
    /// The inputs with every inline rel-type canonicalised and the
    /// title trimmed.
    pub(super) args: CreateEntityArgs,
    pub(super) mount_idx: usize,
    pub(super) schema: Arc<Schema>,
    pub(super) type_def: Arc<TypeDefinition>,
    /// `TITLE_TRIMMED` when trimming changed the title — surfaced by
    /// the validate phase once the warnings accumulator exists.
    pub(super) title_trimmed_warning: Option<WarningHint>,
}

impl Engine {
    /// Canonicalise the inputs, resolve the mount and gate on its
    /// capability, then resolve the schema and the type.
    pub(super) fn resolve_create(
        &self,
        args: CreateEntityArgs,
    ) -> Result<ResolvedCreate, EngineError> {
        let mut args = args;
        // Canonicalise rel_type on every inline relation — same contract
        // as `relate_entity`: input is case-insensitive, storage and
        // response are UPPER_SNAKE_CASE. Syntax errors fall through to
        // the schema check, which surfaces them as INVALID_REL_TYPE.
        for rel in &mut args.relations {
            if let Ok(canonical) = crate::entity::id::validate_rel_type(&rel.rel_type) {
                rel.rel_type = canonical;
            }
        }

        // Trim surrounding whitespace from the title before slug
        // derivation + storage. Internal whitespace is preserved.
        // Fully-whitespace titles collapse to empty and fall through to
        // the validator below (which already refuses empty). Without
        // trimming, a caller-supplied
        // `"   Foo   "` renders with leading/trailing spaces despite the
        // slug being correct. We emit `TITLE_TRIMMED` whenever trimming
        // changed the value so the audit trail records the drift.
        let mut title_trimmed_warning: Option<WarningHint> = None;
        let trimmed_title = args.title.trim();
        if trimmed_title.len() != args.title.len() {
            title_trimmed_warning = Some(WarningHint::TitleTrimmed {
                original: args.title.clone(),
                trimmed: trimmed_title.to_string(),
            });
            args.title = trimmed_title.to_string();
        }

        // 1. Resolve the mount and gate on capability.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == args.mem)
            .ok_or_else(|| self.unknown_mem_error(&args.mem))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(args.mem));
        }

        // 1a. Reload-before-operation. Probe the mem ref and reload
        //     if a sibling writer advanced it past our cached head, so
        //     the duplicate-id check below and the eventual commit both
        //     run against current truth. Any `MemReloaded` warning
        //     rides the outcome's `warnings` (merged at the accumulator
        //     below). This is what makes a create at an id a sibling
        //     just created refuse as already-exists rather than
        //     silently rebasing onto an unobserved commit.
        // 2. Resolve schema + type. The schema map is populated for
        //    every mount during `from_mounts`, so the lookup is total.
        let schema = self
            .schemas
            .get(&args.mem)
            .expect("schema present for every registered mount")
            .clone();
        let type_def = schema
            .get_type(&args.entity_type)
            .ok_or_else(|| unknown_type_error(schema.as_ref(), &args.entity_type))?;

        Ok(ResolvedCreate {
            args,
            mount_idx,
            schema,
            type_def,
            title_trimmed_warning,
        })
    }
}
