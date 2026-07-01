//! Multi-mem routing, visibility filtering, mem config.
//!
//! Writable/visible mem tracking. The engine loads multiple mems;
//! some may be read-only (from --read-mem paths or JSON imports).
//!
//! The router is structured as a **snapshot** (`MemRouterSnapshot`) —
//! a `Clone`-able value that the engine holds behind an `Arc`. Lifecycle
//! operations mutate by cloning the snapshot, editing the clone, and
//! swapping the `Arc` pointer on `Engine`. Readers that hold an `Arc`
//! before the swap observe the pre-swap snapshot for their lifetime; no
//! torn reads are possible. The MCP-level `memstead_mem_create` /
//! `memstead_mem_delete` tools flip state at runtime.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::entity::EntityId;

/// The per-mem engine-internal directory under a folder mem's
/// root (`<mem_root>/.memstead/` — `config.json`, `changes.jsonl`).
/// Defined in `memstead-schema` (mem-config loading lives there);
/// re-exported here as the mem-level home for the concept.
pub use memstead_schema::MEM_META_DIR;

/// Provenance record attached to every writable-mem registration.
///
/// `MEM_NAME_COLLISION` reads the colliding registration's
/// `MemOrigin` and renders it into `details.source` so agents can
/// distinguish "collision with an explicit workspace mem" from
/// "collision with a previously-runtime-created mem" without a
/// follow-up round trip. The enum is kept internal to `memstead-git-branch` with a
/// `render_source` rendering helper that produces the agent-facing
/// string — no public `serde` derivation yet; the render path is the
/// only supported serialization surface until a caller needs structured
/// consumption.
#[derive(Debug, Clone)]
pub enum MemOrigin {
    /// Loaded from the workspace's `.memstead/workspace.toml` `mems = [...]`
    /// entry.
    ExplicitToml,
    /// Registered after `Engine::init` via
    /// `Engine::register_mem_runtime` — the path lifecycle tools
    /// take. `at` captures when the runtime registration
    /// happened (rendered as an RFC-3339 timestamp on the error
    /// surface); `by_tool` names the MCP tool that produced the
    /// registration (today always `"memstead_mem_create"`, but kept
    /// extensible).
    RuntimeCreated {
        at: SystemTime,
        by_tool: &'static str,
    },
}

impl MemOrigin {
    /// Agent-facing string rendering consumed by
    /// `MEM_NAME_COLLISION.details.source`. The string is short,
    /// declarative, and identifies the registration site so agents can
    /// correlate the collision with something they can observe
    /// (`.memstead/workspace.toml` entry, timestamp).
    ///
    /// The workspace config file lives at `.memstead/workspace.toml`.
    /// The error message points at the current path so an agent
    /// following the hint finds it.
    pub fn render_source(&self) -> String {
        match self {
            MemOrigin::ExplicitToml => "explicit from .memstead/workspace.toml".to_string(),
            MemOrigin::RuntimeCreated { at, by_tool } => {
                format!(
                    "runtime-created at {} by {}",
                    render_rfc3339(*at),
                    by_tool
                )
            }
        }
    }

    /// Short discriminator consumed by `memstead_health { include_config:
    /// true }` to tag each writable-mem entry with an agent-readable
    /// origin string. Variants map to stable kebab-case tokens so
    /// downstream filters key on them.
    pub fn kind(&self) -> &'static str {
        match self {
            MemOrigin::ExplicitToml => "explicit",
            MemOrigin::RuntimeCreated { .. } => "runtime_created",
        }
    }
}

/// Render a `SystemTime` as an RFC-3339 UTC timestamp with second
/// precision. Kept deliberately local to this module because the only
/// consumer is `MemOrigin::render_source` — lifting it into a shared
/// utility is premature until a second caller appears.
///
/// Pre-epoch times fall back to the epoch itself; the error surface is
/// an agent-facing string and "crashing on a nonsensical timestamp" is
/// worse than the (vanishingly unlikely) pre-1970 fallback.
fn render_rfc3339(ts: SystemTime) -> String {
    let secs = ts
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let remainder = secs % 86400;
    let hour = remainder / 3600;
    let minute = (remainder % 3600) / 60;
    let second = remainder % 60;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    )
}

/// Convert days since epoch (1970-01-01) to (year, month, day).
/// Algorithm from http://howardhinnant.github.io/date_algorithms.html —
/// duplicated from `entity::generator::days_to_ymd` deliberately: this
/// module's rendering semantics (UTC-anchored, time-inclusive) are a
/// different shape than the generator's pure-date helper, and a shared
/// utility would couple two independent code paths.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Per-writable-mem origin + directory payload held inside
/// `MemRouterSnapshot`. Cloned along with the snapshot on every
/// lifecycle mutation; clone cost is O(1).
///
/// Hierarchical mem identity lives directly on the router HashMap
/// key (and on `Mount::mem`) — there's exactly one identifier (the
/// full path, e.g. `team/sub-mem`). The delete-side lifecycle
/// composer reads the mem name as-is, no path-composition step
/// needed.
#[derive(Debug, Clone)]
pub struct WritableEntry {
    pub dir: Option<PathBuf>,
    pub origin: MemOrigin,
}

/// Mem configuration for the engine runtime — cloneable snapshot.
///
/// Tracks which mems are writable, which are visible, and their
/// directories. Held on `Engine` behind an `Arc<MemRouterSnapshot>`;
/// lifecycle mutations clone the snapshot, edit the clone, and swap the
/// `Arc` pointer atomically inside the engine mutex.
#[derive(Debug, Clone)]
pub struct MemRouterSnapshot {
    /// Writable mem names.
    writable: HashSet<String>,
    /// All visible mem names (writable + read-only).
    visible: HashSet<String>,
    /// Mem name → (directory path, registration origin). Writable
    /// mems only — read-only entries live in `read_only_archives`.
    /// `dir` is `Some(path)` for disk-backed mems and `None` for
    /// mem-repo-backed mems whose content lives only as a branch in
    /// `mem-repo-git` (no working tree).
    writable_entries: HashMap<String, WritableEntry>,
    /// Mem name → sealed-archive path (read-only mems only).
    /// Stored so reload can re-open the same archive when its mtime
    /// changes without needing to re-parse the project config.
    read_only_archives: HashMap<String, PathBuf>,
}

impl MemRouterSnapshot {
    pub fn new() -> Self {
        Self {
            writable: HashSet::new(),
            visible: HashSet::new(),
            writable_entries: HashMap::new(),
            read_only_archives: HashMap::new(),
        }
    }

    /// Register a writable mem with its directory path and origin.
    ///
    /// The mem's hierarchical organisational path is part of `name`
    /// itself (e.g. `"team/sub-mem"`). The router HashMap key, the
    /// `Mount::mem` field, and the lifecycle-allowlist candidate
    /// all converge on the same string — no separate composition
    /// step.
    pub fn add_writable(
        &mut self,
        name: String,
        dir: Option<PathBuf>,
        origin: MemOrigin,
    ) {
        self.visible.insert(name.clone());
        self.writable.insert(name.clone());
        self.writable_entries.insert(
            name,
            WritableEntry { dir, origin },
        );
    }

    /// Remove a writable mem from the router. Returns `true` when
    /// the entry was present.
    ///
    /// Internal — called by `Engine::unregister_mem_runtime`.
    /// Read-only entries are not affected; unregistering a name that
    /// names a read-only mem is a caller-level misuse and returns
    /// `false`.
    pub fn remove_writable(&mut self, name: &str) -> bool {
        if self.writable_entries.remove(name).is_some() {
            self.writable.remove(name);
            // `visible` tracks both writable + read-only; only drop
            // from `visible` when no read-only entry still carries it.
            if !self.read_only_archives.contains_key(name) {
                self.visible.remove(name);
            }
            true
        } else {
            false
        }
    }

    /// Register a read-only mem backed by a sealed `.mem` archive.
    ///
    /// `archive_path` is the on-disk location of the archive. The
    /// router retains it so reload can re-open the same archive without
    /// re-parsing the project config.
    pub fn add_read_only(&mut self, name: String, archive_path: PathBuf) {
        self.visible.insert(name.clone());
        self.read_only_archives.insert(name, archive_path);
    }

    /// Check if a mem is writable.
    pub fn is_writable(&self, mem: &str) -> bool {
        self.writable.contains(mem)
    }

    /// Check if a mem is visible (writable or read-only).
    pub fn is_visible(&self, mem: &str) -> bool {
        self.visible.contains(mem)
    }

    /// Check if an entity is visible from the given context.
    pub fn is_entity_visible(&self, entity_id: &EntityId) -> bool {
        let mem = entity_id.mem();
        mem.is_empty() || self.visible.contains(mem)
    }

    /// Get the directory path for a writable mem.
    ///
    /// Returns `None` when the mem is unknown OR when the mem is
    /// mem-repo-backed (no on-disk directory). Callers that need to
    /// distinguish "mem not found" from "mem has no dir" use
    /// `is_writable` first.
    pub fn dir_for_mem(&self, mem: &str) -> Option<&Path> {
        self.writable_entries
            .get(mem)
            .and_then(|e| e.dir.as_deref())
    }

    /// Get the `MemOrigin` for a writable mem. Used by the
    /// `MEM_NAME_COLLISION` envelope renderer and the
    /// `memstead_health { include_config: true }` per-mem `origin` field.
    pub fn origin_for_mem(&self, mem: &str) -> Option<&MemOrigin> {
        self.writable_entries.get(mem).map(|e| &e.origin)
    }

    /// Get the sealed-archive path for a read-only mem.
    ///
    /// Returns `None` for writable mems and unknown names. Keep this
    /// distinct from `dir_for_mem` — a directory and a zip archive are
    /// different backing stores, and callers usually care which they get.
    pub fn archive_path_for_mem(&self, mem: &str) -> Option<&Path> {
        self.read_only_archives.get(mem).map(|p| p.as_path())
    }

    /// Get all writable mem names.
    pub fn writable_mems(&self) -> &HashSet<String> {
        &self.writable
    }

    /// Get all visible mem names.
    pub fn visible_mems(&self) -> &HashSet<String> {
        &self.visible
    }

    /// Validate that a mem is writable, returning an error message if not.
    pub fn validate_writable(&self, mem: &str) -> Result<(), String> {
        if self.writable.contains(mem) {
            Ok(())
        } else {
            let writable: Vec<_> = self.writable.iter().cloned().collect();
            Err(format!(
                "Mem '{}' is read-only. Writable mems: {}",
                mem,
                writable.join(", ")
            ))
        }
    }
}

impl Default for MemRouterSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience: check if an entity is visible. Returns true if router is None (no filtering).
pub fn is_visible(entity_id: &EntityId, router: Option<&MemRouterSnapshot>) -> bool {
    match router {
        Some(r) => r.is_entity_visible(entity_id),
        None => true,
    }
}

/// Convenience: check if a mem is writable. Returns true if router is None.
pub fn is_writable(mem: &str, router: Option<&MemRouterSnapshot>) -> bool {
    match router {
        Some(r) => r.is_writable(mem),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_router_allows_nothing() {
        let router = MemRouterSnapshot::new();
        assert!(!router.is_writable("specs"));
        assert!(!router.is_visible("specs"));
    }

    #[test]
    fn writable_mem_is_visible() {
        let mut router = MemRouterSnapshot::new();
        router.add_writable(
            "specs".to_string(),
            Some(PathBuf::from("/path/to/specs")),
            MemOrigin::ExplicitToml,
        );
        assert!(router.is_writable("specs"));
        assert!(router.is_visible("specs"));
        assert!(!router.is_writable("other"));
    }

    #[test]
    fn read_only_mem() {
        let mut router = MemRouterSnapshot::new();
        router.add_read_only(
            "external".to_string(),
            PathBuf::from("/path/to/external.mem"),
        );
        assert!(!router.is_writable("external"));
        assert!(router.is_visible("external"));
    }

    #[test]
    fn entity_visibility() {
        let mut router = MemRouterSnapshot::new();
        router.add_writable(
            "specs".to_string(),
            Some(PathBuf::from("/specs")),
            MemOrigin::ExplicitToml,
        );
        router.add_read_only(
            "external".to_string(),
            PathBuf::from("/path/to/external.mem"),
        );

        assert!(router.is_entity_visible(&EntityId::new("specs", "entity")));
        assert!(router.is_entity_visible(&EntityId::new("external", "entity")));
        assert!(!router.is_entity_visible(&EntityId::new("hidden", "entity")));
    }

    #[test]
    fn dir_for_mem() {
        let mut router = MemRouterSnapshot::new();
        router.add_writable(
            "specs".to_string(),
            Some(PathBuf::from("/path/to/specs")),
            MemOrigin::ExplicitToml,
        );
        assert_eq!(
            router.dir_for_mem("specs"),
            Some(Path::new("/path/to/specs"))
        );
        assert_eq!(router.dir_for_mem("unknown"), None);
    }

    #[test]
    fn validate_writable_ok() {
        let mut router = MemRouterSnapshot::new();
        router.add_writable(
            "specs".to_string(),
            Some(PathBuf::from("/specs")),
            MemOrigin::ExplicitToml,
        );
        assert!(router.validate_writable("specs").is_ok());
    }

    #[test]
    fn validate_writable_err() {
        let mut router = MemRouterSnapshot::new();
        router.add_read_only(
            "external".to_string(),
            PathBuf::from("/path/to/external.mem"),
        );
        assert!(router.validate_writable("external").is_err());
    }

    #[test]
    fn convenience_functions_with_none() {
        let id = EntityId::new("any", "entity");
        assert!(is_visible(&id, None));
        assert!(is_writable("any", None));
    }

    #[test]
    fn archive_path_for_read_only_mem() {
        let mut router = MemRouterSnapshot::new();
        router.add_read_only("external".to_string(), PathBuf::from("/deps/external.mem"));
        assert_eq!(
            router.archive_path_for_mem("external"),
            Some(Path::new("/deps/external.mem"))
        );
        // Writable mems do not carry an archive path.
        router.add_writable(
            "specs".to_string(),
            Some(PathBuf::from("/specs")),
            MemOrigin::ExplicitToml,
        );
        assert_eq!(router.archive_path_for_mem("specs"), None);
        // Unknown mems return None cleanly.
        assert_eq!(router.archive_path_for_mem("unknown"), None);
    }

    #[test]
    fn dir_and_archive_paths_stay_separate() {
        // Deliberate check that `dir_for_mem` and `archive_path_for_mem`
        // don't leak into each other's keyspace — a writable mem must
        // never surface via the archive accessor, and vice versa.
        let mut router = MemRouterSnapshot::new();
        router.add_writable(
            "specs".to_string(),
            Some(PathBuf::from("/specs")),
            MemOrigin::ExplicitToml,
        );
        router.add_read_only("external".to_string(), PathBuf::from("/deps/external.mem"));
        assert!(router.dir_for_mem("specs").is_some());
        assert!(router.dir_for_mem("external").is_none());
        assert!(router.archive_path_for_mem("specs").is_none());
        assert!(router.archive_path_for_mem("external").is_some());
    }

    #[test]
    fn remove_writable_returns_true_when_present() {
        let mut router = MemRouterSnapshot::new();
        router.add_writable(
            "specs".to_string(),
            Some(PathBuf::from("/specs")),
            MemOrigin::ExplicitToml,
        );
        assert!(router.remove_writable("specs"));
        assert!(!router.is_writable("specs"));
        assert!(!router.is_visible("specs"));
        assert!(router.dir_for_mem("specs").is_none());
    }

    #[test]
    fn remove_writable_returns_false_when_absent() {
        let mut router = MemRouterSnapshot::new();
        assert!(!router.remove_writable("nonexistent"));
    }

    #[test]
    fn remove_writable_leaves_read_only_visibility_when_same_name_read_only_exists() {
        // Contrived: a name carried by both a writable entry and a
        // read-only archive. Not a current product state, but the
        // router's invariant ("visibility reflects union of registry
        // kinds") is worth locking in.
        let mut router = MemRouterSnapshot::new();
        router.add_writable(
            "shared".to_string(),
            Some(PathBuf::from("/specs")),
            MemOrigin::ExplicitToml,
        );
        router.add_read_only("shared".to_string(), PathBuf::from("/deps/shared.mem"));
        assert!(router.remove_writable("shared"));
        assert!(!router.is_writable("shared"));
        assert!(router.is_visible("shared"));
    }

    #[test]
    fn mem_origin_render_source_explicit() {
        let o = MemOrigin::ExplicitToml;
        // The config file lives at `.memstead/workspace.toml`.
        assert_eq!(o.render_source(), "explicit from .memstead/workspace.toml");
        assert_eq!(o.kind(), "explicit");
    }

    #[test]
    fn mem_origin_render_source_runtime_created() {
        // Anchor at a deterministic epoch offset so the rendered form
        // is stable: 1_700_000_000 = 2023-11-14T22:13:20Z.
        let ts = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let o = MemOrigin::RuntimeCreated {
            at: ts,
            by_tool: "memstead_mem_create",
        };
        let rendered = o.render_source();
        assert!(
            rendered.contains("memstead_mem_create"),
            "rendered source should name the tool: {rendered}"
        );
        assert!(
            rendered.contains("2023-11-14T22:13:20Z"),
            "rendered source should carry the RFC-3339 timestamp: {rendered}"
        );
        assert_eq!(o.kind(), "runtime_created");
    }

    #[test]
    fn snapshot_clone_is_independent() {
        // Locks the COW-snapshot discipline: a clone taken before a
        // mutation does not observe the mutation. This is the
        // invariant `Arc<MemRouterSnapshot>` relies on — readers
        // holding the pre-swap `Arc` see the pre-swap state.
        let mut original = MemRouterSnapshot::new();
        original.add_writable(
            "a".to_string(),
            Some(PathBuf::from("/a")),
            MemOrigin::ExplicitToml,
        );
        let pre_clone = original.clone();

        original.add_writable(
            "b".to_string(),
            Some(PathBuf::from("/b")),
            MemOrigin::ExplicitToml,
        );

        assert!(pre_clone.is_writable("a"));
        assert!(!pre_clone.is_writable("b"));
        assert!(original.is_writable("a"));
        assert!(original.is_writable("b"));
    }

    /// Hierarchical mem identity lives directly in the router HashMap
    /// key — `add_writable("team/sub-mem", …)` registers under the full
    /// path, lookups against `"sub-mem"` (the leaf alone) miss
    /// cleanly. Locks the "path is the only identifier" invariant.
    #[test]
    fn hierarchical_name_is_the_router_key() {
        let mut router = MemRouterSnapshot::new();
        router.add_writable(
            "team/sub-mem".to_string(),
            None,
            MemOrigin::ExplicitToml,
        );
        assert!(router.is_writable("team/sub-mem"));
        // Leaf-only lookup misses — there's no fallback path-lookup.
        assert!(!router.is_writable("sub-mem"));
    }
}
