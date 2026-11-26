//! The check rows' transport between stores.
//!
//! A git-branch mem's rows live on its checks ref in the store that holds
//! it ([`crate::workspace::checks_ref_for_mem`], `engine::checks_home`).
//! Moving them between machines is moving that ref: a push of the mem
//! pushes the ref beside the branch, a fetch or pull fetches the remote's
//! ref of the mem and unites its rows into the local one. A row is a fact
//! whose identity is its bytes: two machines checking the same mem merge
//! by set union, nothing is rewritten, and a round trip adds nothing.
//! When the remote's ref holds a commit the local ref does not descend
//! from, the publish writes the united rows on top of the remote's tip,
//! so the push is a fast-forward; a race with another publisher is a
//! non-fast-forward the publish retries once after importing what landed.
//!
//! Engines before 0.29 published every mem's rows on one branch,
//! [`crate::MEMSTEAD_CHECKS_REF_BRANCH`], `mems/<mem>/checks.jsonl` per
//! mem. A fetch and a pull still read a mem's member there when the
//! remote carries it; nothing writes it any more.
//!
//! The trust rule: rows arrive only through `fetch`, `pull` and `push`
//! of a mem this workspace mounts (and `mem mount --remote`, which
//! mounts it), and only that mem's rows. A fork's ref is never imported:
//! `mem fork --remote` in either form fetches the branch it names and
//! nothing else, so a contributor cannot push checker-role rows that
//! close the owner's gates. Imported rows keep their identity and role,
//! and read exactly as local rows do.

use std::path::Path;

use crate::backend::BackendError;
use crate::check::CheckLedger;
use crate::engine::checks_home::CheckHome;
use crate::engine::{Engine, EngineError, GitBranchOps, RefPrevious};
use crate::workspace::{branch_full_ref, checks_ref_for_mem, checks_tracking_ref};

/// The old single transport branch's tracking ref for `remote`.
fn legacy_tracking_ref(remote: &str) -> String {
    format!(
        "refs/remotes/{remote}/{}",
        crate::MEMSTEAD_CHECKS_REF_BRANCH
    )
}

fn lift_remote(e: BackendError) -> EngineError {
    match e {
        other if EngineError::is_remote_marker(&other) => EngineError::from_transport(other),
        other => EngineError::Backend(other),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One mem's checks ref a publish moved on the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedChecks {
    pub mem: String,
    pub ref_name: String,
    pub new_sha: String,
}

impl Engine {
    /// The remote's ref table, by full ref name.
    fn remote_refs(
        &self,
        hook: &GitBranchOps,
        gitdir: &Path,
        remote: &str,
    ) -> Result<std::collections::HashMap<String, String>, EngineError> {
        Ok((hook.ls_remote)(gitdir, remote)
            .map_err(lift_remote)?
            .into_iter()
            .collect())
    }

    /// The bytes of `file` at `ref_name` in `gitdir`, `None` where the ref
    /// or the file does not exist.
    fn read_checks_file_at(
        &self,
        gitdir: &Path,
        ref_name: &str,
        file: &str,
    ) -> Result<Option<Vec<u8>>, EngineError> {
        let backend = self.checks_ref_backend(gitdir, ref_name)?;
        if backend
            .current_head()
            .map_err(EngineError::Backend)?
            .is_none()
        {
            return Ok(None);
        }
        backend
            .read_entity(Path::new(file))
            .map_err(EngineError::Backend)
    }

    /// Unite rows of `mem` (in the transport form) into where the mem's
    /// rows live, refusing the rows dated beyond the clock skew ahead.
    fn unite_imported_rows(
        &self,
        mem: &str,
        bytes: &[u8],
        merged: &mut crate::check::MergedRows,
    ) -> Result<(), EngineError> {
        let ledger = self.check_ledger_for_mem(mem);
        let prefix = format!("{mem}--");
        let (fresh, refused) = ledger.new_transport_rows(bytes, now_secs());
        // Only this mem's rows: a member never brings another mem's.
        let fresh: Vec<_> = fresh
            .into_iter()
            .filter(|r| r.entity.starts_with(&prefix))
            .collect();
        merged.refused.extend(
            refused
                .into_iter()
                .filter(|r| r.entity.starts_with(&prefix)),
        );
        if fresh.is_empty() {
            return Ok(());
        }
        let appended = match self.check_home(mem) {
            CheckHome::Store { gitdir } => self.append_store_check_rows(&gitdir, mem, &fresh)?,
            CheckHome::File => {
                let Some(root) = self.workspace_root() else {
                    return Ok(());
                };
                let file = CheckLedger::for_workspace(root);
                for rec in &fresh {
                    file.record(rec).map_err(|e| {
                        EngineError::Mem(format!("check ledger: appending imported rows: {e}"))
                    })?;
                }
                fresh.len()
            }
        };
        merged.appended += appended;
        Ok(())
    }

    /// Fetch the remote's checks refs of `mems` (and the old single
    /// transport branch, where the remote still carries it) and unite
    /// each mem's rows into where the mem's rows live. Returns the rows
    /// appended and the rows refused, each named (a row dated beyond the
    /// clock skew ahead of this engine's clock).
    pub(crate) fn import_checks(
        &self,
        hook: &GitBranchOps,
        gitdir: &Path,
        remote: &str,
        mems: &[&str],
    ) -> Result<crate::check::MergedRows, EngineError> {
        let mut merged = crate::check::MergedRows::default();
        let refs = self.remote_refs(hook, gitdir, remote)?;
        let legacy = branch_full_ref(crate::MEMSTEAD_CHECKS_REF_BRANCH);
        let legacy_fetched = if refs.contains_key(&legacy) {
            (hook.fetch)(
                gitdir,
                remote,
                &[format!("+{legacy}:{}", legacy_tracking_ref(remote))],
            )
            .map_err(lift_remote)?;
            true
        } else {
            false
        };
        for mem in mems {
            let ref_name = checks_ref_for_mem(mem);
            if refs.contains_key(&ref_name) {
                let tracking = checks_tracking_ref(remote, mem);
                (hook.fetch)(gitdir, remote, &[format!("+{ref_name}:{tracking}")])
                    .map_err(lift_remote)?;
                if let Some(bytes) =
                    self.read_checks_file_at(gitdir, &tracking, crate::CHECKS_REF_FILE)?
                    && !self.fast_forward_checks(
                        hook,
                        gitdir,
                        mem,
                        &tracking,
                        &bytes,
                        &mut merged,
                    )?
                {
                    self.unite_imported_rows(mem, &bytes, &mut merged)?;
                }
            }
            if legacy_fetched
                && let Some(bytes) = self.read_checks_file_at(
                    gitdir,
                    &legacy_tracking_ref(remote),
                    &crate::checks_ref_member_path(mem),
                )?
            {
                self.unite_imported_rows(mem, &bytes, &mut merged)?;
            }
        }
        Ok(merged)
    }

    /// Where the local checks ref of `mem` lives in `gitdir` and stands
    /// at or behind the fetched `tracking` tip, and the remote's rows
    /// hold nothing this engine refuses: move the local ref forward to
    /// the remote's tip, so the two refs stay one line. Returns whether it
    /// did (the rows are then in place); otherwise the caller unites the
    /// rows by a commit of its own.
    fn fast_forward_checks(
        &self,
        hook: &GitBranchOps,
        gitdir: &Path,
        mem: &str,
        tracking: &str,
        bytes: &[u8],
        merged: &mut crate::check::MergedRows,
    ) -> Result<bool, EngineError> {
        let CheckHome::Store { gitdir: home } = self.check_home(mem) else {
            return Ok(false);
        };
        if home != gitdir {
            return Ok(false);
        }
        let ref_name = checks_ref_for_mem(mem);
        let local_tip = self
            .checks_ref_backend(gitdir, &ref_name)?
            .current_head()
            .map_err(EngineError::Backend)?;
        let Some(remote_tip) = self
            .checks_ref_backend(gitdir, tracking)?
            .current_head()
            .map_err(EngineError::Backend)?
        else {
            return Ok(false);
        };
        if local_tip.as_deref() == Some(remote_tip.as_str()) {
            return Ok(true);
        }
        if let Some(lt) = &local_tip
            && !(hook.is_ancestor)(gitdir, lt, &remote_tip).map_err(EngineError::Backend)?
        {
            return Ok(false);
        }
        let ledger = self.check_ledger_for_mem(mem);
        let prefix = format!("{mem}--");
        let (fresh, refused) = ledger.new_transport_rows(bytes, now_secs());
        if !refused.is_empty() || fresh.iter().any(|r| !r.entity.starts_with(&prefix)) {
            return Ok(false);
        }
        let previous = match &local_tip {
            Some(lt) => RefPrevious::At(lt),
            None => RefPrevious::Absent,
        };
        if (hook.update_ref)(gitdir, &ref_name, &remote_tip, previous).is_err() {
            // Another writer moved the ref meanwhile: unite instead.
            return Ok(false);
        }
        if let Ok(mut cache) = self.checks_cache.lock() {
            cache.rows.remove(&(gitdir.to_path_buf(), mem.to_string()));
        }
        merged.appended += fresh.len();
        Ok(true)
    }

    /// Publish the checks refs of `mems` to `remote`. Each mem's rows are
    /// first completed with the remote's (an import) and with the rows
    /// this workspace's file still holds for it. When every row the
    /// local ref holds is on the remote already, nothing is pushed. A
    /// local ref the remote's tip is an ancestor of is pushed as it is.
    /// Otherwise the publish builds a commit on the remote's tip that
    /// holds the remote's rows and the local ones, and pushes that: the
    /// remote's rows stay on it even when this ledger refused them on
    /// import (a row dated ahead of this clock), and the local ref keeps
    /// only the rows this workspace took. A race with another publisher
    /// is retried once. Returns each ref the push moved and the rows the
    /// import refused, named.
    pub(crate) fn publish_checks(
        &self,
        hook: &GitBranchOps,
        gitdir: &Path,
        remote: &str,
        mems: &[&str],
    ) -> Result<(Vec<PublishedChecks>, Vec<crate::check::RefusedCheckRow>), EngineError> {
        let mut published = Vec::new();
        let mut refused = Vec::new();
        for mem in mems {
            let CheckHome::Store { gitdir: home } = self.check_home(mem) else {
                continue;
            };
            if home != gitdir {
                continue;
            }
            // Rows an older engine left in the file travel too.
            if let Some(root) = self.workspace_root() {
                let rows = CheckLedger::for_workspace(root).file_rows_for_mem(mem);
                self.append_store_check_rows(gitdir, mem, &rows)?;
            }
            let ref_name = checks_ref_for_mem(mem);
            let tracking = checks_tracking_ref(remote, mem);
            let file = crate::CHECKS_REF_FILE;
            let mut last_err: Option<EngineError> = None;
            let mut done = false;
            for _attempt in 0..2 {
                let imported = self.import_checks(hook, gitdir, remote, &[mem])?;
                refused.extend(imported.refused);
                let Some(local_tip) = self
                    .checks_ref_backend(gitdir, &ref_name)?
                    .current_head()
                    .map_err(EngineError::Backend)?
                else {
                    done = true;
                    break;
                };
                let remote_tip = self.remote_refs(hook, gitdir, remote)?.remove(&ref_name);
                let local_rows = CheckLedger::parse_lines(
                    &self
                        .read_checks_file_at(gitdir, &ref_name, file)?
                        .unwrap_or_default(),
                );
                let remote_rows = match &remote_tip {
                    Some(_) => CheckLedger::parse_lines(
                        &self
                            .read_checks_file_at(gitdir, &tracking, file)?
                            .unwrap_or_default(),
                    ),
                    None => Vec::new(),
                };
                let on_remote: std::collections::HashSet<String> = remote_rows
                    .iter()
                    .filter_map(|r| serde_json::to_string(r).ok())
                    .collect();
                if remote_tip.is_some()
                    && local_rows.iter().all(|r| {
                        serde_json::to_string(r)
                            .map(|c| on_remote.contains(&c))
                            .unwrap_or(true)
                    })
                {
                    done = true;
                    break;
                }
                let fast_forward = match &remote_tip {
                    None => true,
                    Some(rt) => {
                        (hook.is_ancestor)(gitdir, rt, &local_tip).map_err(EngineError::Backend)?
                    }
                };
                let pushed = if fast_forward {
                    (hook.push_ref)(gitdir, remote, &ref_name, &ref_name)
                } else {
                    let rt = remote_tip.as_deref().unwrap_or_default();
                    let mut all = remote_rows;
                    all.extend(local_rows);
                    self.commit_checks_on(gitdir, &tracking, rt, mem, &all)?;
                    (hook.push_ref)(gitdir, remote, &tracking, &ref_name)
                };
                match pushed {
                    Ok(new_sha) => {
                        published.push(PublishedChecks {
                            mem: mem.to_string(),
                            ref_name: ref_name.clone(),
                            new_sha,
                        });
                        done = true;
                        break;
                    }
                    Err(BackendError::Other(msg)) if msg.starts_with("NON_FAST_FORWARD:") => {
                        last_err = Some(EngineError::NonFastForward {
                            mem: ref_name.clone(),
                            remote: remote.to_string(),
                        });
                        continue;
                    }
                    Err(e) => return Err(lift_remote(e)),
                }
            }
            if !done {
                return Err(last_err.unwrap_or_else(|| {
                    EngineError::Mem("check rows publish: no attempt ran".to_string())
                }));
            }
        }
        Ok((published, refused))
    }

    /// Commit `rows` (in the transport form) on `vehicle`, a ref that
    /// stands at `parent`: the commit a publish pushes when the remote's
    /// tip is not an ancestor of the local ref. The vehicle is the
    /// tracking ref, which a fetch moves again; the local ref is never
    /// touched here.
    fn commit_checks_on(
        &self,
        gitdir: &Path,
        vehicle: &str,
        parent: &str,
        mem: &str,
        rows: &[crate::check::CheckRecord],
    ) -> Result<(), EngineError> {
        let hook = self
            .git_branch_ops
            .ok_or_else(|| EngineError::Mem("git-branch ops not wired".to_string()))?;
        (hook.update_ref)(gitdir, vehicle, parent, RefPrevious::Any)
            .map_err(EngineError::Backend)?;
        let backend = self.checks_ref_backend(gitdir, vehicle)?;
        backend
            .write_entity(
                Path::new(crate::CHECKS_REF_FILE),
                &CheckLedger::transport_lines(rows),
            )
            .map_err(EngineError::Backend)?;
        let ctx = crate::vcs::CommitContext::new(
            Some("memstead_push"),
            crate::vcs::Actor::Cli,
            None,
            None,
            self.current_role(),
            self.current_identity().map(str::to_string),
        );
        backend
            .commit_with_expected_parent(&format!("memstead: checks {mem}"), &ctx, Some(parent))
            .map_err(EngineError::Backend)?;
        Ok(())
    }
}
