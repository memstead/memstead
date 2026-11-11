use super::*;

impl Engine {
    /// Resolve phase: the entity (no stub, the caller's hash), the mem's
    /// schema and the target type, refusing a retype to the type the
    /// entity already has.
    pub(super) fn resolve_retype(
        &self,
        args: RetypeEntityArgs,
    ) -> Result<ResolvedRetype, EngineError> {
        let id = &args.id;
        let mem = id.mem().to_string();
        let mount_idx = self.writable_mount_index(&mem)?;

        let entity = self
            .store
            .get(id)
            .ok_or_else(|| self.not_found(id.as_ref()))?
            .clone();
        if entity.stub {
            return Err(EngineError::StubNotUpdatable { id: id.to_string() });
        }
        args.expected_hash.check(id, &entity, true, !args.dry_run)?;

        let schema = self
            .schemas
            .get(&mem)
            .expect("schema present for every registered mount")
            .clone();
        let target_def = schema
            .get_type(&args.target_type)
            .ok_or_else(|| unknown_type_error(&schema, &args.target_type))?;
        if entity.entity_type == args.target_type {
            return Err(EngineError::RetypeNoOp {
                id: id.to_string(),
                entity_type: entity.entity_type.clone(),
            });
        }
        Ok(ResolvedRetype {
            args,
            mem,
            mount_idx,
            entity,
            schema,
            target_def,
        })
    }
}
