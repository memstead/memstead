use super::*;

impl Engine {
    /// Resolve phase: the target id's grammar, the cross-mem policy and
    /// read-only gates, the canonical rel-type and description, the
    /// source mount and schema, the target's schema for routing, the
    /// source entity (never a stub) and what is known of the target.
    pub(super) fn resolve_relate(
        &self,
        args: RelateEntityArgs,
        mut drift_warnings: Vec<WarningHint>,
    ) -> Result<ResolvedRelate, EngineError> {
        let mut args = args;
        let source_mem = args.source.mem().to_string();
        let target_mem = args.target.mem().to_string();

        // Target-id grammar gate (shared helper, also called from
        // `Engine::create_entity` for inline relations so both
        // gateways trip the same envelope). Source-id grammar is
        // implicit — a malformed source surfaces as `ENTITY_NOT_FOUND`
        // because it can never have been created.
        //
        // The grammar check runs BEFORE the cross-mem policy check:
        // a bare-string target with no `--` separator (e.g.
        // `bad target`) parses as `mem: ""`, `path: "bad target"`,
        // and without this ordering would surface a cross-mem
        // policy error against an empty mem name — pointing the
        // agent at workspace policy when the actual fix is a
        // malformed id. The grammar check is intrinsic to the target
        // id; it doesn't need to know which mem the target lives
        // in.
        validate_relation_target_grammar(&args.target)?;

        // Track whether the cross-mem target's mem is unmounted —
        // we deferred the warning emission to the canonical
        // `warnings` vec initialisation below, but the policy / RO
        // gates fire first to keep the refusal-before-warning ordering:
        // a policy refusal preempts the warning.
        let mut target_mem_uncreated = false;
        if source_mem != target_mem {
            // Policy gates *new* edges only — remove is structurally
            // cleanup. The same convention governs the acyclic, shape,
            // and schema gates below (each one wraps `if !args.remove`).
            // Without this bypass, a workspace whose cross-mem grant
            // was revoked while edges still existed gets wedged: the
            // grant must be re-introduced just to delete the data it
            // permitted, then re-revoked. The gate-on-add
            // rule holds because `cross_mem_links: named` semantically reads
            // as "only these new edges may be created", not "these
            // edges may exist."
            if !args.remove {
                crate::engine::mutation::validate_cross_mem_add_policy(
                    self,
                    &source_mem,
                    &args.target,
                )?;
            }
            // ReadOnly target mem: the engine has no write access to
            // persist a stub there, so the target must already exist
            // before relate. (Same-mem and cross-mem-to-Write
            // both retain the auto-stub mechanic below.) The add path
            // already refused above via the shared funnel; this check
            // stays unconditional so the remove path keeps its
            // pre-funnel behaviour.
            if let Some(mount) = self.mount(&target_mem)
                && mount.capability == MountCapability::ReadOnly
                && !self.store.contains(&args.target)
                && !matches!(
                    crate::engine::mutation::probe_deferred_target(self, &args.target)?,
                    crate::engine::mutation::DeferredTargetProbe::Exists
                )
            {
                return Err(EngineError::CrossMemTargetNotFound {
                    target_id: args.target.to_string(),
                    target_mem: target_mem.clone(),
                });
            }
            // The target mem isn't mounted in the workspace
            // at all. Policy admitted the edge so the relate must
            // succeed and auto-stub; surface a warning so the operator
            // can distinguish a typo from a deliberate forward
            // reference. The auto-stub still lands via the
            // `AutoStubCreated` path below; this layered warning is
            // additive observability.
            if self.mount(&target_mem).is_none() {
                target_mem_uncreated = true;
            }
        }

        // Canonicalise rel_type to UPPER_SNAKE_CASE so the schema lookup,
        // stored edge, and response all see the same wire-contract form
        // ("case-insensitive on input"). Syntax errors (non-letter
        // characters) fall through to the strict-mode schema check below,
        // which surfaces them as INVALID_REL_TYPE with the declared
        // vocabulary.
        if let Ok(canonical) = crate::entity::id::validate_rel_type(&args.rel_type) {
            args.rel_type = canonical;
        }
        // Normalise the description at the boundary so empty /
        // whitespace-only strings collapse to `None` before the
        // posture check and before the renderer ever sees them.
        args.description = normalise_description(args.description.as_deref());

        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == source_mem)
            .ok_or_else(|| self.unknown_mem_error(&source_mem))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(source_mem));
        }

        let schema_arc = self
            .schemas
            .get(&source_mem)
            .expect("schema present for every registered mount")
            .clone();
        let schema = &schema_arc;

        // Determine whether this is a cross-mem edge to a mem
        // pinning a schema with a *different name*. Same-name (any
        // version pair — a schema name is a domain) and same-mem
        // stay on the intra-mem validation path, governed by the
        // source mem's pinned version; cross-different-schema
        // routes vocabulary and shape checks through the source
        // schema's `cross_mem_relationships:` section.
        //
        // If the target mem is not mounted (unknown to the engine
        // — typically only in malformed callers), there is no target
        // schema to consult and the validation falls back to the
        // intra-mem path. Real workspaces always mount the target
        // mem before relating.
        let target_schema_ref: Option<SchemaRef> = if source_mem == target_mem {
            None
        } else {
            // Loaded mems answer from the schema catalogue; an
            // unmounted mem with discoverable storage answers from
            // its stored config's pin — see
            // `target_schema_ref_for_routing`.
            crate::engine::mutation::target_schema_ref_for_routing(self, &target_mem)
        };
        let cross_mem_different = match (&target_schema_ref, schema.id()) {
            (Some(target), (src_name, _)) => target.name != src_name,
            (None, _) => false,
        };

        let mut warnings: Vec<WarningHint> = Vec::new();
        // Reload-before-operation drift notice, surfaced first.
        warnings.append(&mut drift_warnings);
        // Clone the source entity early so subsequent mutable
        // operations on `self.store` (the stub-creation upsert below)
        // don't conflict with the borrow.
        let entity: Entity = self
            .store
            .get(&args.source)
            .ok_or_else(|| self.not_found(args.source.as_ref()))?
            .clone();

        // Stubs have no `entity_type`, so the schema lookup below
        // would surface a cryptic `UnknownType { name: "" }`. Surface
        // the actual constraint instead — a stub source has no body
        // to write to and no schema-resolved type to validate
        // against. Promotion via `memstead_create` adopts the stub's
        // incoming references and lets the agent re-issue the relate
        // against a real entity.
        if entity.stub {
            return Err(EngineError::StubCannotRelate {
                id: args.source.to_string(),
            });
        }

        let target_type = self
            .store
            .get(&args.target)
            .map(|e| e.entity_type.clone())
            .filter(|t| !t.is_empty());
        // Storage verification for a target in a DEFERRED mem:
        // the store cannot answer for an unloaded
        // mem, so existence comes from the cheap storage probe and —
        // when the shape check will need it — the type from the one
        // resolved blob. Neither triggers the mem's load.
        let target_verified_in_storage = if target_type.is_none() && target_mem != source_mem {
            matches!(
                crate::engine::mutation::probe_deferred_target(self, &args.target)?,
                crate::engine::mutation::DeferredTargetProbe::Exists
            )
        } else {
            false
        };
        let target_type = match target_type {
            Some(t) => Some(t),
            None if target_verified_in_storage && !args.remove => {
                crate::engine::mutation::peek_deferred_target_type(self, &args.target)?
            }
            None => None,
        };
        if target_verified_in_storage {
            // A verified target's mem is not "uncreated" — for an
            // unmounted mem the discovery hook just found its storage
            // and the entity in it; the layered warning would claim
            // the opposite.
            target_mem_uncreated = false;
        }
        Ok(ResolvedRelate {
            args,
            source_mem,
            target_mem,
            mount_idx,
            schema: schema_arc,
            target_schema_ref,
            cross_mem_different,
            warnings,
            entity,
            target_type,
            target_verified_in_storage,
            target_mem_uncreated,
        })
    }
}
