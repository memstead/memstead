//! The mem schema pin and its migration: setting the pin, reading the stamped generation, persisting it, and bumping the backend's copy.

use super::reload::changed_config_fields;
use super::*;

impl Engine {
    /// Set a mem's schema pin — the conformance-gated schema-migration
    /// trigger. Behaviour per the pinned contract:
    ///
    /// - requested == current pin → `Noop`, no state change.
    /// - requested != pin, mem integral against the target →
    ///   atomic switch (`schema_pin = target`, migration state
    ///   cleared) in one workspace-store write → `Switched`.
    /// - requested != pin, mem NOT integral → enter (or stay in)
    ///   dual-pin: `migration_target = target`, writes validate
    ///   against the target from this call on, `findings` carries
    ///   the non-integral entities → `MigrationStarted`
    ///   (first call) / `MigrationPending` (same target re-issued).
    /// - re-issued with the in-flight target once every entity is
    ///   integral → atomic switch → `Switched`.
    ///
    /// The trigger is a label change gated by the conformance check —
    /// no content hashing. The response hands the agent findings and
    /// nothing else (no migration scripts, no hints); each repair
    /// write is validated strictly against the target.
    pub fn set_mem_schema(
        &mut self,
        mem: &str,
        target: &memstead_schema::SchemaRef,
    ) -> Result<crate::engine::SetSchemaOutcome, EngineError> {
        use crate::engine::{SetSchemaOutcome, SetSchemaResult};
        // A quarantined mem's set-schema IS the repair path (an
        // unresolvable pin is the commonest quarantine cause): repin
        // the retained mount, then re-attempt the attach — the same
        // in-process recovery `reload` performs after an external
        // repair. Ordinary mounts proceed below unchanged.
        if self.quarantine_reason(mem).is_some() {
            return self.set_schema_on_quarantined(mem, target);
        }
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        // Capability gate, identical in shape and position to the six
        // sibling setters (`set_mem_version` … `set_mem_sync_state`):
        // a schema-pin change starts a migration — the one lifecycle
        // mutation a read-only mount must be able to refuse like any
        // other.
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem.to_string()));
        }

        // The requested target must resolve before anything else —
        // an unknown ref is an error, not a migration into nowhere.
        let target_schema = self.resolve_schema_by_ref(target).ok_or_else(|| {
            // The migration resolver consulted workspace-authored
            // schemas layered over the built-ins (`resolve_schema_by_ref`).
            let consulted: Vec<_> = self
                .workspace_schemas
                .iter()
                .chain(self.builtin_schemas.iter())
                .cloned()
                .collect();
            EngineError::SchemaNotFound {
                mem: mem.to_string(),
                pin: target.as_display(),
                sources: crate::engine::error::SchemaSourceDiagnostic::for_failed_pin(
                    &target.name,
                    &target.version,
                    &consulted,
                ),
                install_hint: None,
            }
            .with_schema_install_probe(self.workspace_root())
        })?;

        // `Mount.schema` is now the optional assertion; for a mem the
        // operator is actively re-pinning it is normally `Some` (and kept
        // in sync with the config by the switch below). `<unset>` covers a
        // mount that carried no assertion.
        let current_pin = self.mounts[mount_idx].mount.schema.clone();
        let current_pin_display = current_pin
            .as_ref()
            .map(|p| p.as_display())
            .unwrap_or_else(|| "<unset>".to_string());
        let in_flight = self.mounts[mount_idx].mount.migration_target.clone();

        // The noop question is asked of the pin the engine actually
        // serves (the loaded schema, resolved from the authoritative
        // backend config at boot), never of the mount's expectation
        // alone. With a `SCHEMA_PIN_MISMATCH` (mount says 0.4.0, config
        // says 0.1.0) the old comparison against the mount answered
        // "noop" for a target of 0.4.0 and the config stayed at 0.1.0
        // forever: the one command meant to repair the mismatch could
        // not. When the served pin already equals the target and only
        // the mount expectation lags, the expectation is aligned in
        // place and reported as switched.
        let served_pin: Option<memstead_schema::SchemaRef> = self.schemas.get(mem).map(|s| {
            let (name, version) = s.id();
            memstead_schema::SchemaRef::new(name, version)
        });
        // During a dual-pin migration the served schema IS the target
        // (writes validate against it) while the config still pins the
        // old generation, so the shortcut applies only with no migration
        // in flight; an in-flight target falls through to the
        // conformance gate that completes it.
        if served_pin.as_ref() == Some(target) && in_flight.is_none() {
            if current_pin.as_ref() == Some(target) {
                return Ok(SetSchemaOutcome {
                    mem: mem.to_string(),
                    schema_pin: current_pin_display,
                    migration_target: in_flight.map(|t| t.as_display()),
                    outcome: SetSchemaResult::Noop,
                    findings: Vec::new(),
                    stamped_schema: self.stamped_schema_of(mount_idx),
                });
            }
            self.mounts[mount_idx].mount.schema = Some(target.clone());
            self.mounts[mount_idx].mount.migration_target = None;
            self.persist_state()?;
            return Ok(SetSchemaOutcome {
                mem: mem.to_string(),
                schema_pin: target.as_display(),
                migration_target: None,
                outcome: SetSchemaResult::Switched,
                findings: Vec::new(),
                stamped_schema: self.stamped_schema_of(mount_idx),
            });
        }

        // Conformance gate against the requested target. The full
        // integrity definition includes the consistency axis, but the
        // schema-switch gate is conformance: consistency breaks are
        // schema-independent (they neither block nor are caused by a
        // pin change) and keep their always-available repair paths.
        let findings = crate::ops::integrity::conformance_findings(
            &self.store,
            mem,
            target_schema.as_ref(),
            &self.schemas,
        );

        if findings.is_empty() {
            // Atomic switch. The pin's authoritative home is the backend
            // config (boot resolution prefers it over `Mount.schema`), so
            // persist there FIRST — if that write fails, every other piece
            // of state stays untouched and the switch is a clean no-op.
            // Without this the new pin landed only in `mounts.json` and was
            // silently reverted on the next process boot for any
            // config-present mem.
            self.persist_mem_schema_pin(mount_idx, target)?;
            self.mounts[mount_idx].mount.schema = Some(target.clone());
            self.mounts[mount_idx].mount.migration_target = None;
            self.schemas_insert(mem.to_string(), target_schema);
            self.invalidate_communities();
            // The index field set derives from the pinned schema — the
            // schema-switch staleness the whole-map drop used to mask:
            // this mem's index must be
            // rebuilt against the new field set.
            self.invalidate_search_indexes();
            self.persist_state()?;
            // The completed migration re-stamps the mutation stamp (the
            // marker `ENGINE_VERSION_SKEW` reads) with the generation
            // the mem now sits on. Without this the marker kept naming
            // the old pin until the next entity write, and an agent
            // reading it after the migration re-derived from a stale
            // generation (B5 grader finding, 2026-09-02). The stamp
            // writer's equality guard keeps a no-move write from
            // happening; its warnings ride entity mutations and have no
            // channel on this outcome, the same standing as the sweep.
            let _stamp_warnings_have_no_channel_here = self.stamp_mutation_versions(mount_idx);
            return Ok(SetSchemaOutcome {
                mem: mem.to_string(),
                schema_pin: target.as_display(),
                migration_target: None,
                outcome: SetSchemaResult::Switched,
                findings: Vec::new(),
                stamped_schema: self.stamped_schema_of(mount_idx),
            });
        }

        let outcome = if in_flight.as_ref() == Some(target) {
            SetSchemaResult::MigrationPending
        } else {
            SetSchemaResult::MigrationStarted
        };
        self.mounts[mount_idx].mount.migration_target = Some(target.clone());
        // Writes validate against the target from this point on —
        // the load-bearing dual-pin semantic.
        self.schemas_insert(mem.to_string(), target_schema);
        self.invalidate_communities();
        // Same field-set dependency as the atomic-switch branch above.
        self.invalidate_search_indexes();
        self.persist_state()?;
        // A dual-pin entry never moves the marker: the mem still sits
        // on the old generation until every entity is integral, and
        // the outcome says which generation the marker carries.
        Ok(SetSchemaOutcome {
            mem: mem.to_string(),
            schema_pin: current_pin_display,
            migration_target: Some(target.as_display()),
            outcome,
            findings,
            stamped_schema: self.stamped_schema_of(mount_idx),
        })
    }

    /// The resolved schema the mem's mutation stamp names, from the
    /// engine's cached config (kept current by the shared config
    /// writer), or `None` when the mem carries no stamp.
    pub(super) fn stamped_schema_of(&self, mount_idx: usize) -> Option<String> {
        self.mounts
            .get(mount_idx)
            .and_then(|m| m.mem_config.as_ref())
            .and_then(|c| c.mutation_stamp.as_ref())
            .map(|st| st.schema.clone())
    }

    /// Persist a mem's new schema pin into the authoritative backend
    /// config (`.memstead/config.json` for folder, the `__MEMSTEAD`
    /// mem-config blob for git-branch).
    ///
    /// Boot resolution treats the backend config as the authoritative
    /// settled pin and `Mount.schema` (the `mounts.json` copy) as a
    /// cross-checked assertion. A schema switch that updated only
    /// `mounts.json` would therefore be silently reverted on the next
    /// process boot — the config still names the old pin. This keeps the
    /// authoritative home in sync at switch time.
    ///
    /// Value-level field bump: only the `"schema"` string is rewritten;
    /// every other config field (`readMems`, write guidance, …) is
    /// preserved verbatim. Config-absent mems (no `config.json`) keep
    /// `Mount.schema` as their settled pin, so there is nothing to update
    /// — a clean no-op.
    /// **The one way this engine writes a mem config.** Every config-writing
    /// operation goes through it; none serializes its cached struct
    /// (consistency-sweep 04/03, criteria 1 and 2).
    ///
    /// The defect it removes: a long-lived MCP server reads each mem's config
    /// once at boot and holds it for days. Eight operations used to clone that
    /// cached struct, set one field, and write the whole thing back, so
    /// anything a sibling process changed in between was gone. The
    /// reload-before-operation invariant did not save them, because the
    /// staleness probe watches the entity branch (git-branch) or the change
    /// log (folder) and a config-only write advances neither.
    ///
    /// What this does instead is what the schema-pin writer next door already
    /// did: read the config the backend HAS, mutate the one field on that
    /// JSON, write it back. A field this call did not set cannot be reverted,
    /// because this call never had an opinion about it.
    ///
    /// `apply` receives the freshly-read config, PARSED. It deliberately does
    /// not receive raw JSON: an earlier draft handed out the `Value` and each
    /// closure wrote its own key name, which put `review_mark` in the file
    /// where the struct reads `reviewMark` and lost the field on the next
    /// read. Serde owns the wire names; the closures must not restate them.
    /// Unknown fields survive because `MemConfig` flattens them into `extra`.
    ///
    /// `apply` runs again on a re-read if the file moved between the read and
    /// the write, so it must be a pure function of the config it is handed,
    /// never of the engine's cache.
    ///
    /// Returns the parsed new config plus, when the stored config had moved on
    /// from what this engine last observed, the fields the intervening writer
    /// had changed. The caller rides that on its own response as
    /// `CONFIG_WRITE_INTERVENED`.
    pub(crate) fn write_mem_config_merged(
        &mut self,
        mount_idx: usize,
        mem_name: &str,
        tool: &'static str,
        note: Option<&str>,
        apply: &dyn Fn(&mut memstead_schema::config::MemConfig),
    ) -> Result<(memstead_schema::config::MemConfig, Vec<String>), EngineError> {
        // The config-write commit carries the operation's provenance
        // like every other commit: the tool that caused it, the
        // session's actor, client, role and identity, and the note.
        let ctx = self.session_commit_context(Some(tool), note.map(String::from));
        let backend = self.mounts[mount_idx].backend.as_ref();
        let read = |b: &dyn crate::backend::MemBackend| -> Result<Vec<u8>, EngineError> {
            b.read_mem_config()
                .map_err(|e| EngineError::Mem(format!("read mem config for update: {e}")))?
                .ok_or_else(|| {
                    EngineError::InvalidInput(format!(
                        "mem '{mem_name}' has no stored MemConfig (initialize the mem via \
                         `memstead init` or `memstead mem create` first)"
                    ))
                })
        };

        let stored = read(backend)?;
        // What the intervening writer changed, if anyone did. Compared against
        // the engine's cached copy, never against a clock: the rule forbids
        // reacting to cache age, and a single-writer workspace has an
        // identical cache, so this is empty there.
        let intervened = match self.mounts[mount_idx].mem_config.as_ref() {
            Some(cached) => changed_config_fields(cached, &stored),
            None => Vec::new(),
        };

        let render =
            |raw: &[u8]| -> Result<(memstead_schema::config::MemConfig, Vec<u8>), EngineError> {
                let value: serde_json::Value = serde_json::from_slice(raw)
                    .map_err(|e| EngineError::Mem(format!("parse mem config for update: {e}")))?;
                let mut cfg = memstead_schema::config::parse_mem_config(&value)
                    .map_err(|e| EngineError::Mem(format!("parse mem config for update: {e}")))?;
                apply(&mut cfg);
                let mut bytes = serde_json::to_vec_pretty(&cfg)
                    .map_err(|e| EngineError::Mem(format!("serialize mem config: {e}")))?;
                bytes.push(b'\n');
                Ok((cfg, bytes))
            };
        let (mut parsed, mut bytes) = render(&stored)?;

        // Compare-and-set, done by the backend so the check and the write are
        // one step. An earlier draft re-read here and then wrote, which is
        // check-then-write and leaves exactly the lost-update window
        // open. On a mismatch the loop re-reads, re-applies onto what is
        // there, and retries: the intervening writer's change is merged, never
        // overwritten. Bounded, because an unbounded retry against a hot
        // writer is a hang, and reaching the bound is a real contention
        // problem the caller should hear about rather than a state to spin in.
        let mut expected = stored;
        for attempt in 0..8 {
            let wrote = self.mounts[mount_idx].backend.write_mem_config_cas(
                Some(&expected),
                &bytes,
                &ctx,
            )?;
            if wrote {
                break;
            }
            if attempt == 7 {
                return Err(EngineError::Mem(format!(
                    "mem '{mem_name}' config is being written concurrently: eight \
                     compare-and-set attempts all lost the race. Retry, or find the \
                     writer that is not backing off."
                )));
            }
            expected = read(self.mounts[mount_idx].backend.as_ref())?;
            let rendered = render(&expected)?;
            parsed = rendered.0;
            bytes = rendered.1;
        }

        let mounted = &mut self.mounts[mount_idx];
        mounted.mem_config = Some(parsed.clone());
        // Refresh the head cursor so the next drift probe does not surface
        // MEM_RELOADED for the commit this call just produced.
        if let Some(sha) = mounted.backend.current_head().ok().flatten() {
            mounted.last_known_head = Some(sha);
        }
        Ok((parsed, intervened))
    }

    fn persist_mem_schema_pin(
        &mut self,
        mount_idx: usize,
        target: &memstead_schema::SchemaRef,
    ) -> Result<(), EngineError> {
        let ctx = self.session_commit_context(Some("set_mem_schema"), None);
        let value = bump_backend_schema_pin(self.mounts[mount_idx].backend.as_ref(), target, &ctx)?;
        // Refresh the cached parsed config so in-session reads observe the
        // new pin without a reload.
        if let Some(value) = value
            && let Ok(cfg) = memstead_schema::config::parse_mem_config(&value)
        {
            self.mounts[mount_idx].mem_config = Some(cfg);
        }
        Ok(())
    }
}

pub fn bump_backend_schema_pin(
    backend: &dyn crate::backend::MemBackend,
    target: &memstead_schema::SchemaRef,
    ctx: &crate::vcs::CommitContext<'_>,
) -> Result<Option<serde_json::Value>, EngineError> {
    let Some(bytes) = backend
        .read_mem_config()
        .map_err(|e| EngineError::Mem(format!("read mem config for pin update: {e}")))?
    else {
        return Ok(None);
    };
    let mut value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| EngineError::Mem(format!("parse mem config for pin update: {e}")))?;
    value["schema"] = serde_json::Value::String(target.as_display());
    let new_bytes = serde_json::to_vec_pretty(&value)
        .map_err(|e| EngineError::Mem(format!("serialize mem config for pin update: {e}")))?;
    backend
        .write_mem_config(&new_bytes, ctx)
        .map_err(|e| EngineError::Mem(format!("write mem config for pin update: {e}")))?;
    Ok(Some(value))
}
