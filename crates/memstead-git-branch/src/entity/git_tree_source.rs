//! Git-tree-backed entity source — reads `.md` blobs out of a vault
//! ref's tree without a working tree on disk.
//!
//! Mirrors `Directory`'s walk semantics from
//! [`memstead_base::entity::source`]: skip `.git/` and `.memstead/`, keep
//! only `*.md` blobs, return UTF-8 content. Feeds the same parse pipeline
//! via [`memstead_base::entity::loader::parse_entries`].

use std::path::PathBuf;

use memstead_base::entity::loader::{LoadError, LoadResult, parse_entries};
use memstead_base::entity::source::{SourceEntry, SourceReadError};
use memstead_schema::Schema;

/// A vault stored as a git ref's tree — no working tree on disk.
///
/// `repo` is opened once per call by the engine — fresh opens are
/// cheap (no clone, no fetch). `ref_name` is fully-qualified
/// (e.g. `refs/heads/specs`); a missing ref surfaces as
/// [`LoadError::RefNotFound`].
pub struct GitTreeSource {
    pub repo: gix::Repository,
    pub ref_name: String,
}

impl GitTreeSource {
    pub fn read_all(&self) -> Result<(Vec<SourceEntry>, Vec<SourceReadError>), LoadError> {
        read_git_tree(&self.repo, &self.ref_name)
    }
}

/// Resolve `ref_name` in `repo` to a tree, walk every blob entry, and
/// hand back `(path, content)` pairs for the markdown files. Mirrors
/// [`memstead_base::entity::source::EntitySource::Directory`]'s walk
/// semantics (skip `.git/` and `.memstead/`, keep only `*.md` blobs,
/// return UTF-8 content).
///
/// Path strings on tree entries are forward-slash separated regardless
/// of host OS — matches what `Directory` produces on POSIX hosts and
/// keeps dual-adapter parity honest.
fn read_git_tree(
    repo: &gix::Repository,
    ref_name: &str,
) -> Result<(Vec<SourceEntry>, Vec<SourceReadError>), LoadError> {
    let mut reference = repo
        .try_find_reference(ref_name)
        .map_err(|e| LoadError::GitTree(format!("resolve {ref_name}: {e}")))?
        .ok_or_else(|| LoadError::RefNotFound(ref_name.to_string()))?;

    let tree = reference
        .peel_to_tree()
        .map_err(|e| LoadError::GitTree(format!("peel {ref_name} to tree: {e}")))?;

    let entries = tree
        .traverse()
        .breadthfirst
        .files()
        .map_err(|e| LoadError::GitTree(format!("traverse {ref_name}: {e}")))?;

    let mut out = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        if !entry.mode.is_blob() {
            continue;
        }
        let path = match std::str::from_utf8(entry.filepath.as_slice()) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        if !path.ends_with(".md") {
            continue;
        }
        // Skip engine-internal subtrees. Mirrors `find_markdown_files`
        // on the disk side. `.git/` cannot exist inside a tree object
        // (git itself rejects it), but we filter anyway for symmetry.
        if path.starts_with(".git/") || path.starts_with(".memstead/") {
            continue;
        }

        let blob = match repo.find_object(entry.oid) {
            Ok(o) => o,
            Err(e) => {
                errors.push(SourceReadError {
                    source_path: PathBuf::from(&path),
                    error: std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("find blob {}: {e}", entry.oid),
                    ),
                });
                continue;
            }
        };
        let content = match String::from_utf8(blob.data.clone()) {
            Ok(s) => s,
            Err(e) => {
                errors.push(SourceReadError {
                    source_path: PathBuf::from(&path),
                    error: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("blob {} not utf-8: {e}", entry.oid),
                    ),
                });
                continue;
            }
        };

        out.push(SourceEntry {
            relative_path: path.clone(),
            source_path: PathBuf::from(path),
            content,
        });
    }

    // Deterministic ordering, matching the disk walk's post-walk sort.
    out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    Ok((out, errors))
}

/// Load all entities from a vault stored as a branch in a
/// `vault-repo-git` repository.
///
/// Reads blobs from the git object database and feeds the result
/// through [`memstead_base::entity::loader::parse_entries`] so the entity
/// pipeline matches the directory- and archive-backed loaders byte
/// for byte.
pub fn load_vault_from_git_tree(
    repo: gix::Repository,
    ref_name: &str,
    vault: &str,
    vault_schema: &Schema,
) -> Result<LoadResult, LoadError> {
    let source = GitTreeSource {
        repo,
        ref_name: ref_name.to_string(),
    };
    let (entries, read_errors) = source.read_all()?;
    Ok(parse_entries(entries, read_errors, vault, vault_schema))
}

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_base::entity::source::EntitySource;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    use gix::objs::tree::EntryKind;

    /// Build a fresh bare repository in `git_dir`, write the given
    /// `(path, content)` entries as blobs into a tree, commit that
    /// tree to `ref_name`, and return the open repository handle.
    fn seed_git_tree(
        git_dir: &Path,
        ref_name: &str,
        entries: &[(&str, &str)],
    ) -> gix::Repository {
        gix::init_bare(git_dir).unwrap();
        let repo = gix::open(git_dir).unwrap();

        let mut editor = repo.empty_tree().edit().expect("editor init");
        for (path, content) in entries {
            let blob_id = repo.write_blob(content.as_bytes()).unwrap().detach();
            editor
                .upsert(*path, EntryKind::Blob, blob_id)
                .expect("upsert");
        }
        let tree_id = editor.write().expect("tree write").detach();

        let time = gix::date::Time {
            seconds: 0,
            offset: 0,
        };
        let sig = gix::actor::Signature {
            name: "Test".into(),
            email: "test@example.com".into(),
            time,
        };
        let mut buf = gix::date::parse::TimeBuf::default();
        let sig_ref = sig.to_ref(&mut buf);
        repo.commit_as(
            sig_ref,
            sig_ref,
            ref_name,
            "seed",
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .expect("commit_as");
        repo
    }

    #[test]
    fn git_tree_read_matches_directory_read() {
        // Parallel fixtures: same byte content under both adapters
        // must produce the same path-set with byte-identical contents.
        let dir = TempDir::new().unwrap();
        let entries = [
            ("a.md", "alpha"),
            ("b.md", "beta"),
            ("nested/c.md", "gamma"),
            ("nested/deeper/d.md", "delta"),
            ("ignored.txt", "skip me"),
        ];

        let disk_root = dir.path().join("disk");
        for (rel, content) in entries.iter() {
            let p = disk_root.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, content).unwrap();
        }
        let (disk_entries, disk_errs) = EntitySource::Directory { root: disk_root }
            .read_all()
            .unwrap();
        assert!(disk_errs.is_empty());

        let git_dir = dir.path().join("git");
        let repo = seed_git_tree(&git_dir, "refs/heads/main", &entries);
        let (tree_entries, tree_errs) = GitTreeSource {
            repo,
            ref_name: "refs/heads/main".to_string(),
        }
        .read_all()
        .unwrap();
        assert!(tree_errs.is_empty());

        let mut disk_pairs: Vec<(String, String)> = disk_entries
            .iter()
            .map(|e| (e.relative_path.clone(), e.content.clone()))
            .collect();
        let mut tree_pairs: Vec<(String, String)> = tree_entries
            .iter()
            .map(|e| (e.relative_path.clone(), e.content.clone()))
            .collect();
        disk_pairs.sort();
        tree_pairs.sort();
        assert_eq!(disk_pairs, tree_pairs);
        // Both adapters drop `ignored.txt` (non-`.md`) and keep the
        // four markdown files.
        assert_eq!(tree_pairs.len(), 4);
    }

    #[test]
    fn git_tree_skips_engine_internal_dirs() {
        // `.memstead/note.md` is always skipped (engine-internal). An
        // ordinary dot-dir (`.other/note.md`) is not special — it walks
        // like any ordinary nested dir. `.git/` cannot exist inside a
        // git tree object so we don't seed it.
        let dir = TempDir::new().unwrap();
        let git_dir = dir.path().join("git");
        let entries = [
            ("keep.md", "k"),
            (".memstead/note.md", "n"),
            (".other/note.md", "n"),
            ("docs/deep.md", "d"),
        ];
        let repo = seed_git_tree(&git_dir, "refs/heads/main", &entries);
        let (got, errs) = GitTreeSource {
            repo,
            ref_name: "refs/heads/main".to_string(),
        }
        .read_all()
        .unwrap();
        assert!(errs.is_empty());
        let paths: Vec<_> = got.iter().map(|e| e.relative_path.as_str()).collect();
        assert!(paths.contains(&"keep.md"), "got {paths:?}");
        assert!(
            paths.iter().any(|p| p.ends_with("docs/deep.md")),
            "regular nested dirs must load: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with(".memstead/")),
            ".memstead/* must be skipped: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.starts_with(".other/")),
            "an ordinary dot-dir must load, only `.memstead/` is skipped: {paths:?}"
        );
    }

    #[test]
    fn git_tree_missing_ref_yields_error() {
        let dir = TempDir::new().unwrap();
        let git_dir = dir.path().join("git");
        // Need a real repo with at least one ref so `gix::open` succeeds.
        let repo = seed_git_tree(&git_dir, "refs/heads/main", &[("a.md", "x")]);
        let err = GitTreeSource {
            repo,
            ref_name: "refs/heads/does-not-exist".to_string(),
        }
        .read_all()
        .unwrap_err();
        match err {
            LoadError::RefNotFound(name) => {
                assert!(
                    name.contains("does-not-exist"),
                    "RefNotFound must echo the ref: {name}"
                );
            }
            other => panic!("expected RefNotFound, got {other:?}"),
        }
    }

    #[test]
    fn load_vault_via_git_tree_source_round_trips_a_blob() {
        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("vault-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();
        let repo = gix::open(&gitdir).unwrap();

        let blob = repo
            .write_blob(
                "---\ntype: spec\n---\n# Tree Entity\n\n## Identity\n\nFrom git.\n".as_bytes(),
            )
            .unwrap()
            .detach();
        let mut editor = repo.empty_tree().edit().unwrap();
        editor
            .upsert("tree-entity.md", gix::objs::tree::EntryKind::Blob, blob)
            .unwrap();
        let tree_id = editor.write().unwrap().detach();

        let actor = gix::actor::Signature {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time {
                seconds: 0,
                offset: 0,
            },
        };
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/specs",
            "seed",
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();

        let schema = Schema::builtin_default();
        let result =
            load_vault_from_git_tree(repo, "refs/heads/specs", "specs", &schema).unwrap();

        assert_eq!(result.entities.len(), 1);
        assert!(result.errors.is_empty());
        assert_eq!(result.entities[0].entity.title, "Tree Entity");
    }
}
