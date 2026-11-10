//! Validate phase of a rename: the gates on the call, in the order
//! the wire contract fixes them (the entity exists, is no stub, the
//! caller's hash matches), then the new identity: the slug the title
//! derives, its length, the no-op of a title that keeps the slug, and
//! the refusal of an id that is taken. Nothing here reads a referrer
//! or writes anything.

use crate::entity::EntityId;
use crate::entity::id::validate_and_derive_slug;
use crate::ops::WarningHint;

use super::super::unknown_type_error;
use super::{
    Engine, EngineError, RenameEntityArgs, RenameEntityOutcome, RenameValidation, ValidatedRename,
};

impl Engine {
    /// Gate a rename and derive its new identity. The caller holds the
    /// write lock and has reloaded the mem.
    pub(super) fn validate_rename(
        &self,
        args: &RenameEntityArgs,
        mount_idx: usize,
    ) -> Result<RenameValidation, EngineError> {
        let id = &args.id;
        let mem = id.mem().to_string();

        let entity = self
            .store
            .get(id)
            .ok_or_else(|| EngineError::NotFound { id: id.to_string() })?;

        // Stub guard — stubs derive their title from the id and have
        // no `entity_type` to validate against. Recovery is
        // `memstead_create` (stub adoption). Item 02 realised the
        // `STUB_NOT_RENAMABLE` code the description list had
        // advertised since the strictness work landed.
        if entity.stub {
            return Err(EngineError::StubNotRenamable { id: id.to_string() });
        }

        args.expected_hash.check(id, entity, true, true)?;

        let derivation = validate_and_derive_slug(&args.new_title)?;
        let new_slug = derivation.slug.clone();
        let new_id = EntityId::new(&mem, &new_slug);
        crate::entity::id::enforce_id_length(new_id.as_ref())?;

        if new_id == *id {
            // Title-with-same-slug: surface a Tier-2 warning rather
            // than touching disk for nothing. Matches full's
            // RenameResult slug-noop shape so autonomous skills
            // can distinguish the silent no-op from a successful
            // cosmetic rewrite via the typed warning code.
            return Ok(RenameValidation::Noop(RenameEntityOutcome {
                old_id: id.clone(),
                new_id: new_id.clone(),
                old_path: entity.file_path.clone(),
                new_path: entity.file_path.clone(),
                content_hash: entity.content_hash.clone(),
                write_id: String::new(),
                warnings: vec![WarningHint::TitleNormalizedToSlugNoop {
                    requested_title: args.new_title.clone(),
                    current_slug: id.name().to_string(),
                }],
            }));
        }
        // A stub at the new id is a forward reference waiting for its
        // entity, and the rename brings it: the references resolve.
        // This is also what a rename that landed in part leaves behind
        // (its peers already point at the new id), so the same call
        // sent again is not refused by its own first half.
        if let Some(existing) = self.store.get(&new_id)
            && !existing.stub
        {
            return Err(EngineError::AlreadyExists {
                id: new_id.to_string(),
                existing_title: existing.title.clone(),
                existing_is_stub: existing.stub,
            });
        }

        let schema = self
            .schemas
            .get(&mem)
            .expect("schema present for every registered mount");
        let type_def = schema
            .get_type(&entity.entity_type)
            .ok_or_else(|| unknown_type_error(schema, &entity.entity_type))?;

        Ok(RenameValidation::Proceed(Box::new(ValidatedRename {
            id: id.clone(),
            new_id,
            mem,
            mount_idx,
            new_title: args.new_title.clone(),
            derivation,
            entity: entity.clone(),
            type_def,
        })))
    }
}
