//! The mem setting verbs: version, description, title, subject, internal and sync state, each carrying its own read-only refusal.

use super::*;

impl Engine {
    pub fn set_mem_version(
        &mut self,
        mem_name: &str,
        new_version: semver::Version,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemVersionOutcome, EngineError> {
        // Resolve the mount up-front so an unknown-mem name refuses
        // before any drift-probe side effect lands.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        // Probe for concurrent-drift before the write — a sibling
        // engine that committed between our last snapshot and now
        // surfaces `MEM_RELOADED` on the response so callers see
        // the drift without a separate read round-trip. Drift
        // warnings ride alongside the success outcome; an
        // unreachable-backend probe collapses to no warnings (the
        // existing accessor warn-logs internally and skips).
        let mut warnings = self.reload_if_stale(Some(mem_name));
        // Provenance nudge — same posture as every other commit-
        // producing mutation: when `require_notes` is set and no note
        // was supplied, ride a non-blocking `NOTE_MISSING` warning.
        // The version bump still commits.
        if let Some(w) = self.note_missing_warning("set_mem_version", note) {
            warnings.push(w);
        }

        // The old value is read from the STORED config, not the cache: the
        // cache may be days stale, and reporting a version the file has not
        // held since boot would be its own small lie.
        let old_version = self.mounts[mount_idx]
            .mem_config
            .as_ref()
            .and_then(|c| c.version.clone());
        let target = new_version.clone();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_version",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.version = Some(target.clone());
            },
        )?;
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemVersionOutcome {
            mem: mem_name.to_string(),
            old_version,
            new_version,
            warnings,
        })
    }

    /// Update a mem's `description` field in its per-mem config and
    /// persist it through the backend — the one-line text mem-archive
    /// export embeds and the registry card surfaces. `None` clears the
    /// field. Same backend symmetry, drift probe, and provenance-note
    /// posture as [`Self::set_mem_version`]; archive mounts reject with
    /// `BackendError::Sealed`.
    pub fn set_mem_description(
        &mut self,
        mem_name: &str,
        new_description: Option<String>,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemDescriptionOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_mem_description", note) {
            warnings.push(w);
        }

        let old_description = self.mounts[mount_idx]
            .mem_config
            .as_ref()
            .and_then(|c| c.description.clone());
        let target = new_description.clone();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_description",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.description = target.clone();
            },
        )?;
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemDescriptionOutcome {
            mem: mem_name.to_string(),
            old_description,
            new_description,
            warnings,
        })
    }

    /// Update a mem's display `title` — free text, NOT identity: the
    /// mem name stays the sole handle everywhere. `None` clears it.
    /// Mirrors [`Self::set_mem_description`] in backend symmetry,
    /// drift probe, and provenance-note posture.
    pub fn set_mem_title(
        &mut self,
        mem_name: &str,
        new_title: Option<String>,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemTitleOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_mem_title", note) {
            warnings.push(w);
        }

        let old_title = self.mounts[mount_idx]
            .mem_config
            .as_ref()
            .and_then(|c| c.title.clone());
        let target = new_title.clone();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_title",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.title = target.clone();
            },
        )?;
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemTitleOutcome {
            mem: mem_name.to_string(),
            old_title,
            new_title,
            warnings,
        })
    }

    /// Update a mem's `subject` block — scope, method, deliberate
    /// exclusions, published verbatim. `None` clears the block AS A
    /// UNIT. Mirrors [`Self::set_mem_description`].
    pub fn set_mem_subject(
        &mut self,
        mem_name: &str,
        new_subject: Option<memstead_schema::MemSubject>,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemSubjectOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_mem_subject", note) {
            warnings.push(w);
        }

        let old_subject = self.mounts[mount_idx]
            .mem_config
            .as_ref()
            .and_then(|c| c.subject.clone());
        let target = new_subject.clone();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_subject",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.subject = target.clone();
            },
        )?;
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemSubjectOutcome {
            mem: mem_name.to_string(),
            old_subject,
            new_subject,
            warnings,
        })
    }

    /// Mark (or unmark) a mem as **internal** — hidden from the default
    /// `memstead_overview` roster and public projections, while remaining a
    /// real, schema-validated, diffable mem (inspectable when explicitly
    /// scoped by name, and deletable). The ingest process-state redesign
    /// (candidate (b)) flags each `ingest/<name>` process mem this way so it
    /// does not clutter the roster alongside real content.
    ///
    /// Stored as the top-level `internal` config field (captured by the
    /// flattened `extra` map). Backend-symmetric like
    /// [`Self::set_mem_description`]; `EngineError::UnknownMem` /
    /// `ReadOnlyMount` / `InvalidInput` on the usual failures.
    pub fn set_mem_internal(
        &mut self,
        mem_name: &str,
        internal: bool,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemInternalOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        let _ = self.reload_if_stale(Some(mem_name));

        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_internal",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                if internal {
                    c.extra
                        .insert("internal".to_string(), serde_json::Value::Bool(true));
                } else {
                    c.extra.remove("internal");
                }
            },
        )?;
        // This one used to return a bare `bool` and so had nowhere to put the
        // intervention report: it was dropped, not even logged.
        // A writer whose signature
        // cannot carry a warning is a writer that silently will not.
        let mut warnings = Vec::new();
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemInternalOutcome {
            mem: mem_name.to_string(),
            internal,
            warnings,
        })
    }

    /// Set (or clear) one opaque sync-state token in a mem's per-mem
    /// config and persist it through the backend. The ingest layer calls
    /// this after a successful pass over a source's changed slice to
    /// record "the source state the graph was last synced against".
    ///
    /// `key` and `token` are both opaque to the engine: the key is
    /// conventionally `"<ingest>/<facet>"` but the engine treats it as an
    /// arbitrary string; the token's meaning belongs to the medium-type
    /// layer (git → commit id, graph → snapshot token, filesystem → a
    /// JSON-stringified stat digest). The engine never parses either.
    /// An **empty** `token` removes the key — the surface for clearing a
    /// baseline (which the next ingest pass re-seeds at the current
    /// source state).
    ///
    /// Backend-symmetric like [`Self::set_mem_version`]: folder backends
    /// rewrite `.memstead/config.json`; git-branch backends commit
    /// `__MEMSTEAD:mems/<mem>/config.json`. Archive mounts reject with
    /// `BackendError::Sealed`.
    ///
    /// Returns the (mem, key, previous-token) triple so callers can
    /// surface the change without an extra read. `EngineError::UnknownMem`
    /// when the name resolves to no mount; `EngineError::ReadOnlyMount`
    /// when the mount is sealed for writes; `EngineError::InvalidInput`
    /// when the mount has no loaded `MemConfig`.
    pub fn set_mem_sync_state(
        &mut self,
        mem_name: &str,
        key: &str,
        token: &str,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemSyncStateOutcome, EngineError> {
        // Resolve the mount up-front so an unknown-mem name refuses
        // before any drift-probe side effect lands.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        // Probe for concurrent-drift before the write — same posture as
        // every other commit-producing mutation; a sibling engine that
        // committed since our last snapshot surfaces `MEM_RELOADED`.
        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_mem_sync_state", note) {
            warnings.push(w);
        }

        // `previous` has to come from the config this write actually lands
        // on, not from the cache: a sibling ingest pass may have moved the
        // very token this call is replacing, and reporting the cached value
        // would name a baseline that has not been current since boot. The
        // cell is written by each `apply` pass, so after a compare-and-set
        // retry it holds what was really overwritten.
        let seen: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let key_owned = key.to_string();
        let token_owned = token.to_string();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_sync_state",
            note,
            &|c: &mut memstead_schema::config::MemConfig| {
                *seen.borrow_mut() = if token_owned.is_empty() {
                    c.sync_state.remove(&key_owned)
                } else {
                    c.sync_state.insert(key_owned.clone(), token_owned.clone())
                };
            },
        )?;
        let previous = seen.into_inner();
        // `removed` distinguishes a no-op clear (key absent) from a real one
        // so the outcome is honest.
        let removed = token.is_empty() && previous.is_some();
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemSyncStateOutcome {
            mem: mem_name.to_string(),
            key: key.to_string(),
            previous,
            removed,
            warnings,
        })
    }
}
