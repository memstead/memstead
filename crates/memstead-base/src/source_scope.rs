//! Source scope enumeration — the declaration-layer walk that turns a
//! binding source's scope patterns (allow / deny globs over a medium,
//! entity selectors over a graph) plus a binding's ingest-level deny
//! paths into the set of artifacts the binding answers for, `S(D)`.
//!
//! Kernel, not loop: the anchors health axis enumerates a mem's source
//! artifacts to report the unclaimed ones, and the facet read surface
//! reports a scope's enumeration, so the walk lives beside the binding
//! store it reads and never behind the maintenance loop that also
//! consumes it. Moved here from `ingest::cursor` on 2026-09-10; the loop
//! reaches these items through `ingest::cursor`'s re-exports.

use std::path::{Component, Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::Engine;
use crate::pipeline::{MediumType, PatternMode, Source};

/// Lexically normalize a path — resolve `.` and `..` without touching the
/// filesystem (no symlink resolution), matching Node's `path.resolve` on an
/// already-absolute path.
pub(crate) fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

/// The relative path from `from` to `to` (both normalized), matching Node's
/// `path.relative`.
pub(crate) fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from = normalize_lexical(from);
    let to = normalize_lexical(to);
    let from_comps: Vec<Component> = from.components().collect();
    let to_comps: Vec<Component> = to.components().collect();
    let mut common = 0;
    while common < from_comps.len()
        && common < to_comps.len()
        && from_comps[common] == to_comps[common]
    {
        common += 1;
    }
    let mut result = PathBuf::new();
    for _ in common..from_comps.len() {
        result.push("..");
    }
    for comp in &to_comps[common..] {
        result.push(comp.as_os_str());
    }
    result
}

/// The medium pointer resolved to an absolute base directory. Public
/// so init-time surfaces (CLI `projection init`) can resolve a medium
/// base exactly as the strategies do — e.g. to warn when it falls
/// outside the workspace root.
pub fn medium_base(pointer: &str, workspace_root: &Path) -> PathBuf {
    if pointer.is_empty() {
        workspace_root.to_path_buf()
    } else {
        normalize_lexical(&workspace_root.join(pointer))
    }
}

/// Workspace-relative deny globs excluding the engine's own state from
/// every strategy's input set. Unconditional and non-configurable: a
/// binding can never legitimately model `.memstead/`,
/// `.memstead.cache/`, or a mount's resolved storage location as
/// source artifacts — an allow glob covering them does not admit them.
/// The dot-directories key on their *names* (the names are the
/// contract, and a foreign workspace's `.memstead/` is still engine
/// state); the mount storage locations key on their *resolved* paths
/// because their directory names are configurable. Fail-open on an
/// unreadable mount list: the name-based excludes stay in force.
pub(crate) fn engine_state_denies(workspace_root: &Path) -> Vec<String> {
    use crate::workspace_store::{FileWorkspaceStore, WorkspaceStoreAdapter};

    let mut denies: Vec<String> = vec![
        ".memstead/**".to_string(),
        ".memstead.cache/**".to_string(),
        "**/.memstead/**".to_string(),
        "**/.memstead.cache/**".to_string(),
    ];
    if let Ok(ws) = FileWorkspaceStore.load(workspace_root) {
        for mount in &ws.mounts {
            let dir: Option<PathBuf> = match &mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => {
                    gitdir.parent().map(Path::to_path_buf)
                }
                crate::workspace::MountStorage::Folder { path } => Some(path.clone()),
                crate::workspace::MountStorage::Archive { path, .. } => {
                    // A sealed archive is one file, not a tree.
                    let rel = relative_path(workspace_root, &normalize_lexical(path));
                    denies.push(rel.to_string_lossy().to_string());
                    None
                }
                // No on-disk footprint to exclude.
                crate::workspace::MountStorage::InMemory => None,
            };
            if let Some(dir) = dir {
                let rel = relative_path(workspace_root, &normalize_lexical(&dir));
                // A collapsed single-mem folder workspace stores the mem
                // AT the workspace root — excluding `**` there would
                // empty every denominator; skip it.
                if !rel.as_os_str().is_empty() {
                    denies.push(format!("{}/**", rel.to_string_lossy()));
                }
            }
        }
    }
    denies
}

/// Build a [`GlobSet`] from glob patterns, or `None` if any pattern is
/// malformed. The namespace the patterns are written in is the caller's
/// business — scope patterns are source-relative, ingest denies
/// workspace-relative.
pub(crate) fn build_glob_set(patterns: &[&str]) -> Option<GlobSet> {
    build_glob_set_reporting(patterns).0
}

/// [`build_glob_set`], plus the patterns that would not compile.
///
/// All-or-nothing was the old contract and it degraded silently in both
/// directions: one malformed allow emptied the whole enumeration, and one
/// malformed deny disabled every deny and inflated the set. Now the valid
/// patterns still compile and the malformed ones come back by name, so the
/// caller can state the partiality instead of computing over it.
/// `validate_binding` refuses a malformed scope pattern outright; this path
/// carries records written before that gate existed.
pub(crate) fn build_glob_set_reporting(patterns: &[&str]) -> (Option<GlobSet>, Vec<String>) {
    let mut builder = GlobSetBuilder::new();
    let mut malformed = Vec::new();
    let mut any = false;
    for pattern in patterns {
        match Glob::new(pattern) {
            Ok(g) => {
                builder.add(g);
                any = true;
            }
            Err(_) => malformed.push((*pattern).to_string()),
        }
    }
    let set = if any { builder.build().ok() } else { None };
    (set, malformed)
}

/// Enumerate the workspace-relative file paths a primary source's facet scope
/// selects — the `mtime` strategy's input set. Mirrors the plugin's
/// `enumerateFacetFiles`: the path-shaped mediums (`codebase` / `filesystem` /
/// `git` — a git source's artifacts are paths pinned at a commit, so the walk
/// is identical and only the anchor namespace differs); the facet's
/// allow globs minus its deny globs, evaluated over the medium's directory
/// tree. Returns a sorted, de-duplicated list. An unscoped facet (no allows)
/// yields an empty list here — but callers must not treat that as signal: the
/// strategy layer (`compute_mtime_slice` / `current_primary_token`) refuses
/// an unscoped facet via `facet_unscoped` *before* enumerating, so the empty
/// list is only ever reached for a genuinely-empty scoped enumeration.
///
/// ## Scope patterns are SOURCE-relative
///
/// A facet's own `scope` patterns resolve against the source's **pointer** —
/// the same base the walk starts from, and the same convention the anchor
/// artifact decision already ratified for every other surface around a
/// binding. It is also what the rendered brief teaches, printing the pointer
/// and the allow list directly beneath it.
///
/// Until 2026-08-27 the walk honoured the pointer and then matched each
/// candidate by its *workspace*-relative path, with no join. The two readings
/// coincide only for an empty pointer (the shape the scaffolder writes, and
/// the only shape the enumeration test covered), which is why the defect stayed
/// invisible: under a non-empty pointer a prefix-anchored pattern or a bare
/// literal matched nothing, a `**`-prefixed pattern matched regardless, and a
/// scope mixing the two produced a silently truncated denominator that every
/// coverage figure was then computed over. [`scope_migration_notes`] names the
/// patterns a binding still has in the old dialect.
///
/// `deny_paths` are the ingest-level denies (`ResolvedIngest::deny_paths`),
/// applied on top of the facet's own scope denies. These stay
/// **workspace-relative** — deliberately, and it is not a second dialect for
/// the same thing: an ingest deny spans every source in the binding, so it has
/// no single pointer to be relative to, and the git strategy pushes the same
/// entries down as workspace-rooted `:(glob,exclude)` pathspecs. Scope is
/// per-source and source-relative; ingest denies are per-binding and
/// workspace-relative. Passing `&[]` yields the facet-scope-only behaviour.
pub fn enumerate_facet_files(
    source: &Source,
    deny_paths: &[String],
    workspace_root: &Path,
) -> Vec<String> {
    enumerate_facet_files_reported(source, deny_paths, workspace_root).files
}

/// [`enumerate_facet_files`], carrying what the enumeration could not speak
/// for: patterns that would not compile, and patterns still in the retired
/// workspace-relative dialect. Any surface that reduces the set to a figure
/// must consult [`ScopeEnumeration::is_partial`] first.
pub fn enumerate_facet_files_reported(
    source: &Source,
    deny_paths: &[String],
    workspace_root: &Path,
) -> ScopeEnumeration {
    if !matches!(
        source.medium_type,
        MediumType::Codebase | MediumType::Filesystem | MediumType::Git
    ) {
        return ScopeEnumeration::default();
    }
    let legacy_dialect = scope_migration_notes(source);
    let mut allows: Vec<&str> = Vec::new();
    let mut scope_denies: Vec<&str> = Vec::new();
    for rule in &source.scope {
        match rule.mode {
            PatternMode::Allow => allows.push(&rule.path),
            PatternMode::Deny => scope_denies.push(&rule.path),
        }
    }
    // Ingest deny_paths deny on top of the facet's own denies, in the
    // workspace-relative grammar — the same entries the git strategy resolves
    // as exclude pathspecs, so deny enforcement is strategy-invariant. They are
    // matched against the candidate's workspace-relative path, NOT its
    // source-relative one: an ingest deny spans every source in the binding.
    let mut ws_denies: Vec<&str> = deny_paths.iter().map(String::as_str).collect();
    // Engine self-exclusion — unconditional, below configuration; the
    // git strategy pushes the same set as exclude pathspecs so the
    // denominator stays strategy-invariant.
    let forced = engine_state_denies(workspace_root);
    for f in &forced {
        ws_denies.push(f);
    }
    if allows.is_empty() {
        return ScopeEnumeration {
            legacy_dialect,
            ..Default::default()
        };
    }
    // Malformed patterns no longer take the whole set with them: the valid
    // ones compile, the bad ones come back by name, and the caller states the
    // partiality. A malformed ALLOW leaves the denominator short; a malformed
    // DENY leaves it long. Either way the figure over it is not the answer.
    let (allow_set, mut malformed) = build_glob_set_reporting(&allows);
    let (scope_deny_set, deny_malformed) = build_glob_set_reporting(&scope_denies);
    malformed.extend(deny_malformed);
    // The ingest-level denies go through the SAME resolver the path-check
    // command answers with — glob plus the literal-base directory-prefix rule
    // plus the malformed-entry fallback — so a path denied at the hook can
    // never be counted in the denominator. Sharing the glob library alone left
    // the two disagreeing on exactly those extra rules.
    let ws_oracle = crate::check_path::DenyOracle::new(
        &ws_denies
            .iter()
            .map(|d| (*d).to_string())
            .collect::<Vec<_>>(),
    );
    let Some(allow_set) = allow_set else {
        return ScopeEnumeration {
            malformed,
            legacy_dialect,
            ..Default::default()
        };
    };

    // Walk the medium's directory tree. A candidate is matched twice, in two
    // namespaces: the facet's own scope patterns against its SOURCE-relative
    // path (relative to the medium base below), the ingest-level denies
    // against its workspace-relative one. The returned paths stay
    // workspace-relative — that is the namespace every consumer of this list
    // speaks. VCS internals are never source artifacts — they are pruned here
    // so `.git/**` plumbing cannot enter `S(D)`, matching the git strategy
    // (whose diffs never name `.git` files).
    let base = medium_base(&source.pointer, workspace_root);
    // Directory pruning by allow-prefix: a directory is entered only when
    // some allow pattern could still match beneath it. Each pattern
    // contributes its longest literal leading segment run (up to the first
    // segment carrying a glob metacharacter); a directory whose segments
    // diverge from every pattern's literal prefix can contain no match, so
    // the walk never descends into it. A pattern that begins with a glob
    // segment (`**/*.rs`) contributes the empty prefix and keeps the full
    // walk — behaviour is unchanged for unanchored scopes. This is what
    // keeps a broad-pointer facet (`..` with `dev/**/*.md`) from stat-walking
    // every build tree and node_modules in the repository on each
    // enumeration (status tokens, verify S(D), sync briefs all pass here).
    let allow_prefixes: Vec<Vec<String>> = allows.iter().map(|p| glob_literal_prefix(p)).collect();
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![base.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                let skip = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    // VCS internals and engine state are never source
                    // artifacts — pruning here saves the walk; the
                    // forced deny globs enforce the same exclusion for
                    // anything that still slips into a candidate list.
                    VCS_INTERNAL_DIRS.contains(&n) || n == ".memstead" || n == ".memstead.cache"
                });
                if !skip {
                    let dir_src_rel = relative_path(&base, &normalize_lexical(&path))
                        .to_string_lossy()
                        .to_string();
                    if allow_could_match_under(&allow_prefixes, &dir_src_rel) {
                        stack.push(path);
                    }
                }
            } else if file_type.is_file() {
                let normalized = normalize_lexical(&path);
                let rel = relative_path(workspace_root, &normalized)
                    .to_string_lossy()
                    .to_string();
                let src_rel = relative_path(&base, &normalized)
                    .to_string_lossy()
                    .to_string();
                let denied = scope_deny_set
                    .as_ref()
                    .is_some_and(|d| d.is_match(&src_rel))
                    || ws_oracle.is_denied(&rel);
                if allow_set.is_match(&src_rel) && !denied {
                    out.push(rel);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    ScopeEnumeration {
        files: out,
        malformed,
        legacy_dialect,
    }
}

/// The longest run of leading path segments in a glob pattern that carry no
/// glob metacharacter — the literal region a match must live under. `dev/**/
/// *.md` → `["dev"]`; `VISION.md` → `["VISION.md"]`; `**/*.rs` → `[]`.
pub(crate) fn glob_literal_prefix(pattern: &str) -> Vec<String> {
    pattern
        .split('/')
        .take_while(|seg| {
            !seg.chars()
                .any(|c| matches!(c, '*' | '?' | '[' | ']' | '{' | '}'))
        })
        .map(str::to_string)
        .collect()
}

/// Whether any allow pattern could match a file somewhere under the
/// directory at `dir_rel` (source-relative). True when, for some pattern's
/// literal prefix, the directory's segments and the prefix agree over their
/// common length: the directory is either an ancestor of the literal region
/// (the walk must pass through it) or inside the pattern's glob region. An
/// empty prefix (pattern starts with a glob segment) matches every
/// directory — no pruning for unanchored scopes.
pub(crate) fn allow_could_match_under(prefixes: &[Vec<String>], dir_rel: &str) -> bool {
    let dir_segs: Vec<&str> = dir_rel.split('/').filter(|s| !s.is_empty()).collect();
    prefixes.iter().any(|prefix| {
        let n = dir_segs.len().min(prefix.len());
        dir_segs[..n]
            .iter()
            .zip(prefix[..n].iter())
            .all(|(d, p)| *d == p.as_str())
    })
}

/// What an enumeration of a source's scope selected, and what it could not
/// speak for.
///
/// The campaign rule this serves: a surface does not report a figure over
/// state it did not examine. A coverage percentage computed over a denominator
/// that silently lost patterns is exactly that figure, so the partiality
/// travels with the set rather than being discarded at the seam.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeEnumeration {
    /// The workspace-relative artifact paths the scope selected.
    pub files: Vec<String>,
    /// Scope patterns that would not compile, by name. Their share of the
    /// population is unknown, so any enumeration carrying one is partial.
    pub malformed: Vec<String>,
    /// Scope patterns still written in the retired workspace-relative
    /// dialect — reported so an author is told before a pattern's meaning
    /// changes, never after.
    pub legacy_dialect: Vec<ScopePatternNote>,
}

impl ScopeEnumeration {
    /// Whether this enumeration is known to be incomplete. A partial
    /// enumeration must not be reduced to a percentage: the denominator is
    /// not the population.
    ///
    /// BOTH causes count. A malformed pattern was skipped, so its share never
    /// entered the walk; a pattern still in the retired workspace-relative
    /// dialect selects nothing under the pointer join, so its share is
    /// likewise absent. The second is the more dangerous of the two, because a
    /// MIXED scope still enumerates and the surviving subset looks like a
    /// population.
    pub fn is_partial(&self) -> bool {
        !self.malformed.is_empty() || !self.legacy_dialect.is_empty()
    }

    /// Why this enumeration is incomplete, naming the offending patterns, or
    /// `None` when it is whole.
    pub fn partiality_reason(&self) -> Option<String> {
        let mut causes = Vec::new();
        if !self.malformed.is_empty() {
            causes.push(format!(
                "scope pattern(s) that would not compile were skipped: {}",
                self.malformed.join(", ")
            ));
        }
        if !self.legacy_dialect.is_empty() {
            causes.push(format!(
                "scope pattern(s) are still written against the workspace root rather than the \
                 source pointer, so they select nothing under the pointer join: {}",
                self.legacy_dialect
                    .iter()
                    .map(|n| n.pattern.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        (!causes.is_empty()).then(|| causes.join("; "))
    }
}

/// One scope pattern still written in the retired workspace-relative dialect.
///
/// Emitted by [`scope_migration_notes`] so a binding author is told BEFORE a
/// pattern's meaning changes, never after. The campaign rule this serves is
/// the plain one: a surface does not quietly begin covering a different set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopePatternNote {
    /// The pattern as written in the binding record.
    pub pattern: String,
    /// Whether it is an allow or a deny rule.
    pub deny: bool,
    /// The pattern rewritten into the source-relative dialect, when the
    /// rewrite is mechanical (the pattern begins with the source's pointer).
    /// `None` when it is not — the author has to decide what they meant.
    pub suggested: Option<String>,
}

/// Name every scope pattern of `source` that reads as the retired
/// workspace-relative dialect: one that begins with the source's own pointer,
/// so it selected artifacts before the 2026-08-27 join and selects nothing
/// after it.
///
/// A pointer-less source has no dialect to migrate (the two readings coincide
/// — which is exactly why the defect stayed invisible on the scaffolded path),
/// so it returns empty. A `**`-prefixed pattern is prefix-free and matched
/// under both readings, so it is not reported.
pub fn scope_migration_notes(source: &Source) -> Vec<ScopePatternNote> {
    let pointer = source.pointer.trim_end_matches('/');
    if pointer.is_empty() {
        return Vec::new();
    }
    let prefix = format!("{pointer}/");
    source
        .scope
        .iter()
        .filter(|r| !r.path.starts_with("**"))
        .filter_map(|rule| {
            let stripped = rule.path.strip_prefix(&prefix)?;
            Some(ScopePatternNote {
                pattern: rule.path.clone(),
                deny: rule.mode == PatternMode::Deny,
                suggested: (!stripped.is_empty()).then(|| stripped.to_string()),
            })
        })
        .collect()
}

/// One parsed entry of a **graph** facet's scope — the entity-namespace
/// counterpart of a path glob. A graph source selects entities, and an entity
/// is not a path: matching id-shaped globs against `mem--slug` invites the
/// "looks scoped, selects nothing" failure the dead-deny lint exists to catch
/// on paths, so the vocabulary is explicit about which axis it selects on.
///
/// Grammar (the whole of it):
///
/// - `*` — every entity in the source mem
/// - `type:<entity_type>` — entities of exactly that type
/// - `id:<glob>` — entities whose full `mem--slug` id matches the glob
///
/// Anything else is refused at binding validation
/// ([`crate::binding::validate_binding`]) rather than silently selecting
/// nothing: a scope nothing interprets is the defect, not a permissible form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitySelector {
    /// `*` — every entity in the mem.
    All,
    /// `type:<entity_type>` — exact type match.
    Type(String),
    /// `id:<glob>` — glob over the full entity id.
    Id(String),
}

/// Parse one graph scope pattern. `None` for an unrecognised form — the
/// caller decides whether that is a validation refusal (declaration time) or
/// a skipped rule (run time, already refused at declaration).
pub fn parse_entity_selector(pattern: &str) -> Option<EntitySelector> {
    let pattern = pattern.trim();
    if pattern == "*" {
        return Some(EntitySelector::All);
    }
    if let Some(rest) = pattern.strip_prefix("type:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        return Some(EntitySelector::Type(rest.to_string()));
    }
    if let Some(rest) = pattern.strip_prefix("id:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        // A malformed glob is a refusal, not a rule that matches nothing.
        Glob::new(rest).ok()?;
        return Some(EntitySelector::Id(rest.to_string()));
    }
    None
}

/// Does `selector` select this entity?
pub(crate) fn selector_matches(selector: &EntitySelector, id: &str, entity_type: &str) -> bool {
    match selector {
        EntitySelector::All => true,
        EntitySelector::Type(t) => entity_type == t,
        EntitySelector::Id(g) => Glob::new(g)
            .ok()
            .map(|glob| glob.compile_matcher().is_match(id))
            .unwrap_or(false),
    }
}

/// Enumerate the entity ids a **graph** source's facet scope selects — the
/// graph medium's `S(D)`, the exact counterpart of [`enumerate_facet_files`]
/// for a path medium. The source's `pointer` names the source mem; the store
/// already holds every mounted mem's entities, so this is a filter over
/// memory rather than any kind of walk.
///
/// Stubs are excluded: a stub is a placeholder the engine created for an
/// unresolved reference, not an authored source artifact. Counting them would
/// inflate the denominator with entities the source never wrote, making
/// coverage look worse than it is for a reason no author can act on.
///
/// An unscoped facet (no allow rules) yields an empty list here — callers must
/// not read that as "nothing in scope"; the strategy layer refuses an unscoped
/// facet before reaching this, exactly as it does for the path mediums.
pub fn enumerate_graph_entities(engine: &Engine, source: &Source) -> Vec<String> {
    if source.medium_type != MediumType::Graph {
        return Vec::new();
    }
    let mut allows: Vec<EntitySelector> = Vec::new();
    let mut denies: Vec<EntitySelector> = Vec::new();
    for rule in &source.scope {
        // An unparseable rule is already a validation refusal; at run time it
        // selects nothing rather than everything — a scope the engine cannot
        // read must never widen reach.
        let Some(sel) = parse_entity_selector(&rule.path) else {
            continue;
        };
        match rule.mode {
            PatternMode::Allow => allows.push(sel),
            PatternMode::Deny => denies.push(sel),
        }
    }
    if allows.is_empty() {
        return Vec::new();
    }
    let mem = source.pointer.as_str();
    let mut out: Vec<String> = Vec::new();
    for entity in engine.store().all_entities() {
        if entity.mem != mem || entity.stub {
            continue;
        }
        let id = entity.id.0.as_str();
        let ty = entity.entity_type.as_str();
        if !allows.iter().any(|s| selector_matches(s, id, ty)) {
            continue;
        }
        if denies.iter().any(|s| selector_matches(s, id, ty)) {
            continue;
        }
        out.push(id.to_string());
    }
    out.sort();
    out.dedup();
    out
}

/// Enumerate one primary source's in-scope artifacts, whatever its medium —
/// the single entry point every `S(D)` consumer uses. Path-shaped mediums
/// (codebase / filesystem / git) walk the file tree; a graph source filters
/// the source mem's entities. A medium the matrix marks non-enumerable
/// yields nothing, and its callers render the non-enumerable basis rather
/// than a denominator.
///
/// This exists because the enumeration bail was never in one place: five call
/// sites each repeated the same loop over `enumerate_facet_files`, so teaching
/// only the report about a new medium left the findings store, the refinement
/// rotation, and the exclude membership gate empty-handed.
pub fn enumerate_source_artifacts(
    engine: &Engine,
    source: &Source,
    deny_paths: &[String],
    workspace_root: &Path,
) -> Vec<String> {
    enumerate_source_artifacts_reported(engine, source, deny_paths, workspace_root).files
}

/// [`enumerate_source_artifacts`], carrying what the enumeration could not
/// speak for. Any surface reducing `S(D)` to a figure consults
/// [`ScopeEnumeration::is_partial`] first.
pub fn enumerate_source_artifacts_reported(
    engine: &Engine,
    source: &Source,
    deny_paths: &[String],
    workspace_root: &Path,
) -> ScopeEnumeration {
    match source.medium_type {
        MediumType::Codebase | MediumType::Filesystem | MediumType::Git => {
            enumerate_facet_files_reported(source, deny_paths, workspace_root)
        }
        // Entity selectors are their own grammar with their own parser — a
        // graph scope has no glob to malform and no pointer dialect to migrate.
        MediumType::Graph => ScopeEnumeration {
            files: enumerate_graph_entities(engine, source),
            ..Default::default()
        },
        MediumType::Web => ScopeEnumeration::default(),
    }
}

/// VCS metadata directories — never source artifacts. Pruned from source
/// enumeration (`S(D)`, mtime slices, advance) and from the dead-deny scan.
pub(crate) const VCS_INTERNAL_DIRS: &[&str] = &[".git", ".svn", ".hg"];
