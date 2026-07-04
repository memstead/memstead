//! Drift detection and per-mem change synthesis.
//!
//! `reload_if_stale` probes each candidate mount's `current_head()`
//! cursor on every operation (no throttle), reloads mems whose
//! on-disk state has advanced past the engine's cached head, and
//! surfaces `MemReloaded` warnings for handlers that need to
//! re-derive conclusions from a now-reloaded snapshot. `changes_since`
//! produces
//! the per-entity diff between a stored cursor and the backend's
//! current state — folder mounts synthesise from the changelog, the
//! git-branch hook walks the tree with rename detection, archive
//! mounts return empty.

use crate::backend::BackendError;
use crate::workspace::MountStorage;

use super::mutation::lookup_title_and_type;
use super::{Engine, EngineError};

impl Engine {
    /// Reload-before-operation: before any read or write executes,
    /// check the mem ref; if it advanced past the engine's cached
    /// `last_known_head`, reload the affected mem(s) and return one
    /// [`WarningHint::MemReloaded`] per reload so the caller can
    /// surface the drift to the agent (the response *itself* already
    /// carries fresh content — the warning explains why state
    /// shifted).
    ///
    /// The ref check runs on **every** call — there is no throttle
    /// window. A per-operation `current_head()` read is microseconds,
    /// effectively free at LLM latencies, and a throttle that let an
    /// operation execute against an already-moved ref would reintroduce
    /// the exact silent-staleness this guards against. This is the
    /// correctness floor: no operation acts on a projection that is
    /// behind git truth.
    ///
    /// `mem = Some(name)` scopes the probe to one mount; `None`
    /// scans every mount. Read handlers that target a known mem
    /// (`memstead_entity` derives the mem from the id;
    /// `memstead_changes_since` takes it as a param) and every mutation
    /// (which knows its target mem) pass a name; tools that scan
    /// multi-mem (`memstead_search` without a mem filter,
    /// `memstead_overview`, `memstead_health`) pass `None`.
    ///
    /// Behaviour matrix per mount:
    /// - cached `Some(old)` + on-disk `Some(new)`, `old != new` →
    ///   reload the mem, emit `MemReloaded`, refresh the cached
    ///   head to `new`.
    /// - cached `None` + on-disk `Some(new)` → silently capture the
    ///   first observed head as the baseline (no warning — there's
    ///   no prior in-memory snapshot to be stale against).
    /// - cached / on-disk match, on-disk `None` (folder, archive,
    ///   refdb hiccup), or `current_head` errors → no-op.
    ///
    /// Reload errors are warn-logged and the affected mem is
    /// skipped — the caller's response is still served from the (now
    /// stale) in-memory snapshot rather than failing the entire
    /// request. The next operation retries.
    ///
    /// Cache invalidation rides on `reload_one_mem` — community
    /// and search-index memos drop when any mem reloads.
    pub fn reload_if_stale(&mut self, mem: Option<&str>) -> Vec<crate::ops::WarningHint> {
        // Phase 1 — pick candidate mem names that match the filter.
        // Cloned so the immutable borrow doesn't survive into the
        // mutation phase.
        let candidates: Vec<String> = self
            .mounts
            .iter()
            .filter(|m| mem.is_none_or(|v| m.mount.mem == v))
            .map(|m| m.mount.mem.clone())
            .collect();

        if candidates.is_empty() {
            return Vec::new();
        }

        // Phase 2 — probe every candidate's current head via the
        // backend. Errors collapse to None so a transient backend
        // hiccup doesn't surface as a warning; the next operation
        // retries.
        let probes: Vec<(String, Option<String>, Option<String>)> = candidates
            .iter()
            .filter_map(|name| {
                let m = self.mounts.iter().find(|m| &m.mount.mem == name)?;
                let new_head = m.backend.current_head().ok().flatten();
                let cached = m.last_known_head.clone();
                Some((name.clone(), cached, new_head))
            })
            .collect();

        // Phase 3 — act on each probe. The drift case is the only
        // one that calls `reload_one_mem`; every other arm just
        // (for first-observation) captures the baseline head silently.
        let mut warnings = Vec::new();
        for (name, cached, new_head) in probes {
            match (cached, new_head.clone()) {
                (Some(old), Some(new)) if old != new => {
                    match self.reload_one_mem(&name) {
                        Ok(report) => {
                            warnings.push(crate::ops::WarningHint::MemReloaded {
                                mem: name.clone(),
                                old_head: old.clone(),
                                new_head: new.clone(),
                                entities_loaded: report.added.len() + report.changed.len(),
                            });
                            if let Some(state) =
                                self.mounts.iter_mut().find(|m| m.mount.mem == name)
                            {
                                state.last_known_head = Some(new.clone());
                            }
                            // Build the structured notice now — the
                            // backend's current head equals `new` and no
                            // follow-on write in this operation has
                            // committed yet, so the `old → new` delta
                            // describes only the sibling's change. Stashed
                            // for the response layer to drain.
                            let notice = self.mem_changed_notice(&name, &old, &new);
                            self.pending_mem_changed.push(notice);
                        }
                        Err(e) => {
                            tracing::warn!(
                                mem = %name,
                                error = %e,
                                "drift-detected reload_one_mem failed; serving \
                                 stale snapshot — will retry on the next operation"
                            );
                        }
                    }
                }
                _ => {
                    if let Some(state) = self.mounts.iter_mut().find(|m| m.mount.mem == name)
                        && state.last_known_head.is_none()
                    {
                        state.last_known_head = new_head;
                    }
                }
            }
        }

        warnings
    }

    /// Mark `mount_idx`'s on-disk head as advanced by *this* engine's
    /// own write so the next `reload_if_stale` doesn't surface
    /// `MEM_RELOADED` for the commit we just produced. Mutation
    /// paths call this immediately after `backend.commit` returns —
    /// the cached `last_known_head` jumps straight to the new SHA
    /// without going through a reload. Because every mutation runs
    /// `reload_if_stale` for its target mem *before* committing,
    /// the cached `last_known_head` is current at commit time, so
    /// this advance is over a verified parent — it can never jump
    /// the cache past an unobserved sibling commit. Cross-session and
    /// out-of-band advances (sibling engine, manual `git pull`) that
    /// land before the next operation still mismatch the cached value
    /// and fire the warning as before.
    ///
    /// Empty SHA is a no-op (no commit landed — e.g. duplicate-add
    /// relate). Backends that don't track a head (folder, archive)
    /// leave `last_known_head` at `None` and still no-op via the
    /// drift-check's `cached: None` branch.
    pub(crate) fn record_self_write(&mut self, mount_idx: usize, commit_sha: &str) {
        if commit_sha.is_empty() {
            return;
        }
        // Capture the pre-write head + mem name so the
        // `MemChangedEvent` we emit reflects the transition the
        // current commit produced. We do this before mutating
        // `last_known_head` because that field is the previous SHA
        // from the event's point of view.
        let (mem, previous) = match self.mounts.get(mount_idx) {
            Some(state) => (
                state.mount.mem.clone(),
                state.last_known_head.clone().unwrap_or_default(),
            ),
            None => return,
        };
        if let Some(state) = self.mounts.get_mut(mount_idx) {
            state.last_known_head = Some(commit_sha.to_string());
        }
        // Skip emit when no SHA actually advanced — folder backends
        // (and archive backends) carry `last_known_head: None` and
        // pass `commit_sha = ""` in some paths; the early-return at
        // the top already catches the explicit empty case, but
        // `previous == commit_sha` covers idempotent re-writes that
        // pass through the same write path (e.g. a relate that
        // re-applies the same edge). Skipping keeps the event stream
        // a stream of *changes* rather than a stream of *writes*.
        if previous == commit_sha {
            return;
        }
        let event = crate::engine::events::MemChangedEvent {
            mem,
            head: commit_sha.to_string(),
            previous,
            n_commits: 1,
        };
        self.emit_mem_changed(&event);
    }

    /// Drain the reload-before-operation notices accumulated since the
    /// last drain. The response layer calls this after an operation
    /// completes to attach the structured `mem_changed` notice. Every
    /// handler that can trigger a reload (directly via
    /// [`Self::reload_if_stale`] or indirectly through a mutation) must
    /// drain, or an undrained notice leaks into the next operation's
    /// response.
    pub fn take_mem_changed_notices(&mut self) -> Vec<crate::ops::MemChangedNotice> {
        std::mem::take(&mut self.pending_mem_changed)
    }

    /// Build a [`crate::ops::MemChangedNotice`] describing the
    /// per-entity delta a reload applied to `mem` (from `from_head`
    /// to `to_head`). Derived from [`Self::changes_since`] so it
    /// carries rename detection on git-branch mounts; on any backend
    /// error (e.g. an unresolvable cursor) it falls back to an empty
    /// delta — the heads alone still tell the agent the mem moved.
    ///
    /// Callers pair this with [`Self::reload_if_stale`]: a returned
    /// [`crate::ops::WarningHint::MemReloaded`] carries the
    /// `old_head` / `new_head` to pass here. The delta matches the
    /// transition the reload applied (`changes_since` walks the same
    /// `from_head → current` range).
    pub fn mem_changed_notice(
        &self,
        mem: &str,
        from_head: &str,
        to_head: &str,
    ) -> crate::ops::MemChangedNotice {
        let changes = self
            .changes_since(mem, from_head, None)
            .map(|r| r.changes)
            .unwrap_or_default();
        crate::ops::MemChangedNotice::from_delta(
            mem.to_string(),
            from_head.to_string(),
            to_head.to_string(),
            changes,
        )
    }

    /// Per-entity events for `mem` between `since` and the backend's
    /// current state.
    ///
    /// 1. Resolves the mount (returns [`EngineError::UnknownMem`]
    ///    on unknown mem).
    /// 2. Validates `rename_similarity` against
    ///    `[RENAME_SIMILARITY_MIN, RENAME_SIMILARITY_MAX]`. Out-of-range
    ///    values refuse with [`EngineError::InvalidInput`] carrying
    ///    `details.allowed_range` and `details.requested`. `None` falls
    ///    back to [`crate::ops::RENAME_SIMILARITY_DEFAULT`].
    /// 3. Dispatches on the mount's `MountStorage`:
    ///    - Folder mounts synthesize from the JSONL changelog via
    ///      [`crate::ops::folder_changes_since`].
    ///    - Git-branch mounts call the registered
    ///      [`GitBranchOps::changes_since`] hook (real tree-diff with
    ///      rename detection); missing hook = full flavour not loaded
    ///      and the report comes back empty.
    ///    - Archive mounts return an empty report.
    /// 4. Enriches each envelope's `title` / `entity_type` from the
    ///    in-memory store (best-effort — `Removed` envelopes always
    ///    leave both `None`; missing-from-store entities also leave
    ///    them `None`).
    /// 5. Returns [`crate::ops::ChangesReport`] with `mem`,
    ///    `since` (echoed), `head` (backend-resolved current
    ///    cursor), enriched `changes`, and any clamping warnings.
    pub fn changes_since(
        &self,
        mem: &str,
        since: &str,
        rename_similarity: Option<f32>,
    ) -> Result<crate::ops::ChangesReport, EngineError> {
        let m = self.find_mount(mem)?;

        // Reject out-of-range `rename_similarity` early so CLI and MCP
        // share one refusal surface.
        // The prior clamp+warn shape silently accepted nonsense values
        // (e.g. 1.5 ≡ 1.0); typed refusal gives the agent a recoverable
        // signal.
        if let Some(v) = rename_similarity
            && !(crate::ops::RENAME_SIMILARITY_MIN..=crate::ops::RENAME_SIMILARITY_MAX).contains(&v)
        {
            return Err(EngineError::RenameSimilarityOutOfRange {
                requested: v,
                allowed_min: crate::ops::RENAME_SIMILARITY_MIN,
                allowed_max: crate::ops::RENAME_SIMILARITY_MAX,
            });
        }
        let clamped = rename_similarity.unwrap_or(crate::ops::RENAME_SIMILARITY_DEFAULT);

        let backend_changes = match &m.mount.storage {
            MountStorage::Folder { path } => {
                crate::ops::folder_changes_since(path, mem, since).map_err(EngineError::Backend)?
            }
            MountStorage::GitBranch { gitdir, branch } => match self.git_branch_ops.as_ref() {
                Some(hook) => match (hook.changes_since)(gitdir, branch, mem, since, clamped) {
                    Ok(c) => c,
                    // Lift the backend's typed bad-`since` marker to a typed
                    // engine error carrying the untruncated SHA, parallel
                    // to the UNKNOWN_REMOTE / LOCAL_DIVERGENCE prefixes.
                    Err(BackendError::Other(msg)) if msg.starts_with("COMMIT_NOT_FOUND:") => {
                        let since = msg
                            .strip_prefix("COMMIT_NOT_FOUND:")
                            .unwrap_or_default()
                            .to_string();
                        return Err(EngineError::InvalidChangesCursor {
                            mem: mem.to_string(),
                            since,
                        });
                    }
                    Err(e) => return Err(EngineError::Backend(e)),
                },
                None => crate::ops::BackendChanges::empty_at(since),
            },
            // Archive is sealed; the in-memory backend keeps a
            // provenance log but no cursor-addressable change history,
            // so both yield no backend-derived changes here (the live
            // playground stream rides the engine's event broadcast, not
            // this path).
            MountStorage::Archive { .. } | MountStorage::InMemory => {
                crate::ops::BackendChanges::empty_at(since)
            }
        };

        // Enrich each id-only envelope from the engine's store.
        // `Removed` always leaves title / entity_type None — the
        // entity is gone by definition; the post-reload store does
        // not have it. Other variants populate when the lookup
        // succeeds; missing ids stay None.
        let enriched: Vec<crate::ops::ChangeEnvelope> = backend_changes
            .changes
            .into_iter()
            .map(|env| match env {
                crate::ops::ChangeEnvelope::Added { id, .. } => {
                    let (title, entity_type) = lookup_title_and_type(&self.store, &id);
                    crate::ops::ChangeEnvelope::Added {
                        id,
                        title,
                        entity_type,
                    }
                }
                crate::ops::ChangeEnvelope::Updated { id, .. } => {
                    let (title, entity_type) = lookup_title_and_type(&self.store, &id);
                    crate::ops::ChangeEnvelope::Updated {
                        id,
                        title,
                        entity_type,
                    }
                }
                crate::ops::ChangeEnvelope::Removed { id, .. } => {
                    crate::ops::ChangeEnvelope::Removed {
                        id,
                        title: None,
                        entity_type: None,
                    }
                }
                crate::ops::ChangeEnvelope::Renamed { from_id, to_id, .. } => {
                    let (title, entity_type) = lookup_title_and_type(&self.store, &to_id);
                    crate::ops::ChangeEnvelope::Renamed {
                        from_id,
                        to_id,
                        title,
                        entity_type,
                    }
                }
            })
            .collect();

        // Out-of-range `rename_similarity` is now a hard refusal (see
        // early-return above); the response carries no clamping warning.
        let warnings: Vec<crate::ops::WarningHint> = Vec::new();

        // The backend populates
        // notes + memstead_ref on every git-branch call (folder + archive
        // backends leave them empty / None). Surface them
        // unconditionally; the MCP `include_notes` parameter becomes
        // a renderer-side filter rather than a separate engine call.
        let notes = if backend_changes.notes.is_empty() && backend_changes.memstead_ref.is_none() {
            None
        } else {
            Some(backend_changes.notes)
        };
        Ok(crate::ops::ChangesReport {
            mem: mem.to_string(),
            since: backend_changes.since,
            head: backend_changes.head,
            changes: enriched,
            warnings,
            notes,
            memstead_ref: backend_changes.memstead_ref,
        })
    }

    /// Fetch updates from `remote` into the workspace's mem-repo.
    /// Advances remote-tracking refs only; the local branch pointer
    /// is not moved.
    ///
    /// `refspecs` is forwarded verbatim to `git fetch`. An empty list
    /// uses the remote's configured defaults.
    ///
    /// Refusal codes: `UNKNOWN_MEM`, `UNKNOWN_REMOTE`,
    /// `INVALID_INPUT` (folder / archive mounts).
    ///
    /// V1 atomicity: schema-validation quarantine for
    /// fetched commits is not yet wired. The remote-tracking refs
    /// advance unconditionally on a successful fetch; downstream
    /// schema validation runs on read via the engine's existing
    /// reload pipeline.
    pub fn fetch(
        &self,
        mem: &str,
        remote: &str,
        refspecs: &[String],
    ) -> Result<crate::ops::FetchOutcome, EngineError> {
        let m = self.find_mount(mem)?;
        match &m.mount.storage {
            MountStorage::Folder { .. } | MountStorage::Archive { .. } | MountStorage::InMemory => {
                Err(EngineError::InvalidInput(format!(
                    "mem '{mem}' is not git-backed — `memstead_fetch` requires a git-branch mount",
                )))
            }
            MountStorage::GitBranch { gitdir, .. } => match self.git_branch_ops.as_ref() {
                Some(hook) => (hook.fetch)(gitdir, remote, refspecs).map_err(|e| match e {
                    BackendError::Other(msg) if msg.starts_with("UNKNOWN_REMOTE:") => {
                        EngineError::UnknownRemote(
                            msg.trim_start_matches("UNKNOWN_REMOTE:").trim().to_string(),
                        )
                    }
                    other => EngineError::Backend(other),
                }),
                None => Err(EngineError::Backend(BackendError::Other(
                    "git-branch fetch hook not installed (full flavour not loaded)".to_string(),
                ))),
            },
        }
    }

    /// Pull updates from `remote` into the named mem's branch.
    /// Fetches into the remote-tracking ref, runs a pre-merge schema
    /// validation pass against the prospective state, then
    /// fast-forwards the local branch. Refuses with
    /// `LOCAL_DIVERGENCE` for diverged local branches and with
    /// `SCHEMA_VIOLATION_IN_FETCH` when the prospective state fails
    /// schema validation — in both refusal cases the local branch
    /// pointer is untouched (the underlying fetch has updated
    /// `refs/remotes/*` but the engine has not promoted the new
    /// state).
    pub fn pull(
        &mut self,
        mem: &str,
        remote: &str,
    ) -> Result<crate::ops::PullOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| EngineError::UnknownMem(mem.to_string()))?;

        // Run the fetch step alone first so we can validate the
        // prospective state against the schema before letting the
        // pull's fast-forward land. Errors map to the typed surface
        // just like a standalone `memstead_fetch` call.
        let gitdir = match &self.mounts[mount_idx].mount.storage {
            MountStorage::Folder { .. } | MountStorage::Archive { .. } | MountStorage::InMemory => {
                return Err(EngineError::InvalidInput(format!(
                    "mem '{mem}' is not git-backed — `memstead_pull` requires a git-branch mount",
                )));
            }
            MountStorage::GitBranch { gitdir, .. } => gitdir.clone(),
        };
        let hook = self.git_branch_ops.ok_or_else(|| {
            EngineError::Backend(BackendError::Other(
                "git-branch pull hook not installed (full flavour not loaded)".to_string(),
            ))
        })?;
        (hook.fetch)(&gitdir, remote, &[]).map_err(|e| match e {
            BackendError::Other(msg) if msg.starts_with("UNKNOWN_REMOTE:") => {
                EngineError::UnknownRemote(
                    msg.trim_start_matches("UNKNOWN_REMOTE:").trim().to_string(),
                )
            }
            other => EngineError::Backend(other),
        })?;

        // Pre-merge schema validation. The remote-tracking ref now
        // points at the fetched tip; we walk it, parse every `.md`
        // blob against the mem's pinned schema, and refuse the
        // pull if any parse fails. The local branch pointer is still
        // unchanged at this point — the refusal is fully atomic.
        let remote_ref = format!("refs/remotes/{remote}/{mem}");
        self.validate_ref_against_schema(&hook, &gitdir, mem, &remote_ref)?;

        // Run the underlying pull (re-runs the fetch via git CLI, but
        // that's a no-op cache-wise and keeps the fast-forward logic
        // co-located with the rest of the transport implementation).
        let outcome = (hook.pull)(&gitdir, remote, mem).map_err(|e| match e {
            BackendError::Other(msg) if msg.starts_with("UNKNOWN_REMOTE:") => {
                EngineError::UnknownRemote(
                    msg.trim_start_matches("UNKNOWN_REMOTE:").trim().to_string(),
                )
            }
            BackendError::Other(msg) if msg.starts_with("LOCAL_DIVERGENCE:") => {
                let payload = msg.trim_start_matches("LOCAL_DIVERGENCE:");
                let mut parts = payload.splitn(2, ':');
                let v = parts.next().unwrap_or(mem).to_string();
                let remote_ref = parts.next().unwrap_or("refs/remotes/?/?").to_string();
                EngineError::LocalDivergence { mem: v, remote_ref }
            }
            other => EngineError::Backend(other),
        })?;

        // Rewind cached head + emit change event.
        if outcome.previous_sha != outcome.new_sha {
            if let Some(state) = self.mounts.get_mut(mount_idx) {
                state.last_known_head = Some(outcome.new_sha.clone());
            }
            let event = crate::engine::events::MemChangedEvent {
                mem: mem.to_string(),
                head: outcome.new_sha.clone(),
                previous: outcome.previous_sha.clone(),
                n_commits: 1,
            };
            self.emit_mem_changed(&event);
        }
        Ok(outcome)
    }

    /// Push the named mem's branch to `remote`. Runs a pre-push
    /// schema validation pass against the local branch tree; refuses
    /// with `LOCAL_INVALID_STATE` when the local state fails schema
    /// validation (the remote is not contacted in that case). Refuses
    /// with `NON_FAST_FORWARD` when the push is not a fast-forward
    /// and `force: false`; with `force: true` runs a
    /// `--force-with-lease` push instead.
    pub fn push(
        &self,
        mem: &str,
        remote: &str,
        force: bool,
    ) -> Result<crate::ops::PushOutcome, EngineError> {
        let m = self.find_mount(mem)?;
        let gitdir = match &m.mount.storage {
            MountStorage::Folder { .. } | MountStorage::Archive { .. } | MountStorage::InMemory => {
                return Err(EngineError::InvalidInput(format!(
                    "mem '{mem}' is not git-backed — `memstead_push` requires a git-branch mount",
                )));
            }
            MountStorage::GitBranch { gitdir, .. } => gitdir.clone(),
        };
        let hook = self.git_branch_ops.ok_or_else(|| {
            EngineError::Backend(BackendError::Other(
                "git-branch push hook not installed (full flavour not loaded)".to_string(),
            ))
        })?;

        // Pre-push schema validation: walk the local branch tree, run
        // the mem's pinned schema over every `.md` blob. Any parse
        // failure refuses the push with `LOCAL_INVALID_STATE` — the
        // remote is not contacted.
        let local_ref = format!("refs/heads/{mem}");
        if let Err(EngineError::SchemaViolationInFetch { violations, .. }) =
            self.validate_ref_against_schema(&hook, &gitdir, mem, &local_ref)
        {
            return Err(EngineError::LocalInvalidState {
                mem: mem.to_string(),
                remote: remote.to_string(),
                detail: format!(
                    "{} violation(s) in local branch: {}",
                    violations.len(),
                    violations.join("; "),
                ),
            });
        }

        (hook.push)(&gitdir, remote, mem, force).map_err(|e| match e {
            BackendError::Other(msg) if msg.starts_with("UNKNOWN_REMOTE:") => {
                EngineError::UnknownRemote(
                    msg.trim_start_matches("UNKNOWN_REMOTE:").trim().to_string(),
                )
            }
            BackendError::Other(msg) if msg.starts_with("NON_FAST_FORWARD:") => {
                let payload = msg.trim_start_matches("NON_FAST_FORWARD:");
                let mut parts = payload.splitn(2, ':');
                let v = parts.next().unwrap_or(mem).to_string();
                let r = parts.next().unwrap_or(remote).to_string();
                EngineError::NonFastForward { mem: v, remote: r }
            }
            BackendError::Other(msg) if msg.starts_with("UNKNOWN_REF:") => {
                EngineError::UnknownRef(msg.trim_start_matches("UNKNOWN_REF:").trim().to_string())
            }
            other => EngineError::Backend(other),
        })
    }

    /// Configure (or re-point) a named remote on the workspace's
    /// mem-repo, so `fetch` / `pull` / `push` have somewhere to go.
    /// Upsert semantics — safe to re-run with a new URL. The mem-repo
    /// is shared by every git-branch mount, so the op is
    /// workspace-level: any git-branch mount locates it; refuses
    /// `INVALID_INPUT` when the workspace has none.
    pub fn remote_add(
        &self,
        name: &str,
        url: &str,
    ) -> Result<crate::ops::RemoteAddOutcome, EngineError> {
        // Both values become git subprocess arguments — refuse shapes
        // that would parse as flags.
        if name.is_empty() || name.starts_with('-') || url.is_empty() || url.starts_with('-') {
            return Err(EngineError::InvalidInput(format!(
                "remote name and url must be non-empty and must not start with '-' \
                 (got name '{name}', url '{url}')",
            )));
        }
        let gitdir = self
            .mounts
            .iter()
            .find_map(|m| match &m.mount.storage {
                MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                EngineError::InvalidInput(
                    "no git-branch mounts — `remote-add` requires a mem-repo workspace".to_string(),
                )
            })?;
        let hook = self.git_branch_ops.ok_or_else(|| {
            EngineError::Backend(BackendError::Other(
                "git-branch remote_add hook not installed (full flavour not loaded)".to_string(),
            ))
        })?;
        (hook.remote_add)(&gitdir, name, url).map_err(EngineError::Backend)
    }

    /// Pre-merge schema validation pass: walks every `.md` blob at
    /// `ref_name` and runs `parse_entries` with the mem's pinned
    /// schema. Returns `Ok(())` when the tree is schema-clean;
    /// returns `EngineError::SchemaViolationInFetch` with the list of
    /// per-entity violation messages otherwise. The validation is
    /// strict on parse-time errors — any `(path, error)` pair from
    /// `parse_entries` triggers a refusal.
    ///
    /// `ref_name` is the prospective state (a `refs/remotes/*` ref
    /// for pull, `refs/heads/*` for push). The engine layer maps the
    /// returned error into the surface code it needs
    /// (`SCHEMA_VIOLATION_IN_FETCH` for pull, `LOCAL_INVALID_STATE`
    /// for push).
    fn validate_ref_against_schema(
        &self,
        hook: &crate::engine::GitBranchOps,
        gitdir: &std::path::Path,
        mem: &str,
        ref_name: &str,
    ) -> Result<(), EngineError> {
        let schema = self
            .schemas
            .get(mem)
            .ok_or_else(|| EngineError::SchemaNotFound {
                mem: mem.to_string(),
                pin: "<missing engine-side resolution>".to_string(),
                // Internal invariant breach (an already-resolved schema
                // absent from the per-mem map), not a source-resolution
                // failure — no per-source diagnostics apply.
                sources: Vec::new(),
            })?
            .clone();

        let blobs = (hook.read_tree)(gitdir, ref_name).map_err(|e| match e {
            BackendError::Other(msg) if msg.starts_with("UNKNOWN_REF:") => {
                EngineError::UnknownRef(msg.trim_start_matches("UNKNOWN_REF:").trim().to_string())
            }
            other => EngineError::Backend(other),
        })?;

        let mut source_entries: Vec<crate::entity::source::SourceEntry> = Vec::new();
        for (rel_path, content) in blobs {
            source_entries.push(crate::entity::source::SourceEntry {
                relative_path: rel_path.clone(),
                source_path: std::path::PathBuf::from(rel_path),
                content,
            });
        }

        // First pass: permissive parse via the engine's loader so we
        // can build Entity values for the strict validator. The
        // loader silently absorbs frontmatter / title / section
        // drift; the strict pass below is what catches it.
        let load_result = crate::entity::loader::parse_entries(
            source_entries.clone(),
            Vec::new(),
            mem,
            schema.as_ref(),
        );
        let mut violations: Vec<String> = load_result
            .errors
            .iter()
            .map(|(path, msg)| format!("{}: {msg}", path.display()))
            .collect();

        // Strict per-entity validator: enforces "looks like a mem
        // entity" invariants (frontmatter shape, title presence,
        // required sections, unknown sections, relationship syntax,
        // wiki-link shape) that the permissive loader doesn't refuse.
        // Re-runs against the same source bytes so unparseable
        // frontmatter surfaces here even when the loader's tolerant
        // path produces an Entity stub.
        let entities_by_path: std::collections::HashMap<String, &crate::entity::Entity> =
            load_result
                .entities
                .iter()
                .map(|p| (p.entity.file_path.clone(), &p.entity))
                .collect();
        for source in &source_entries {
            let Some(entity) = entities_by_path.get(&source.relative_path) else {
                continue;
            };
            let type_def = match schema.get_type(&entity.entity_type) {
                Some(t) => t,
                None => {
                    violations.push(format!(
                        "{}: unknown entity_type '{}' in schema",
                        source.relative_path, entity.entity_type,
                    ));
                    continue;
                }
            };
            if let Err(e) = crate::validator::strict::validate_strict(
                &source.content,
                entity,
                type_def.as_ref(),
                &source.relative_path,
            ) {
                violations.push(format!("{}: {e}", source.relative_path));
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(EngineError::SchemaViolationInFetch {
                mem: mem.to_string(),
                ref_name: ref_name.to_string(),
                violations,
            })
        }
    }

    /// Reset a mem's branch pointer to `target_sha`. The only
    /// engine surface that moves a branch pointer over existing
    /// commits — every other mutation appends. Refuses if any commit
    /// that would be discarded by the reset is already reachable from
    /// a `refs/remotes/*` ref (the engine's definition of "pushed").
    ///
    /// `target_sha` accepts anything `gix::rev_parse_single` admits:
    /// a SHA, an abbreviated SHA, a branch name, a tag. The branch
    /// itself (`refs/heads/<mem>`) must exist.
    ///
    /// Refusal codes:
    /// - [`EngineError::UnknownMem`] (`UNKNOWN_MEM`)
    /// - [`EngineError::UnknownRef`] (`UNKNOWN_REF`) — branch or
    ///   target ref does not resolve.
    /// - [`EngineError::PushedCommitsProtected`]
    ///   (`PUSHED_COMMITS_PROTECTED`) — at least one discarded commit
    ///   is pushed. The error carries the offending SHAs verbatim.
    /// - [`EngineError::InvalidInput`] (`INVALID_INPUT`) — mem is
    ///   folder / archive-backed (history rewriting only makes sense
    ///   for git-branch mounts).
    ///
    /// Emits a [`crate::engine::events::MemChangedEvent`] on
    /// success when the SHA actually changed; the reset's effect is
    /// observable through the same change-event surface every commit
    /// flows through. Engine's cached `last_known_head` for the
    /// affected mount is rewound to the new SHA so the next drift
    /// probe doesn't flag the reset as a sibling-writer surprise.
    pub fn branch_reset(
        &mut self,
        mem: &str,
        target_sha: &str,
    ) -> Result<crate::ops::BranchResetOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| EngineError::UnknownMem(mem.to_string()))?;

        let outcome = match &self.mounts[mount_idx].mount.storage {
            MountStorage::Folder { .. } | MountStorage::Archive { .. } | MountStorage::InMemory => {
                return Err(EngineError::InvalidInput(format!(
                    "mem '{mem}' is not git-backed — `memstead_branch_reset` requires a git-branch mount",
                )));
            }
            MountStorage::GitBranch { gitdir, branch } => match self.git_branch_ops.as_ref() {
                Some(hook) => {
                    (hook.branch_reset)(gitdir, branch, target_sha).map_err(|e| match e {
                        BackendError::Other(msg) if msg.starts_with("UNKNOWN_REF:") => {
                            let raw = msg.trim_start_matches("UNKNOWN_REF:").trim().to_string();
                            EngineError::UnknownRef(raw)
                        }
                        BackendError::Other(msg)
                            if msg.starts_with("PUSHED_COMMITS_PROTECTED:") =>
                        {
                            let payload =
                                msg.trim_start_matches("PUSHED_COMMITS_PROTECTED:").trim();
                            let pushed_shas = payload
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            EngineError::PushedCommitsProtected {
                                mem: mem.to_string(),
                                target_sha: target_sha.to_string(),
                                pushed_shas,
                            }
                        }
                        other => EngineError::Backend(other),
                    })?
                }
                None => {
                    return Err(EngineError::Backend(BackendError::Other(
                        "git-branch branch_reset hook not installed (full flavour not loaded)"
                            .to_string(),
                    )));
                }
            },
        };

        // Rewind the engine's cached HEAD so subsequent drift probes
        // don't surface MEM_RELOADED for the reset we just made.
        // Then emit a change event so subscribers see the transition
        // (skipping the no-op case where previous == new).
        if outcome.previous_sha != outcome.new_sha {
            if let Some(state) = self.mounts.get_mut(mount_idx) {
                state.last_known_head = Some(outcome.new_sha.clone());
            }
            let event = crate::engine::events::MemChangedEvent {
                mem: mem.to_string(),
                head: outcome.new_sha.clone(),
                previous: outcome.previous_sha.clone(),
                // n_commits stays at 1 for reset events. The wire
                // shape is the same `MemChangedEvent` consumers
                // already key on; semantics: "the head moved by this
                // operation". Replay-aware consumers branch on the
                // commit-vs-reset distinction by inspecting the
                // produced commit (a reset's new head is an existing
                // commit, not a freshly minted one).
                n_commits: 1,
            };
            self.emit_mem_changed(&event);
        }
        Ok(outcome)
    }

    /// Cross-mem references that a reset of `mem` to `target_sha` would
    /// strand: incoming edges from entities in *other* mems whose target
    /// exists at the current head but would not exist at the target
    /// commit — entities created after the target, or renamed to their
    /// current id after it (the reset re-materialises the old id, so
    /// references to the new id dangle either way).
    ///
    /// A read — computes against the live store and the commit history,
    /// moves nothing. The human surface calls this fresh at
    /// confirmation-dialog time and warns before `branch_reset`. Sorted
    /// (from_id, to_id, rel_type) for stable rendering.
    ///
    /// Refusals mirror `changes_since`: `UnknownMem`, `InvalidCursor`
    /// for an unresolvable `target_sha`, `InvalidInput` for
    /// non-git-backed mounts.
    pub fn branch_reset_stranded_refs(
        &self,
        mem: &str,
        target_sha: &str,
    ) -> Result<Vec<crate::ops::StrandedCrossMemRef>, EngineError> {
        use crate::ops::ChangeEnvelope;

        let report = self.changes_since(mem, target_sha, None)?;
        let mut discarded: std::collections::HashSet<String> = std::collections::HashSet::new();
        for change in &report.changes {
            match change {
                ChangeEnvelope::Added { id, .. } => {
                    discarded.insert(id.to_string());
                }
                ChangeEnvelope::Renamed { to_id, .. } => {
                    discarded.insert(to_id.to_string());
                }
                ChangeEnvelope::Updated { .. } | ChangeEnvelope::Removed { .. } => {}
            }
        }
        if discarded.is_empty() {
            return Ok(Vec::new());
        }

        let mut stranded: Vec<crate::ops::StrandedCrossMemRef> = self
            .store
            .all_entities()
            .filter(|e| e.mem != mem)
            .flat_map(|e| {
                e.relationships
                    .iter()
                    .filter(|r| discarded.contains(&r.target.to_string()))
                    .map(|r| crate::ops::StrandedCrossMemRef {
                        from_id: e.id.to_string(),
                        from_mem: e.mem.clone(),
                        to_id: r.target.to_string(),
                        rel_type: r.rel_type.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        stranded.sort_by(|a, b| {
            (&a.from_id, &a.to_id, &a.rel_type).cmp(&(&b.from_id, &b.to_id, &b.rel_type))
        });
        Ok(stranded)
    }

    /// Two-ref structural diff. Produces a per-entity [`crate::ops::Diff`]
    /// comparing the trees at `ref_a` and `ref_b` for the named
    /// mem's storage. Folder and archive backends carry no git
    /// refs and refuse via [`EngineError::InvalidInput`]; the
    /// git-branch backend routes through [`GitBranchOps::diff`] when
    /// the full flavour is loaded.
    ///
    /// `mem` selects the storage context (the gitdir, for
    /// git-branch mounts). `ref_a` / `ref_b` are arbitrary refs the
    /// underlying git layer accepts — branch names, commit SHAs, tag
    /// names — so cross-mem diffs work via fully-qualified refs
    /// (`refs/heads/<other-mem>`) without a separate API.
    ///
    /// Refusal codes:
    /// - [`EngineError::UnknownMem`] (`UNKNOWN_MEM`) — no mount
    ///   for `mem`.
    /// - [`EngineError::UnknownRef`] (`UNKNOWN_REF`) — either ref
    ///   does not resolve. Surfaces verbatim from the git layer's
    ///   `rev_parse` refusal.
    /// - [`EngineError::RenameSimilarityOutOfRange`] (`INVALID_INPUT`)
    ///   — `config.rename_similarity` outside `[0.1, 1.0]`.
    /// - [`EngineError::InvalidInput`] (`INVALID_INPUT`) — mem is
    ///   folder or archive-backed (no refs to diff).
    pub fn diff(
        &self,
        mem: &str,
        ref_a: &str,
        ref_b: &str,
        config: Option<crate::ops::DiffConfig>,
    ) -> Result<crate::ops::Diff, EngineError> {
        let m = self.find_mount(mem)?;
        let config = config.unwrap_or_default();

        if config.rename_similarity < crate::ops::RENAME_SIMILARITY_MIN
            || config.rename_similarity > crate::ops::RENAME_SIMILARITY_MAX
        {
            return Err(EngineError::RenameSimilarityOutOfRange {
                requested: config.rename_similarity,
                allowed_min: crate::ops::RENAME_SIMILARITY_MIN,
                allowed_max: crate::ops::RENAME_SIMILARITY_MAX,
            });
        }

        match &m.mount.storage {
            MountStorage::Folder { .. } | MountStorage::Archive { .. } | MountStorage::InMemory => {
                Err(EngineError::InvalidInput(format!(
                    "mem '{mem}' is not git-backed — `memstead_diff` requires a git-branch mount",
                )))
            }
            MountStorage::GitBranch { gitdir, .. } => match self.git_branch_ops.as_ref() {
                Some(hook) => {
                    (hook.diff)(gitdir, mem, ref_a, ref_b, &config).map_err(|e| match e {
                        // Map the standard backend-side "ref not found" shape into the
                        // typed engine-level refusal. The git-branch dispatcher uses
                        // `BackendError::Other` with a leading marker so the engine can
                        // recover the typed code without re-parsing the message.
                        BackendError::Other(msg) if msg.starts_with("UNKNOWN_REF:") => {
                            let raw = msg.trim_start_matches("UNKNOWN_REF:").trim().to_string();
                            EngineError::UnknownRef(raw)
                        }
                        other => EngineError::Backend(other),
                    })
                }
                None => Err(EngineError::Backend(BackendError::Other(
                    "git-branch diff hook not installed (full flavour not loaded)".to_string(),
                ))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use crate::backend::{BackendError, MemBackend};
    use crate::engine::test_helpers::*;
    use crate::engine::{DeleteEntityArgs, Engine, EngineError};
    use crate::entity::EntityId;

    use crate::provenance::Provenance;
    use crate::storage::ArchiveBackend;
    use crate::vcs::CommitContext;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    #[test]
    fn engine_diff_unknown_mem_returns_typed_error() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let err = engine.diff("nope", "a", "b", None).unwrap_err();
        assert!(matches!(err, EngineError::UnknownMem(v) if v == "nope"));
    }

    #[test]
    fn engine_diff_folder_mount_refuses_with_invalid_input() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        // Folder backend has no git refs — refuse cleanly via the
        // typed `INVALID_INPUT` code rather than collapsing through
        // the backend layer.
        let err = engine.diff("specs", "a", "b", None).unwrap_err();
        match err {
            EngineError::InvalidInput(msg) => {
                assert!(msg.contains("not git-backed"), "unexpected msg: {msg}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn engine_diff_rename_similarity_out_of_range_refuses() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let bad = crate::ops::DiffConfig {
            rename_similarity: 2.0,
            ..Default::default()
        };
        let err = engine.diff("specs", "a", "b", Some(bad)).unwrap_err();
        assert!(matches!(
            err,
            EngineError::RenameSimilarityOutOfRange { .. }
        ));
    }

    #[test]
    fn engine_changes_since_archive_mount_returns_empty_report() {
        // Archive backends have no diff surface; the engine wrapper
        // produces an empty `ChangesReport` with the cursor echoed.
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
        let mount = archive_mount("ext", archive_path.clone());
        let engine = Engine::from_mounts(vec![(
            mount,
            Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let report = engine.changes_since("ext", "abc", None).expect("known mem");
        assert_eq!(report.mem, "ext");
        assert_eq!(report.since, "abc");
        assert_eq!(report.head, "abc");
        assert!(report.changes.is_empty());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn engine_changes_since_unknown_mem_returns_typed_error() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let err = engine
            .changes_since("does-not-exist", "abc", None)
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownMem(_)));
    }

    #[test]
    fn engine_changes_since_refuses_rename_similarity_below_min() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        // 0.05 is below RENAME_SIMILARITY_MIN (0.1); typed refusal,
        // not a silent clamp.
        let err = engine
            .changes_since("specs", "abc", Some(0.05))
            .expect_err("out-of-range refuses");
        match err {
            EngineError::RenameSimilarityOutOfRange {
                requested,
                allowed_min,
                allowed_max,
            } => {
                assert!((requested - 0.05).abs() < f32::EPSILON);
                assert!((allowed_min - crate::ops::RENAME_SIMILARITY_MIN).abs() < f32::EPSILON);
                assert!((allowed_max - crate::ops::RENAME_SIMILARITY_MAX).abs() < f32::EPSILON);
            }
            other => panic!("expected RenameSimilarityOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn engine_changes_since_refuses_rename_similarity_above_max() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        // 1.5 is above RENAME_SIMILARITY_MAX (1.0); typed refusal.
        let err = engine
            .changes_since("specs", "abc", Some(1.5))
            .expect_err("out-of-range refuses");
        match err {
            EngineError::RenameSimilarityOutOfRange { requested, .. } => {
                assert!((requested - 1.5).abs() < f32::EPSILON);
            }
            other => panic!("expected RenameSimilarityOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn engine_changes_since_no_warning_when_rename_similarity_in_range() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        // 0.5 is comfortably inside the valid range; no warning.
        let report = engine
            .changes_since("specs", "abc", Some(0.5))
            .expect("known mem");
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn engine_changes_since_no_warning_when_rename_similarity_omitted() {
        // Caller passes None → wrapper falls back to the default;
        // no clamping, no warning.
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let report = engine
            .changes_since("specs", "abc", None)
            .expect("known mem");
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn engine_changes_since_enriches_envelope_title_and_type_from_store() {
        // `build_demo_engine` creates three entities via the engine's
        // mutation pipeline, which appends Create events to the folder
        // backend's changelog. `Engine::changes_since` synthesises
        // BackendChanges from the changelog (id-only envelopes), then
        // enriches title / entity_type from the in-memory store.
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let report = engine
            .changes_since("specs", crate::ops::EMPTY_TREE_SHA, None)
            .expect("known mem");

        // Three Create events → three Added envelopes, each enriched.
        assert_eq!(report.changes.len(), 3);
        for env in &report.changes {
            match env {
                crate::ops::ChangeEnvelope::Added {
                    id,
                    title,
                    entity_type,
                } => {
                    assert!(title.is_some(), "title enriched for {id}");
                    assert_eq!(entity_type.as_deref(), Some("spec"), "type for {id}");
                }
                other => panic!("expected Added envelope, got {other:?}"),
            }
        }
    }

    #[test]
    fn engine_changes_since_removed_envelope_keeps_title_and_type_none() {
        // Create-then-delete net effect = Removed. Even though the
        // store may still know the entity, the engine wrapper
        // unconditionally strips title / entity_type on Removed.
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let (actor, client) = cli_actor();
        let id = EntityId::new("specs", "lonely-three");
        let hash = engine
            .get_entity(&id)
            .expect("seeded entity present")
            .content_hash
            .clone();
        engine
            .delete_entity(
                DeleteEntityArgs {
                    id: id.clone(),
                    expected_hash: Some(hash),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let report = engine
            .changes_since("specs", crate::ops::EMPTY_TREE_SHA, None)
            .unwrap();
        let removed = report
            .changes
            .iter()
            .find(|e| {
                matches!(e,
                crate::ops::ChangeEnvelope::Removed { id: rid, .. } if rid == &id)
            })
            .expect("removed envelope for lonely-three");
        match removed {
            crate::ops::ChangeEnvelope::Removed {
                title, entity_type, ..
            } => {
                assert!(title.is_none());
                assert!(entity_type.is_none());
            }
            other => panic!("expected Removed, got {other:?}"),
        }
    }

    // ---- Engine::cross_mem_link_allowed ---------------------------

    #[test]
    fn reload_if_stale_returns_empty_for_folder_only_engine() {
        // The folder backend inherits MemBackend::current_head's
        // default (Ok(None)). Drift detection sees no signal and
        // returns no warnings.
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let warnings = engine.reload_if_stale(None);
        assert!(warnings.is_empty());
        let warnings = engine.reload_if_stale(Some("specs"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn reload_if_stale_short_circuits_for_unknown_mem_filter() {
        // Filtering by an unknown mem produces zero candidates;
        // the method returns an empty Vec without panicking.
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let warnings = engine.reload_if_stale(Some("does-not-exist"));
        assert!(warnings.is_empty());
    }

    /// Test fixture: a `MemBackend` whose `current_head` and
    /// (read-side) entity surface are externally mutable so a test
    /// can simulate a sibling writer advancing the head between
    /// drift-check probes. Write methods are no-ops; the engine's
    /// drift-check path never invokes them.
    struct ManualHeadBackend {
        head: std::sync::Mutex<Option<String>>,
        entities: std::sync::Mutex<Vec<(PathBuf, Vec<u8>)>>,
    }

    impl ManualHeadBackend {
        fn new(initial_head: Option<&str>) -> Self {
            Self {
                head: std::sync::Mutex::new(initial_head.map(String::from)),
                entities: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn set_head(&self, head: Option<&str>) {
            *self.head.lock().unwrap() = head.map(String::from);
        }
    }

    impl MemBackend for ManualHeadBackend {
        fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
            Ok(self
                .entities
                .lock()
                .unwrap()
                .iter()
                .map(|(p, _)| p.clone())
                .collect())
        }
        fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
            Ok(self
                .entities
                .lock()
                .unwrap()
                .iter()
                .find(|(p, _)| p == rel)
                .map(|(_, b)| b.clone()))
        }
        fn write_entity(&self, _: &Path, _: &[u8]) -> Result<(), BackendError> {
            Ok(())
        }
        fn delete_entity(&self, _: &Path) -> Result<(), BackendError> {
            Ok(())
        }
        fn move_entity(&self, _: &Path, _: &Path) -> Result<(), BackendError> {
            Ok(())
        }
        fn commit(
            &self,
            _: &str,
            _: &CommitContext<'_>,
        ) -> Result<crate::storage::CommitId, BackendError> {
            Ok("synthetic".to_string())
        }
        fn append_provenance(&self, _: &Provenance) -> Result<(), BackendError> {
            Ok(())
        }
        fn read_provenance(&self, _: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
            Ok(Vec::new())
        }
        fn current_head(&self) -> Result<Option<String>, BackendError> {
            Ok(self.head.lock().unwrap().clone())
        }
    }

    #[test]
    fn reload_if_stale_emits_mem_reloaded_when_head_advances() {
        // Use an Arc<ManualHeadBackend> so the test retains a handle
        // for mutation after the engine has taken ownership of a
        // Box<dyn MemBackend> wrapper around it.
        struct ArcBackend(std::sync::Arc<ManualHeadBackend>);
        impl MemBackend for ArcBackend {
            fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
                self.0.list_entities()
            }
            fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
                self.0.read_entity(rel)
            }
            fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
                self.0.write_entity(p, b)
            }
            fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
                self.0.delete_entity(p)
            }
            fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
                self.0.move_entity(f, t)
            }
            fn commit(
                &self,
                m: &str,
                c: &CommitContext<'_>,
            ) -> Result<crate::storage::CommitId, BackendError> {
                self.0.commit(m, c)
            }
            fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
                self.0.append_provenance(r)
            }
            fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
                self.0.read_provenance(c)
            }
            fn current_head(&self) -> Result<Option<String>, BackendError> {
                self.0.current_head()
            }
        }

        let shared = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
        let backend = Box::new(ArcBackend(shared.clone()));
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(pin("default")),
            storage: MountStorage::Folder {
                path: PathBuf::from("/dev/null"),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();

        // No drift on first probe — cached==new.
        let warnings = engine.reload_if_stale(Some("specs"));
        assert!(warnings.is_empty());

        // Sibling writer advances the head.
        shared.set_head(Some("bbb"));

        let warnings = engine.reload_if_stale(Some("specs"));
        assert_eq!(warnings.len(), 1);
        match &warnings[0] {
            crate::ops::WarningHint::MemReloaded {
                mem,
                old_head,
                new_head,
                ..
            } => {
                assert_eq!(mem, "specs");
                assert_eq!(old_head, "aaa");
                assert_eq!(new_head, "bbb");
            }
            other => panic!("expected MemReloaded, got {other:?}"),
        }

        // Drift cleared — the engine's cached head now matches the
        // backend's current head; another probe is a no-op.
        let warnings = engine.reload_if_stale(Some("specs"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn mem_drifted_tracks_sibling_advance_until_reload() {
        // The read-only drift probe used by the macOS roster: it reports
        // `true` once a sibling writer advances the backend past the
        // engine's cached head, *without* itself reloading, and clears
        // after the engine re-reads.
        struct ArcBackend(std::sync::Arc<ManualHeadBackend>);
        impl MemBackend for ArcBackend {
            fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
                self.0.list_entities()
            }
            fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
                self.0.read_entity(rel)
            }
            fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
                self.0.write_entity(p, b)
            }
            fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
                self.0.delete_entity(p)
            }
            fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
                self.0.move_entity(f, t)
            }
            fn commit(
                &self,
                m: &str,
                c: &CommitContext<'_>,
            ) -> Result<crate::storage::CommitId, BackendError> {
                self.0.commit(m, c)
            }
            fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
                self.0.append_provenance(r)
            }
            fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
                self.0.read_provenance(c)
            }
            fn current_head(&self) -> Result<Option<String>, BackendError> {
                self.0.current_head()
            }
        }

        let shared = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
        let backend = Box::new(ArcBackend(shared.clone()));
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(pin("default")),
            storage: MountStorage::Folder {
                path: PathBuf::from("/dev/null"),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();

        // Fresh boot: cached == live, no drift.
        assert!(!engine.mem_drifted("specs").unwrap());

        // Sibling writer advances the head — drift is visible WITHOUT a reload.
        shared.set_head(Some("bbb"));
        assert!(engine.mem_drifted("specs").unwrap());
        // Probing did not reload — still drifted on a second read.
        assert!(engine.mem_drifted("specs").unwrap());

        // Re-reading through the engine clears it.
        let _ = engine.reload_if_stale(Some("specs"));
        assert!(!engine.mem_drifted("specs").unwrap());

        // Unknown mem errors rather than reporting a bogus `false`.
        assert!(matches!(
            engine.mem_drifted("nope"),
            Err(EngineError::UnknownMem(_))
        ));
    }

    #[test]
    fn reload_one_mem_report_head_before_is_prior_cursor_and_advances() {
        // Regression for the reload→changes_since recipe. `head_before`
        // must report the engine's PRIOR cursor (the SHA it last knew),
        // not the post-drift on-disk tip — otherwise
        // `changes_since(since=head_before)` spans an empty range in
        // exactly the sibling-drift case the recipe targets. The reload
        // must also advance the cursor to the new tip so the next
        // staleness probe is a no-op rather than a spurious reload.
        struct ArcBackend(std::sync::Arc<ManualHeadBackend>);
        impl MemBackend for ArcBackend {
            fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
                self.0.list_entities()
            }
            fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
                self.0.read_entity(rel)
            }
            fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
                self.0.write_entity(p, b)
            }
            fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
                self.0.delete_entity(p)
            }
            fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
                self.0.move_entity(f, t)
            }
            fn commit(
                &self,
                m: &str,
                c: &CommitContext<'_>,
            ) -> Result<crate::storage::CommitId, BackendError> {
                self.0.commit(m, c)
            }
            fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
                self.0.append_provenance(r)
            }
            fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
                self.0.read_provenance(c)
            }
            fn current_head(&self) -> Result<Option<String>, BackendError> {
                self.0.current_head()
            }
        }

        let shared = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
        let backend = Box::new(ArcBackend(shared.clone()));
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(pin("default")),
            storage: MountStorage::Folder {
                path: PathBuf::from("/dev/null"),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();

        // Sibling writer advances the head past the engine's cursor.
        shared.set_head(Some("bbb"));

        let report = engine.reload_one_mem_report("specs").unwrap();
        // head_before is the prior cursor "aaa", not the drifted tip.
        assert_eq!(report.head_before, "aaa");
        assert_eq!(report.head_after, "bbb");

        // Cursor advanced to "bbb": a follow-up staleness probe is a
        // no-op, not a spurious MEM_RELOADED.
        let warnings = engine.reload_if_stale(Some("specs"));
        assert!(
            warnings.is_empty(),
            "cursor should have advanced to bbb, got {warnings:?}"
        );
    }

    #[test]
    fn reload_if_stale_fires_every_call_no_throttle() {
        // Two back-to-back probes with the head advancing between
        // them: the second must reload and warn. There is no throttle
        // window — the ref check is the correctness floor.
        let shared = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
        struct ArcBackend(std::sync::Arc<ManualHeadBackend>);
        impl MemBackend for ArcBackend {
            fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
                self.0.list_entities()
            }
            fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
                self.0.read_entity(rel)
            }
            fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
                self.0.write_entity(p, b)
            }
            fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
                self.0.delete_entity(p)
            }
            fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
                self.0.move_entity(f, t)
            }
            fn commit(
                &self,
                m: &str,
                c: &CommitContext<'_>,
            ) -> Result<crate::storage::CommitId, BackendError> {
                self.0.commit(m, c)
            }
            fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
                self.0.append_provenance(r)
            }
            fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
                self.0.read_provenance(c)
            }
            fn current_head(&self) -> Result<Option<String>, BackendError> {
                self.0.current_head()
            }
        }

        let backend = Box::new(ArcBackend(shared.clone()));
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(pin("default")),
            storage: MountStorage::Folder {
                path: PathBuf::from("/dev/null"),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();

        // First probe observes cached==new; no warning.
        let warnings = engine.reload_if_stale(Some("specs"));
        assert!(warnings.is_empty());

        // Sibling advances head — the very next probe reloads and
        // warns, with no throttle window to mask it.
        shared.set_head(Some("bbb"));
        let warnings = engine.reload_if_stale(Some("specs"));
        assert_eq!(
            warnings.len(),
            1,
            "no throttle window — the moved ref reloads on the next probe"
        );
    }
}
