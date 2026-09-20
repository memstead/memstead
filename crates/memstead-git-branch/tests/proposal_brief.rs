//! `Engine::proposal_brief` on the git-branch backend: a fork's changes
//! against its source, three ways (plan-proposal 02).
//!
//! The fixture stages the six shapes on one fork: an entity added
//! (clean), an entity added that fails the target's write gate, an
//! entity modified in two sections and one metadata field (with a url
//! span anchor row and referrers in two mems), an entity deleted, an
//! entity renamed, an entity in conflict by modification (both sides
//! changed the same section; a self-link in another section is never a
//! difference), an entity in conflict by deletion, and an entity the
//! target created under the slug the fork added. Every refusal lands
//! nothing, and a render leaves both branches and the workspace state
//! byte-identical.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use memstead_base::anchor::AnchorInput;
use memstead_base::backend::MemBackend;
use memstead_base::mem_management::{self, MemForkParams};
use memstead_base::ops::proposal::{ConflictKind, PrecheckOutcome, ProposalChange};
use memstead_base::vcs::Actor;
use memstead_base::{
    CreateEntityArgs, DeleteEntityArgs, EntityId, RenameEntityArgs, UpdateEntityArgs,
};
use memstead_git_branch::mem_repo_config::commit_config_at_gitdir;
use memstead_git_branch::ops::transport::resolve_ref_in_gitdir;
use memstead_git_branch::storage::git_tree::GitTreeBackend;
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::vcs::CommitContext;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

const WORKSPACE_HEAD: &str = "format = \"memstead-git-branch-2\"\n\n\
     [persistence_adapter]\nname = \"file-two-layer\"\n\n";

fn gitdir_of(root: &Path) -> PathBuf {
    root.join("mem-repo").join(".git").canonicalize().unwrap()
}

fn sha_of(gitdir: &Path, ref_name: &str) -> Option<String> {
    resolve_ref_in_gitdir(gitdir, ref_name).unwrap()
}

fn sections(identity: &str, purpose: &str) -> indexmap::IndexMap<String, String> {
    let mut s = indexmap::IndexMap::new();
    s.insert("identity".to_string(), identity.to_string());
    s.insert("purpose".to_string(), purpose.to_string());
    s
}

fn create(
    engine: &mut memstead_base::Engine,
    mem: &str,
    title: &str,
    sections: indexmap::IndexMap<String, String>,
    anchors: Vec<AnchorInput>,
) {
    engine
        .create_entity(
            CreateEntityArgs {
                anchors,
                mem: mem.to_string(),
                title: title.to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("create {title:?} in {mem:?}: {e:?}"));
}

fn update_sections(
    engine: &mut memstead_base::Engine,
    id: (&str, &str),
    sections: &[(&str, &str)],
    metadata: &[(&str, &str)],
) {
    engine
        .update_entity(
            UpdateEntityArgs {
                id: EntityId::new(id.0, id.1),
                expected_hash: None,
                sections: sections
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                append_sections: Default::default(),
                patch_sections: Default::default(),
                sections_unset: Vec::new(),
                metadata: metadata
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                metadata_unset: Vec::new(),
                dry_run: false,
                declare_relations: Vec::new(),
                anchors: Vec::new(),
                anchors_unset: Vec::new(),
                relations_unset: Vec::new(),
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("update {id:?}: {e:?}"));
}

fn relate(engine: &mut memstead_base::Engine, from: (&str, &str), to: (&str, &str)) {
    engine
        .relate_entity(
            memstead_base::RelateEntityArgs {
                source: EntityId::new(from.0, from.1),
                expected_hash: None,
                rel_type: "DEPENDS_ON".to_string(),
                target: EntityId::new(to.0, to.1),
                remove: false,
                description: None,
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("relate {from:?} -> {to:?}: {e:?}"));
}

fn delete(engine: &mut memstead_base::Engine, id: (&str, &str)) {
    engine
        .delete_entity(
            DeleteEntityArgs {
                id: EntityId::new(id.0, id.1),
                expected_hash: None,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("delete {id:?}: {e:?}"));
}

/// Every file under `.memstead/` of the workspace root, path to bytes:
/// the workspace state a render must leave untouched.
fn workspace_state(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&root.join(".memstead"), root, &mut out);
    out
}

/// The staged fixture: the source `specs`, a peer `plans` holding a
/// referrer, the fork `specs-fork` with the six shapes, and `orphan`,
/// a mem whose recorded source is not mounted. Returns the workspace
/// and the engine, booted once so every side change was made through
/// the engine (the sibling-writer path being the one exception: the
/// gate-failing entity, which no engine would write).
fn staged() -> (TempDir, memstead_base::Engine) {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/workspace.toml"),
        format!("{WORKSPACE_HEAD}[cross_mem_links]\nspecs = [\"plans\"]\nplans = [\"specs\"]\n"),
    )
    .unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[
            ("specs", "default@1.0.0"),
            ("plans", "default@1.0.0"),
            ("orphan", "default@1.0.0"),
        ],
    );
    let gitdir = gitdir_of(tmp.path());
    // `orphan` claims a source that is not mounted.
    commit_config_at_gitdir(
        &gitdir,
        "orphan",
        format!(
            r#"{{"schema": "default@1.0.0", "forkedFrom": {{"mem": "ghost", "sha": "{}"}}}}"#,
            "a".repeat(40)
        )
        .as_bytes(),
        &CommitContext::internal(),
        "orphan claims a ghost",
    )
    .unwrap();
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    // The source at the ancestor.
    create(
        &mut engine,
        "plans",
        "Roadmap",
        sections("the roadmap", "seed"),
        Vec::new(),
    );
    create(
        &mut engine,
        "specs",
        "Alpha",
        sections("alpha stands", "seed"),
        Vec::new(),
    );
    create(
        &mut engine,
        "specs",
        "Beta",
        sections("beta as the source has it", "beta's purpose"),
        vec![AnchorInput {
            artifact: Some("https://example.org/page".to_string()),
            grain: Some("url".to_string()),
            class: Some("anchored".to_string()),
            span: Some("the quoted words".to_string()),
            ..Default::default()
        }],
    );
    create(
        &mut engine,
        "specs",
        "Gamma",
        sections("gamma stands", "seed"),
        Vec::new(),
    );
    create(
        &mut engine,
        "specs",
        "Delta",
        sections("delta stands", "seed"),
        Vec::new(),
    );
    let mut epsilon = sections("epsilon as the source has it", "seed");
    epsilon.insert(
        "specifies".to_string(),
        "- rests on [[specs--alpha]] and [[specs:alpha|alpha]]".to_string(),
    );
    create(&mut engine, "specs", "Epsilon", epsilon, Vec::new());
    create(
        &mut engine,
        "specs",
        "Zeta",
        sections("zeta stands", "seed"),
        Vec::new(),
    );
    // Kappa carries a self-link and is touched by nobody after the
    // fork: only the fork commit's retargeting moves its bytes.
    create(
        &mut engine,
        "specs",
        "Kappa",
        sections("kappa rests on [[specs--alpha]]", "seed"),
        Vec::new(),
    );
    relate(&mut engine, ("specs", "alpha"), ("specs", "beta"));
    relate(&mut engine, ("plans", "roadmap"), ("specs", "beta"));

    // The fork.
    mem_management::fork_mem(
        &mut engine,
        MemForkParams {
            source: "specs".to_string(),
            sha: None,
            name: "specs-fork".to_string(),
            remote: None,
            note: None,
            operator_mode: true,
            actor: Actor::Cli,
            client: None,
        },
    )
    .expect("fork lands");

    // The fork's changes: modified (two sections, one field), deleted,
    // renamed, modified-to-be-conflicted twice, added clean, added.
    update_sections(
        &mut engine,
        ("specs-fork", "beta"),
        &[
            ("identity", "beta as the proposer has it"),
            ("purpose", "beta's purpose, sharpened"),
        ],
        &[("level", "M1")],
    );
    delete(&mut engine, ("specs-fork", "gamma"));
    engine
        .rename_entity(
            RenameEntityArgs {
                id: EntityId::new("specs-fork", "delta"),
                expected_hash: None,
                new_title: "Delta Prime".to_string(),
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("rename lands");
    update_sections(
        &mut engine,
        ("specs-fork", "epsilon"),
        &[("identity", "epsilon as the proposer has it")],
        &[],
    );
    update_sections(
        &mut engine,
        ("specs-fork", "zeta"),
        &[("identity", "zeta as the proposer has it")],
        &[],
    );
    create(
        &mut engine,
        "specs-fork",
        "Eta",
        sections("eta is new", "a new card"),
        Vec::new(),
    );
    create(
        &mut engine,
        "specs-fork",
        "Iota",
        sections("iota from the fork", "seed"),
        Vec::new(),
    );
    // Theta fails the gate: `level` outside the enum. No engine writes
    // it, so it lands as a sibling writer's commit on the fork branch.
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs-fork".to_string());
    writer
        .write_entity(
            Path::new("theta.md"),
            b"---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M9\n---\n# Theta\n\n## Identity\n\ntheta claims\n\n## Purpose\n\nseed\n",
        )
        .unwrap();
    writer
        .commit("theta by a sibling writer", &CommitContext::internal())
        .unwrap();

    // The target's changes meanwhile: the same section of epsilon,
    // zeta deleted, its own iota.
    update_sections(
        &mut engine,
        ("specs", "epsilon"),
        &[("identity", "epsilon as the owner now has it")],
        &[],
    );
    delete(&mut engine, ("specs", "zeta"));
    create(
        &mut engine,
        "specs",
        "Iota",
        sections("iota from the owner", "seed"),
        Vec::new(),
    );

    (tmp, engine)
}

// ---------------------------------------------------------------------
// AC1: the brief renders the staged fork's changes three ways
// ---------------------------------------------------------------------

/// The four shas, one block per touched entity and none for the
/// untouched one, the differing sections, the referrers, the anchor
/// row under the fork's id with its span and no fetched observation,
/// the precheck outcomes with typed codes, the three conflict kinds
/// with the three versions, the disposition skeleton, and byte
/// identity of both branches and the workspace state after the render.
#[test]
fn brief_renders_the_six_shapes_three_ways_and_writes_nothing() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    let before: Vec<Option<String>> = ["specs", "specs-fork", "plans", "__MEMSTEAD"]
        .iter()
        .map(|r| sha_of(&gitdir, &format!("refs/heads/{r}")))
        .collect();
    let state_before = workspace_state(tmp.path());
    let forked_from = engine
        .mem_config_for("specs-fork")
        .and_then(|c| c.forked_from.clone())
        .expect("the fork records its origin");

    let brief = engine
        .proposal_brief("specs-fork")
        .expect("the brief renders");

    // The shas a merge pins.
    assert_eq!(brief.fork, "specs-fork");
    assert_eq!(brief.target, "specs");
    assert_eq!(brief.ancestor, forked_from.sha);
    assert_eq!(brief.base, forked_from.base.clone().unwrap());
    assert!(!brief.base_is_ancestor);
    assert_eq!(Some(brief.fork_tip.clone()), before[1]);
    assert_eq!(Some(brief.target_tip.clone()), before[0]);
    assert_eq!(brief.proposal_id, format!("specs-fork@{}", brief.base));

    // One entry per touched entity, by slug, sorted; alpha untouched.
    let slugs: Vec<&str> = brief.entries.iter().map(|e| e.slug.as_str()).collect();
    assert_eq!(
        slugs,
        vec![
            "beta",
            "delta-prime",
            "epsilon",
            "eta",
            "gamma",
            "iota",
            "theta",
            "zeta"
        ]
    );
    let by_slug: BTreeMap<&str, &memstead_base::ops::ProposalEntry> =
        brief.entries.iter().map(|e| (e.slug.as_str(), e)).collect();

    // (a) modified: two sections and one field differ, referrers in
    // two mems, the url row under the fork's id with its span and no
    // observation, precheck clean as an update.
    let beta = by_slug["beta"];
    assert_eq!(beta.status, ProposalChange::Modified);
    assert_eq!(beta.fork_id, "specs-fork--beta");
    assert_eq!(beta.target_id, "specs--beta");
    let delta = beta.fork_delta.as_ref().unwrap();
    assert_eq!(delta.sections, vec!["identity", "purpose"]);
    assert_eq!(delta.metadata, vec!["level"]);
    assert!(!delta.title_differs && !delta.relationships_differ);
    let referrers: Vec<(String, String)> = beta
        .referrers
        .iter()
        .map(|r| (r.id.clone(), r.rel_type.clone()))
        .collect();
    assert_eq!(
        referrers,
        vec![
            ("plans--roadmap".to_string(), "DEPENDS_ON".to_string()),
            ("specs--alpha".to_string(), "DEPENDS_ON".to_string()),
        ]
    );
    assert_eq!(beta.anchors.len(), 1, "{:?}", beta.anchors);
    assert_eq!(beta.anchors[0].artifact, "https://example.org/page");
    assert_eq!(beta.anchors[0].grain, "url");
    assert_eq!(beta.anchors[0].class, "anchored");
    assert_eq!(beta.anchors[0].span.as_deref(), Some("the quoted words"));
    assert_eq!(
        beta.anchors[0].last_state, None,
        "no observation is fetched"
    );
    assert_eq!(beta.precheck.outcome, PrecheckOutcome::Clean);
    assert_eq!(beta.precheck.operation.as_deref(), Some("update"));
    assert!(beta.conflict.is_none());
    assert!(beta.content_hash.is_some());
    assert!(
        beta.base_body
            .as_deref()
            .unwrap()
            .contains("beta as the source has it")
    );
    assert!(
        beta.fork_body
            .as_deref()
            .unwrap()
            .contains("beta as the proposer has it")
    );

    // (c) added and mechanically clean, as a create.
    let eta = by_slug["eta"];
    assert_eq!(eta.status, ProposalChange::Added);
    assert_eq!(eta.precheck.outcome, PrecheckOutcome::Clean);
    assert_eq!(eta.precheck.operation.as_deref(), Some("create"));
    assert!(eta.base_body.is_none() && eta.fork_delta.is_none());
    assert!(eta.referrers.is_empty());

    // Added and refused by the gate: the typed code, never a content
    // judgement.
    let theta = by_slug["theta"];
    assert_eq!(theta.status, ProposalChange::Added);
    assert_eq!(theta.precheck.outcome, PrecheckOutcome::Failed);
    assert_eq!(theta.precheck.failures.len(), 1);
    assert_eq!(theta.precheck.failures[0].code, "INVALID_ENUM_VALUE");
    assert!(theta.precheck.failures[0].message.contains("M9"));

    // Deleted: the base body, no precheck.
    let gamma = by_slug["gamma"];
    assert_eq!(gamma.status, ProposalChange::Deleted);
    assert!(gamma.fork_body.is_none());
    assert!(gamma.base_body.as_deref().unwrap().contains("gamma stands"));
    assert_eq!(gamma.precheck.outcome, PrecheckOutcome::NotRun);
    assert!(gamma.content_hash.is_none());

    // Renamed: keyed by the new slug, the old one named, rehearsed as
    // a create of the new slug in the target.
    let delta_prime = by_slug["delta-prime"];
    assert_eq!(delta_prime.status, ProposalChange::Renamed);
    assert_eq!(delta_prime.renamed_from.as_deref(), Some("delta"));
    assert_eq!(delta_prime.title.as_deref(), Some("Delta Prime"));
    assert_eq!(delta_prime.precheck.outcome, PrecheckOutcome::Clean);
    assert_eq!(delta_prime.precheck.operation.as_deref(), Some("create"));
    assert!(delta_prime.conflict.is_none());

    // (d) conflict by modification: both sides changed `identity`; the
    // self-link in `specifies` is never a difference on either side;
    // the three versions are present, normalised to the fork's name.
    let epsilon = by_slug["epsilon"];
    assert_eq!(epsilon.status, ProposalChange::Modified);
    let conflict = epsilon.conflict.as_ref().expect("epsilon conflicts");
    assert_eq!(conflict.kind, ConflictKind::TargetModified);
    assert_eq!(
        epsilon.fork_delta.as_ref().unwrap().sections,
        vec!["identity"]
    );
    assert_eq!(
        conflict.target_delta.as_ref().unwrap().sections,
        vec!["identity"]
    );
    let target_body = conflict.target_body.as_deref().unwrap();
    assert!(target_body.contains("epsilon as the owner now has it"));
    assert!(
        target_body.contains("[[specs-fork--alpha]]"),
        "{target_body}"
    );
    assert!(!target_body.contains("[[specs--alpha]]"), "{target_body}");
    assert!(
        epsilon
            .base_body
            .as_deref()
            .unwrap()
            .contains("epsilon as the source has it")
    );
    assert!(
        epsilon
            .fork_body
            .as_deref()
            .unwrap()
            .contains("epsilon as the proposer has it")
    );
    assert_eq!(epsilon.precheck.outcome, PrecheckOutcome::Clean);

    // Conflict by deletion: the target no longer holds it.
    let zeta = by_slug["zeta"];
    assert_eq!(
        zeta.conflict.as_ref().unwrap().kind,
        ConflictKind::TargetDeleted
    );
    assert!(zeta.conflict.as_ref().unwrap().target_body.is_none());
    assert_eq!(zeta.precheck.outcome, PrecheckOutcome::NotRun);

    // The target created the slug the fork added.
    let iota = by_slug["iota"];
    assert_eq!(iota.status, ProposalChange::Added);
    assert_eq!(
        iota.conflict.as_ref().unwrap().kind,
        ConflictKind::TargetAdded
    );
    assert!(
        iota.conflict
            .as_ref()
            .unwrap()
            .target_body
            .as_deref()
            .unwrap()
            .contains("iota from the owner")
    );
    assert_eq!(iota.precheck.outcome, PrecheckOutcome::NotRun);

    // The summary counts what the entries say.
    assert_eq!(brief.summary.added, 3);
    assert_eq!(brief.summary.modified, 3);
    assert_eq!(brief.summary.deleted, 1);
    assert_eq!(brief.summary.renamed, 1);
    assert_eq!(brief.summary.conflicts, 3);
    assert_eq!(brief.summary.precheck_failures, 1);
    assert_eq!(brief.summary.re_proposals, 0);

    // The disposition skeleton: one slot per entry, empty, the closed
    // vocabulary, `adopt` withheld on the three conflicts.
    assert_eq!(
        brief
            .dispositions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        slugs
    );
    for (slug, slot) in &brief.dispositions {
        assert_eq!(slot.disposition, "");
        assert_eq!(slot.reason, "");
        let expected: Vec<&str> = if ["epsilon", "zeta", "iota"].contains(&slug.as_str()) {
            vec!["adopt_with_changes", "reject"]
        } else {
            vec!["adopt", "adopt_with_changes", "reject"]
        };
        assert_eq!(slot.accepts, expected, "{slug}");
    }

    // The JSON carries the same and round-trips; the markdown names
    // every block and renders the same bytes twice.
    let json = serde_json::to_value(&brief).unwrap();
    assert_eq!(
        json["dispositions"]["epsilon"]["accepts"],
        serde_json::json!(["adopt_with_changes", "reject"])
    );
    assert_eq!(json["entries"][0]["slug"], "beta");
    let back: memstead_base::ops::ProposalBrief = serde_json::from_value(json).unwrap();
    assert_eq!(back, brief);
    let md = memstead_base::ops::render_proposal_brief(&brief);
    assert_eq!(md, memstead_base::ops::render_proposal_brief(&brief));
    for needle in [
        &format!("- Ancestor on `specs`: `{}`", brief.ancestor),
        &format!("- Base (the fork commit): `{}`", brief.base),
        &format!("- Fork tip: `{}`", brief.fork_tip),
        &format!("- Target tip: `{}`", brief.target_tip),
        "## `beta`: modified (Beta)",
        "- Differences (fork against base): sections `identity`, `purpose`; metadata `level`",
        "- Target referrers: `plans--roadmap` (DEPENDS_ON), `specs--alpha` (DEPENDS_ON)",
        "  - `https://example.org/page` (url, anchored; span: \"the quoted words\"; last state: unobserved)",
        "- Precheck (update in `specs`): clean",
        "## `theta`: added (Theta)",
        "- Precheck (create in `specs`): FAILED",
        "  - `INVALID_ENUM_VALUE`:",
        "## `epsilon`: modified (Epsilon)",
        "- CONFLICT: the target modified this entity since the ancestor (target_modified; target against base: sections `identity`)",
        "### Base version",
        "### Fork version",
        "### Target version",
        "## `zeta`: modified (Zeta)",
        "- CONFLICT: the target deleted this entity since the ancestor (target_deleted)",
        "## `iota`: added (Iota)",
        "- CONFLICT: the target created an entity with this slug since the ancestor (target_added)",
        "## `delta-prime`: renamed (Delta Prime)",
        "- Renamed from: `delta`",
        "## `gamma`: deleted (Gamma)",
    ] {
        assert!(md.contains(needle), "missing {needle:?} in:\n{md}");
    }
    assert!(
        !md.contains("`alpha`:"),
        "an untouched entity is absent:\n{md}"
    );
    assert!(
        !md.contains("`kappa`:"),
        "a self-link retargeted by the fork commit is no change:\n{md}"
    );

    // Nothing moved: both branches, the peer, the registry, the state.
    let after: Vec<Option<String>> = ["specs", "specs-fork", "plans", "__MEMSTEAD"]
        .iter()
        .map(|r| sha_of(&gitdir, &format!("refs/heads/{r}")))
        .collect();
    assert_eq!(before, after, "a render moves no branch");
    assert_eq!(
        state_before,
        workspace_state(tmp.path()),
        "a render touches no state"
    );
    // A second render over the same state is the same brief.
    assert_eq!(engine.proposal_brief("specs-fork").unwrap(), brief);
}

/// The target's proposal record marks a re-proposal by the same
/// content hash (another slug, the same proposed body) and by the same
/// id; an adopted entity in the record never marks; a record that does
/// not parse refuses, named.
#[test]
fn brief_marks_re_proposals_from_the_targets_record() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    let first = engine.proposal_brief("specs-fork").unwrap();
    let eta_hash = first
        .entries
        .iter()
        .find(|e| e.slug == "eta")
        .and_then(|e| e.content_hash.clone())
        .expect("eta has a hash");
    // A record on the target branch: eta's body rejected under another
    // slug, theta rejected by id, beta adopted.
    let record = serde_json::json!({
        "version": 1,
        "proposals": [{
            "id": "earlier-fork@0000",
            "proposer": "proposer-p1",
            "ancestor": first.ancestor,
            "base": "0000",
            "target_tip": first.ancestor,
            "merged_by": "owner",
            "at": "2026-09-19T00:00:00Z",
            "entities": {
                "eta-under-another-name": {"disposition": "reject", "reason": "weak sourcing", "content_hash": eta_hash},
                "theta": {"disposition": "reject", "reason": "no evidence", "content_hash": "other"},
                "beta": {"disposition": "adopt", "content_hash": "other"}
            }
        }]
    });
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    writer
        .write_entity(
            Path::new(".memstead/proposals.json"),
            serde_json::to_string_pretty(&record).unwrap().as_bytes(),
        )
        .unwrap();
    writer
        .commit("proposal record", &CommitContext::internal())
        .unwrap();

    let brief = engine.proposal_brief("specs-fork").unwrap();
    let marks = |slug: &str| -> Vec<(String, String, Option<String>)> {
        brief
            .entries
            .iter()
            .find(|e| e.slug == slug)
            .unwrap()
            .re_proposal
            .iter()
            .map(|m| (m.matched_by.clone(), m.proposal.clone(), m.reason.clone()))
            .collect()
    };
    assert_eq!(
        marks("eta"),
        vec![(
            "content_hash".to_string(),
            "earlier-fork@0000".to_string(),
            Some("weak sourcing".to_string())
        )]
    );
    assert_eq!(
        marks("theta"),
        vec![(
            "id".to_string(),
            "earlier-fork@0000".to_string(),
            Some("no evidence".to_string())
        )]
    );
    assert!(marks("beta").is_empty());
    assert_eq!(brief.summary.re_proposals, 2);
    let md = memstead_base::ops::render_proposal_brief(&brief);
    assert!(md.contains(
        "- RE-PROPOSAL: rejected in proposal `earlier-fork@0000` (the same content): weak sourcing"
    ));
    assert!(md.contains(
        "- RE-PROPOSAL: rejected in proposal `earlier-fork@0000` (the same id): no evidence"
    ));

    // A malformed record refuses, naming the member.
    writer
        .write_entity(Path::new(".memstead/proposals.json"), b"not json")
        .unwrap();
    writer
        .commit("broken record", &CommitContext::internal())
        .unwrap();
    let err = engine.proposal_brief("specs-fork").unwrap_err();
    assert!(
        err.to_string().contains(".memstead/proposals.json"),
        "{err}"
    );
}

/// A fork recorded without a base (made before fork commits existed)
/// reads against its ancestor, and the self-links its bodies still
/// qualify with the source's name are never a difference.
#[test]
fn brief_falls_back_to_the_ancestor_for_a_fork_without_a_base() {
    let (tmp, engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    // Rewrite the fork's config without `base`, as an older engine
    // wrote it.
    let forked_from = engine
        .mem_config_for("specs-fork")
        .and_then(|c| c.forked_from.clone())
        .unwrap();
    commit_config_at_gitdir(
        &gitdir,
        "specs-fork",
        format!(
            r#"{{"schema": "default@1.0.0", "forkedFrom": {{"mem": "specs", "sha": "{}"}}}}"#,
            forked_from.sha
        )
        .as_bytes(),
        &CommitContext::internal(),
        "older fork config",
    )
    .unwrap();
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine reboots");
    let brief = engine.proposal_brief("specs-fork").unwrap();
    assert!(brief.base_is_ancestor);
    assert_eq!(brief.base, forked_from.sha);
    assert_eq!(brief.ancestor, forked_from.sha);
    // Against the ancestor, the fork commit's retargeting is visible
    // as a byte change on the entity whose body carries a self-link,
    // yet after normalisation it differs in nothing: reported as a
    // modification with no differing section, never as a conflict.
    let epsilon = brief.entries.iter().find(|e| e.slug == "epsilon").unwrap();
    assert_eq!(
        epsilon.fork_delta.as_ref().unwrap().sections,
        vec!["identity"]
    );
    // Kappa's bytes moved by the retargeting alone: absent from the
    // brief, as it is when the fork commit is the base.
    assert!(
        !brief.entries.iter().any(|e| e.slug == "kappa"),
        "{:?}",
        brief.entries.iter().map(|e| &e.slug).collect::<Vec<_>>()
    );
    let md = memstead_base::ops::render_proposal_brief(&brief);
    assert!(md.contains("(no fork commit recorded; the ancestor is the base)"));
}

// ---------------------------------------------------------------------
// The refusal complement
// ---------------------------------------------------------------------

/// A mem with no `forkedFrom` refuses `INVALID_INPUT` naming the
/// reason; a fork whose source is not mounted refuses `UNKNOWN_MEM`;
/// an unknown mem refuses `UNKNOWN_MEM`; nothing moves.
#[test]
fn brief_refuses_a_non_fork_an_unmounted_source_and_an_unknown_mem() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    let before: Vec<Option<String>> = ["specs", "specs-fork", "orphan", "__MEMSTEAD"]
        .iter()
        .map(|r| sha_of(&gitdir, &format!("refs/heads/{r}")))
        .collect();
    let state_before = workspace_state(tmp.path());

    let err = engine.proposal_brief("specs").unwrap_err();
    assert_eq!(err.code(), "INVALID_INPUT", "{err}");
    assert!(err.to_string().contains("forkedFrom"), "{err}");

    let err = engine.proposal_brief("orphan").unwrap_err();
    assert_eq!(err.code(), "UNKNOWN_MEM", "{err}");
    assert!(err.to_string().contains("ghost"), "{err}");

    let err = engine.proposal_brief("nobody").unwrap_err();
    assert_eq!(err.code(), "UNKNOWN_MEM", "{err}");

    let after: Vec<Option<String>> = ["specs", "specs-fork", "orphan", "__MEMSTEAD"]
        .iter()
        .map(|r| sha_of(&gitdir, &format!("refs/heads/{r}")))
        .collect();
    assert_eq!(before, after);
    assert_eq!(state_before, workspace_state(tmp.path()));
}
