//! `Engine::proposal_merge` and `Engine::proposal_list` on the git-branch
//! backend (plan-proposal 03).
//!
//! The fixture is plan 02's six-shape fork, now written under identities
//! (`author-owner` on the source, `proposer-p1` on the fork), with one
//! more fork entity a sibling writer committed under an identity but
//! with a body the gate refuses. The merge is exercised end to end:
//! what lands, under which identities and trailers, the checks it
//! records, the record it keeps, the whole-store validation; every
//! refusal with the refs and the sidecars byte-identical afterwards; and
//! two successive proposals against one target so the record marks a
//! re-proposal, travels into an archive and is read off an archive.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use memstead_base::anchor::AnchorInput;
use memstead_base::backend::MemBackend;
use memstead_base::check::{CheckKind, CheckState, Verdict};
use memstead_base::engine::independence::Independence;
use memstead_base::mem_management::{self, MemForkParams};
use memstead_base::ops::proposal::{ProposalBrief, ProposalRecord};
use memstead_base::vcs::Actor;
use memstead_base::{
    CreateEntityArgs, DeleteEntityArgs, EntityId, RenameEntityArgs, UpdateEntityArgs,
};
use memstead_git_branch::ops::agent_notes::agent_notes_since;
use memstead_git_branch::ops::transport::resolve_ref_in_gitdir;
use memstead_git_branch::storage::git_tree::GitTreeBackend;
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::vcs::CommitContext;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

const WORKSPACE_HEAD: &str = "format = \"memstead-git-branch-2\"\n\n\
     [persistence_adapter]\nname = \"file-two-layer\"\n\n";

const OWNER: &str = "author-owner";
const PROPOSER: &str = "proposer-p1";
const MERGER: &str = "owner-o1";
const RECORD: &str = ".memstead/proposals.json";

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

/// Every file under `.memstead/` of the workspace root, path to bytes.
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

/// The refs a merge may move, plus the two sidecars on the target and
/// the fork: the byte-identity a refusal must keep.
fn refs_and_sidecars(gitdir: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    let mut out: Vec<(String, Option<Vec<u8>>)> = ["specs", "specs-fork", "plans", "__MEMSTEAD"]
        .iter()
        .map(|r| {
            (
                r.to_string(),
                sha_of(gitdir, &format!("refs/heads/{r}")).map(String::into_bytes),
            )
        })
        .collect();
    for branch in ["specs", "specs-fork"] {
        let backend = GitTreeBackend::new(gitdir.to_path_buf(), format!("refs/heads/{branch}"));
        for member in [RECORD, ".memstead/anchors.json"] {
            out.push((
                format!("{branch}:{member}"),
                backend.read_entity(Path::new(member)).unwrap(),
            ));
        }
    }
    out
}

/// Whether the tree of commit `sha` holds `path`.
fn tree_has(gitdir: &Path, sha: &str, path: &str) -> bool {
    let repo = gix::open(gitdir).unwrap();
    let commit = repo
        .rev_parse_single(sha)
        .unwrap()
        .object()
        .unwrap()
        .try_into_commit()
        .unwrap();
    commit
        .tree()
        .unwrap()
        .lookup_entry_by_path(path)
        .unwrap()
        .is_some()
}

fn record_on(gitdir: &Path, branch: &str) -> Option<ProposalRecord> {
    GitTreeBackend::new(gitdir.to_path_buf(), format!("refs/heads/{branch}"))
        .read_entity(Path::new(RECORD))
        .unwrap()
        .map(|b| ProposalRecord::from_bytes(&b).expect("the record parses"))
}

/// The staged fixture of plan 02 under identities, plus `lambda`: a
/// fork entity a sibling writer committed under `proposer-p1` with an
/// engine-shaped subject and a body the gate refuses (`level: M9`).
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
        &[("specs", "default@1.0.0"), ("plans", "default@1.0.0")],
    );
    let gitdir = gitdir_of(tmp.path());
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    engine.set_identity(Some(OWNER.to_string()));

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
    create(
        &mut engine,
        "specs",
        "Kappa",
        sections("kappa rests on [[specs--alpha]]", "seed"),
        Vec::new(),
    );
    relate(&mut engine, ("specs", "alpha"), ("specs", "beta"));
    relate(&mut engine, ("plans", "roadmap"), ("specs", "beta"));

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

    // The proposer's changes.
    engine.set_identity(Some(PROPOSER.to_string()));
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
    // Theta: a sibling writer, no identity, a body the gate refuses.
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
    // Lambda: a sibling writer under the proposer's identity with an
    // engine-shaped subject, and a body the gate refuses.
    writer
        .write_entity(
            Path::new("lambda.md"),
            b"---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M9\n---\n# Lambda\n\n## Identity\n\nlambda claims\n\n## Purpose\n\nseed\n",
        )
        .unwrap();
    let mut ctx = CommitContext::internal();
    ctx.identity = Some(PROPOSER.to_string());
    writer
        .commit("memstead: create specs-fork--lambda", &ctx)
        .unwrap();

    // The owner's changes meanwhile.
    engine.set_identity(Some(OWNER.to_string()));
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

    // The merger's session.
    engine.set_identity(Some(MERGER.to_string()));
    (tmp, engine)
}

/// The brief as a file with every slot filled: the plan's six shapes,
/// and the three gate-failing or conflicting entries rejected.
fn filled(engine: &mut memstead_base::Engine) -> ProposalBrief {
    let mut file = engine.proposal_brief("specs-fork").unwrap();
    let set = |file: &mut ProposalBrief, slug: &str, disposition: &str, reason: &str| {
        let slot = file.dispositions.get_mut(slug).unwrap();
        slot.disposition = disposition.to_string();
        slot.reason = reason.to_string();
    };
    set(&mut file, "beta", "adopt", "");
    set(&mut file, "eta", "adopt", "");
    set(&mut file, "gamma", "adopt", "");
    set(&mut file, "delta-prime", "adopt", "");
    set(
        &mut file,
        "epsilon",
        "adopt_with_changes",
        "both sentences hold; merged by hand",
    );
    file.dispositions.get_mut("epsilon").unwrap().body = Some(serde_json::json!({
        "sections": {
            "identity": "epsilon as the owner and the proposer have it",
            "purpose": "seed",
            "specifies": "- rests on [[specs--alpha]] and [[specs:alpha|alpha]]"
        },
        "metadata": {}
    }));
    set(&mut file, "theta", "reject", "fails the gate: level M9");
    set(&mut file, "lambda", "reject", "fails the gate: level M9");
    set(
        &mut file,
        "zeta",
        "reject",
        "the owner deleted it on purpose",
    );
    set(&mut file, "iota", "reject", "the owner's card stands");
    file
}

// ---------------------------------------------------------------------
// AC1: a filled brief lands under two identities with a check per entity
// ---------------------------------------------------------------------

#[test]
fn merge_lands_the_six_shapes_under_two_identities_with_checks_and_the_record() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    let fork_tip_before = sha_of(&gitdir, "refs/heads/specs-fork");
    let memstead_before = sha_of(&gitdir, "refs/heads/__MEMSTEAD");
    let file = filled(&mut engine);
    let proposal_id = file.proposal_id.clone();
    let beta_hash = file
        .entries
        .iter()
        .find(|e| e.slug == "beta")
        .and_then(|e| e.content_hash.clone())
        .unwrap();

    let outcome = engine
        .proposal_merge("specs-fork", &file, Actor::Cli, None, None)
        .expect("the merge lands");

    // One proposer, so one merge commit; the amend commit follows.
    assert_eq!(outcome.merge_commits.len(), 1, "{outcome:?}");
    let merge = &outcome.merge_commits[0];
    assert_eq!(merge.identity, PROPOSER);
    assert_eq!(
        merge.entities,
        vec![
            "specs--beta",
            "specs--delta-prime",
            "specs--delta",
            "specs--epsilon",
            "specs--eta",
            "specs--gamma",
        ]
    );
    let amend = outcome.amend_commit.as_ref().expect("the amend commit");
    assert_eq!(amend.identity, MERGER);
    assert_eq!(amend.entities, vec!["specs--epsilon"]);
    assert_eq!(outcome.target_tip_before, file.target_tip);
    assert_eq!(
        Some(outcome.target_tip_after.clone()),
        sha_of(&gitdir, "refs/heads/specs")
    );
    assert_eq!(outcome.target_tip_after, amend.sha);
    assert_eq!(outcome.merged_by, MERGER);
    assert_eq!(outcome.record_path, RECORD);

    // Per entity: the action, the proposer, the check.
    let by_slug: BTreeMap<&str, &memstead_base::ops::proposal::MergedEntity> = outcome
        .entities
        .iter()
        .map(|e| (e.slug.as_str(), e))
        .collect();
    for (slug, disposition, action, checked) in [
        ("beta", "adopt", "updated", true),
        ("eta", "adopt", "created", true),
        ("gamma", "adopt", "deleted", false),
        ("delta-prime", "adopt", "created", true),
        ("epsilon", "adopt_with_changes", "updated", true),
        ("theta", "reject", "none", false),
        ("lambda", "reject", "none", false),
        ("zeta", "reject", "none", false),
        ("iota", "reject", "none", false),
    ] {
        let e = by_slug[slug];
        assert_eq!(e.disposition, disposition, "{slug}");
        assert_eq!(e.action, action, "{slug}");
        assert_eq!(e.check_recorded, checked, "{slug}");
        assert_eq!(e.proposer.is_some(), disposition != "reject", "{slug}");
    }

    // The target as the fork has it: created, updated, deleted, renamed,
    // self-links under the target's name, anchors under the target's id.
    let store = engine.store();
    let beta = store.get(&EntityId::new("specs", "beta")).unwrap();
    assert_eq!(
        beta.sections["identity"], "beta as the proposer has it",
        "{beta:?}"
    );
    assert_eq!(beta.metadata["level"].to_frontmatter_string(), "M1");
    assert_eq!(beta.sections["purpose"], "beta's purpose, sharpened");
    let eta = store.get(&EntityId::new("specs", "eta")).unwrap();
    assert_eq!(eta.sections["identity"], "eta is new");
    assert!(store.get(&EntityId::new("specs", "gamma")).is_none());
    assert!(store.get(&EntityId::new("specs", "delta")).is_none());
    assert_eq!(
        store
            .get(&EntityId::new("specs", "delta-prime"))
            .unwrap()
            .title,
        "Delta Prime"
    );
    let epsilon = store.get(&EntityId::new("specs", "epsilon")).unwrap();
    assert_eq!(
        epsilon.sections["identity"],
        "epsilon as the owner and the proposer have it"
    );
    assert!(
        epsilon.sections["specifies"].contains("[[specs--alpha]]"),
        "{epsilon:?}"
    );
    assert!(
        store.get(&EntityId::new("specs", "iota")).unwrap().sections["identity"]
            .contains("from the owner"),
        "a rejected entity lands nothing"
    );
    assert!(store.get(&EntityId::new("specs", "theta")).is_none());
    assert!(store.get(&EntityId::new("specs", "lambda")).is_none());
    assert!(store.get(&EntityId::new("specs", "zeta")).is_none());
    let rows = engine.entity_anchors(&EntityId::new("specs", "beta"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].span.as_deref(), Some("the quoted words"));
    let sidecar = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string())
        .read_anchors_sidecar()
        .unwrap()
        .unwrap();
    let sidecar = memstead_base::anchor::AnchorSidecar::from_bytes(&sidecar).unwrap();
    assert_eq!(sidecar.get("specs--beta").len(), 1);
    assert!(sidecar.get("specs-fork--beta").is_empty());

    // The commits' trailers, read by the note reader.
    let notes = agent_notes_since(
        "specs",
        &gitdir,
        memstead_base::ops::EMPTY_TREE_SHA,
        Some("refs/heads/specs"),
    )
    .unwrap()
    .notes;
    let merge_note = notes.iter().find(|n| n.sha == merge.sha).unwrap();
    assert_eq!(merge_note.tool_verb.as_deref(), Some("proposal-merge"));
    assert_eq!(merge_note.identity.as_deref(), Some(PROPOSER));
    assert_eq!(merge_note.merged_by.as_deref(), Some(MERGER));
    assert_eq!(merge_note.proposal.as_deref(), Some(proposal_id.as_str()));
    assert_eq!(merge_note.entity_ids, merge.entities);
    assert_eq!(
        merge_note.created_ids,
        vec!["specs--delta-prime", "specs--eta"]
    );
    let amend_note = notes.iter().find(|n| n.sha == amend.sha).unwrap();
    assert_eq!(amend_note.tool_verb.as_deref(), Some("proposal-amend"));
    assert_eq!(amend_note.identity.as_deref(), Some(MERGER));
    assert_eq!(amend_note.merged_by.as_deref(), Some(MERGER));
    assert_eq!(amend_note.proposal.as_deref(), Some(proposal_id.as_str()));
    assert!(amend_note.created_ids.is_empty());

    // The provenance read: created_by / last_modified_by name the
    // proposer, with the merger, the proposal and the disposition beside.
    let eta_prov = engine.entity_provenance("specs", "specs--eta").unwrap();
    assert!(!eta_prov.story_truncated);
    let created = eta_prov.created_by.as_ref().unwrap();
    assert_eq!(created.identity.as_deref(), Some(PROPOSER));
    assert_eq!(created.merged_by.as_deref(), Some(MERGER));
    assert_eq!(created.proposal.as_deref(), Some(proposal_id.as_str()));
    assert_eq!(created.disposition.as_deref(), Some("adopt"));
    assert_eq!(created.verb.as_deref(), Some("proposal-merge"));
    let beta_prov = engine.entity_provenance("specs", "specs--beta").unwrap();
    let last = beta_prov.last_modified_by.as_ref().unwrap();
    assert_eq!(last.identity.as_deref(), Some(PROPOSER));
    assert_eq!(last.merged_by.as_deref(), Some(MERGER));
    assert_eq!(last.disposition.as_deref(), Some("adopt"));
    assert_eq!(
        beta_prov.created_by.as_ref().unwrap().identity.as_deref(),
        Some(OWNER)
    );
    let eps_prov = engine.entity_provenance("specs", "specs--epsilon").unwrap();
    let last = eps_prov.last_modified_by.as_ref().unwrap();
    assert_eq!(last.identity.as_deref(), Some(MERGER));
    assert_eq!(last.verb.as_deref(), Some("proposal-amend"));
    assert_eq!(last.disposition.as_deref(), Some("adopt_with_changes"));

    // The merger's verification check per adopted entity, against the
    // landed hash (the amend commit's for epsilon); none for a rejected
    // or deleted one; the checks axis reads them confirmed_independent.
    let touches = engine.mem_touches("specs");
    for slug in ["beta", "eta", "delta-prime", "epsilon"] {
        let id = format!("specs--{slug}");
        let (state, record) = engine.entity_check_state("specs", &id).unwrap();
        assert_eq!(state, CheckState::CheckedOk, "{slug}");
        let record = record.unwrap();
        assert_eq!(record.identity.as_deref(), Some(MERGER), "{slug}");
        assert_eq!(
            record.method.as_deref(),
            Some(format!("proposal {proposal_id}").as_str()),
            "{slug}"
        );
        let entity = engine.store().get(&EntityId(id.clone())).unwrap();
        assert_eq!(record.entity_hash, entity.content_hash, "{slug}");
        let (independence, _) = engine.independence_of(entity, &record, &touches);
        assert_eq!(independence, Independence::ConfirmedIndependent, "{slug}");
    }
    for slug in ["theta", "lambda", "zeta", "iota"] {
        let id = format!("specs--{slug}");
        if engine.store().get(&EntityId(id.clone())).is_some() {
            let (state, _) = engine.entity_check_state("specs", &id).unwrap();
            assert_eq!(state, CheckState::NeverChecked, "{slug}");
        }
    }
    assert_eq!(eta_prov.check_state, "checked_ok");

    // A check by the proposer on a merged entity still reads
    // self_checked: `Merged-By` is never compared.
    engine.set_identity(Some(PROPOSER.to_string()));
    let proposer_check = engine
        .record_check(
            "specs",
            "specs--eta",
            Verdict::Ok,
            CheckKind::Verification,
            Some("own re-read"),
            Actor::Cli,
            None,
        )
        .unwrap();
    let eta = engine.store().get(&EntityId::new("specs", "eta")).unwrap();
    let (independence, _) = engine.independence_of(eta, &proposer_check, &touches);
    assert_eq!(independence, Independence::SelfChecked);
    engine.set_identity(Some(MERGER.to_string()));

    // The record on the target branch, in the merge commit: the id, the
    // proposer, the shas, the merger, the time, and per entity the
    // disposition, the reason and the proposed hash; never a body.
    let record = record_on(&gitdir, "specs").expect("the record rides the target branch");
    assert_eq!(record.version, 1);
    assert_eq!(record.proposals.len(), 1);
    let p = &record.proposals[0];
    assert_eq!(p.id, proposal_id);
    assert_eq!(p.proposer.as_deref(), Some(PROPOSER));
    assert_eq!(p.ancestor, file.ancestor);
    assert_eq!(p.base, file.base);
    assert_eq!(p.target_tip, file.target_tip);
    assert_eq!(p.merged_by.as_deref(), Some(MERGER));
    assert!(p.at.ends_with('Z') && p.at.len() == 20, "{}", p.at);
    assert_eq!(p.entities.len(), 9);
    assert_eq!(p.entities["beta"].disposition, "adopt");
    assert_eq!(
        p.entities["beta"].content_hash.as_deref(),
        Some(beta_hash.as_str())
    );
    assert_eq!(p.entities["gamma"].content_hash, None);
    assert_eq!(p.entities["theta"].disposition, "reject");
    assert_eq!(
        p.entities["theta"].reason.as_deref(),
        Some("fails the gate: level M9")
    );
    assert_eq!(p.entities["epsilon"].disposition, "adopt_with_changes");
    let record_bytes = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string())
        .read_entity(Path::new(RECORD))
        .unwrap()
        .unwrap();
    let text = String::from_utf8(record_bytes).unwrap();
    assert!(
        !text.contains("theta claims"),
        "no rejected body in the record"
    );
    assert!(
        !text.contains("beta as the proposer"),
        "no body at all in the record"
    );
    // The record rode the merge commit itself.
    assert!(
        tree_has(&gitdir, &merge.sha, RECORD),
        "the record is in the merge commit"
    );
    assert!(
        record_on(&gitdir, "specs-fork").is_none(),
        "the fork carries no record"
    );
    assert_eq!(engine.proposal_list("specs").unwrap(), record);

    // The whole store validates; the fork and the registry never moved.
    assert!(outcome.validation.is_clean(), "{:?}", outcome.validation);
    // alpha, beta, delta-prime, epsilon, eta, iota, kappa.
    assert_eq!(outcome.validation.entities, 7);
    assert_eq!(sha_of(&gitdir, "refs/heads/specs-fork"), fork_tip_before);
    assert_eq!(sha_of(&gitdir, "refs/heads/__MEMSTEAD"), memstead_before);
    let health = engine.health_scoped(Some("specs")).unwrap();
    assert!(health.load_errors.is_empty(), "{:?}", health.load_errors);
    assert!(
        engine.consistency_findings("specs").unwrap().is_empty(),
        "{:?}",
        engine.consistency_findings("specs").unwrap()
    );
    assert!(
        engine
            .conformance_findings("specs", None)
            .unwrap()
            .is_empty()
    );

    // A second boot reads the same truth off the branch.
    let landed_beta_hash = engine
        .store()
        .get(&EntityId::new("specs", "beta"))
        .unwrap()
        .content_hash
        .clone();
    let engine2 = engine_from_workspace_root(tmp.path()).unwrap();
    assert_eq!(
        engine2
            .store()
            .get(&EntityId::new("specs", "beta"))
            .unwrap()
            .content_hash,
        landed_beta_hash
    );
    assert!(
        engine2
            .store()
            .get(&EntityId::new("specs", "gamma"))
            .is_none()
    );
}

/// Two proposer identities on one fork: one commit per identity, in
/// slug order, each parent-pinned to the one before; the record names
/// both.
#[test]
fn merge_commits_once_per_proposer_identity_in_slug_order() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    // A second proposer touches alpha on the fork.
    engine.set_identity(Some("proposer-p2".to_string()));
    update_sections(
        &mut engine,
        ("specs-fork", "alpha"),
        &[("identity", "alpha as the second proposer has it")],
        &[],
    );
    engine.set_identity(Some(MERGER.to_string()));
    let mut file = filled(&mut engine);
    let slot = file.dispositions.get_mut("alpha").unwrap();
    slot.disposition = "adopt".to_string();
    let target_tip = file.target_tip.clone();

    let outcome = engine
        .proposal_merge("specs-fork", &file, Actor::Cli, None, None)
        .unwrap();
    assert_eq!(outcome.merge_commits.len(), 2);
    assert_eq!(outcome.merge_commits[0].identity, "proposer-p2");
    assert_eq!(outcome.merge_commits[0].entities, vec!["specs--alpha"]);
    assert_eq!(outcome.merge_commits[1].identity, PROPOSER);
    let notes = agent_notes_since(
        "specs",
        &gitdir,
        memstead_base::ops::EMPTY_TREE_SHA,
        Some("refs/heads/specs"),
    )
    .unwrap()
    .notes;
    // Newest first: amend, p1's merge, p2's merge, then the pinned tip.
    assert_eq!(notes[0].sha, outcome.amend_commit.as_ref().unwrap().sha);
    assert_eq!(notes[1].sha, outcome.merge_commits[1].sha);
    assert_eq!(notes[2].sha, outcome.merge_commits[0].sha);
    assert_eq!(notes[3].sha, target_tip);
    let record = record_on(&gitdir, "specs").unwrap();
    assert_eq!(
        record.proposals[0].proposer.as_deref(),
        Some("proposer-p2, proposer-p1")
    );
    // The record rode the last merge commit, not the first.
    assert!(!tree_has(&gitdir, &outcome.merge_commits[0].sha, RECORD));
    assert!(tree_has(&gitdir, &outcome.merge_commits[1].sha, RECORD));
    assert!(outcome.validation.is_clean());
}

/// AC1's refusal complement: no merger identity (`INVALID_INPUT`), an
/// adopted entity whose last fork commit carries no identity
/// (`PROPOSAL_UNATTRIBUTED`, named), an adopted body the gate refuses
/// (the gate's own code), each landing nothing and never writing the
/// fork.
#[test]
fn merge_refuses_without_identity_unattributed_and_gate_failures_landing_nothing() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    let before = refs_and_sidecars(&gitdir);
    let state_before = workspace_state(tmp.path());
    let file = filled(&mut engine);

    engine.set_identity(None);
    let err = engine
        .proposal_merge("specs-fork", &file, Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_INPUT", "{err}");
    assert!(err.to_string().contains("--identity"), "{err}");
    engine.set_identity(Some(MERGER.to_string()));

    let mut unattributed = file.clone();
    let slot = unattributed.dispositions.get_mut("theta").unwrap();
    slot.disposition = "adopt".to_string();
    slot.reason.clear();
    let err = engine
        .proposal_merge("specs-fork", &unattributed, Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "PROPOSAL_UNATTRIBUTED", "{err}");
    assert_eq!(err.details()["slug"], "theta");

    let mut gate = file.clone();
    let slot = gate.dispositions.get_mut("lambda").unwrap();
    slot.disposition = "adopt".to_string();
    slot.reason.clear();
    let err = engine
        .proposal_merge("specs-fork", &gate, Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_ENUM_VALUE", "{err}");

    assert_eq!(
        before,
        refs_and_sidecars(&gitdir),
        "a refusal lands nothing"
    );
    assert_eq!(state_before, workspace_state(tmp.path()));
    assert!(record_on(&gitdir, "specs").is_none());
    assert!(
        engine.store().get(&EntityId::new("specs", "eta")).is_none(),
        "the store is rolled back too"
    );
    assert_eq!(
        engine.entity_check_state("specs", "specs--beta").unwrap().0,
        CheckState::NeverChecked,
        "no check is recorded on a refused merge"
    );
}

// ---------------------------------------------------------------------
// AC2: incomplete, stale or conflicting files refuse and land nothing
// ---------------------------------------------------------------------

#[test]
fn merge_refuses_incomplete_stale_and_conflicting_files_and_lands_nothing() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    let before = refs_and_sidecars(&gitdir);
    let state_before = workspace_state(tmp.path());
    let file = filled(&mut engine);
    let mut refuse = |file: &ProposalBrief, code: &str| -> memstead_base::EngineError {
        let err = engine
            .proposal_merge("specs-fork", file, Actor::Cli, None, None)
            .unwrap_err();
        assert_eq!(err.code(), code, "{err}");
        err
    };

    // Missing a slot, or naming one the brief lacks.
    let mut incomplete = file.clone();
    incomplete.dispositions.remove("beta");
    let err = refuse(&incomplete, "PROPOSAL_DISPOSITIONS_INCOMPLETE");
    assert_eq!(err.details()["missing"], serde_json::json!(["beta"]));
    let mut extra = file.clone();
    extra
        .dispositions
        .insert("ghost".to_string(), file.dispositions["beta"].clone());
    let err = refuse(&extra, "PROPOSAL_DISPOSITIONS_INCOMPLETE");
    assert_eq!(err.details()["unexpected"], serde_json::json!(["ghost"]));

    // Outside the vocabulary.
    let mut vocab = file.clone();
    vocab.dispositions.get_mut("beta").unwrap().disposition = "maybe".to_string();
    let err = refuse(&vocab, "INVALID_INPUT");
    assert!(err.to_string().contains("maybe"), "{err}");

    // A reject or an adopt_with_changes without a reason.
    let mut no_reason = file.clone();
    no_reason
        .dispositions
        .get_mut("theta")
        .unwrap()
        .reason
        .clear();
    let err = refuse(&no_reason, "INVALID_INPUT");
    assert!(err.to_string().contains("reason"), "{err}");
    let mut no_reason = file.clone();
    no_reason.dispositions.get_mut("epsilon").unwrap().reason = "  ".to_string();
    refuse(&no_reason, "INVALID_INPUT");

    // `adopt` on a conflict entity.
    let mut conflict = file.clone();
    let slot = conflict.dispositions.get_mut("epsilon").unwrap();
    slot.disposition = "adopt".to_string();
    slot.body = None;
    let err = refuse(&conflict, "PROPOSAL_CONFLICT");
    assert_eq!(err.details()["slug"], "epsilon");
    assert_eq!(err.details()["kind"], "target_modified");

    // `adopt_with_changes` without a body, and with a body the gate
    // refuses.
    let mut no_body = file.clone();
    no_body.dispositions.get_mut("epsilon").unwrap().body = None;
    let err = refuse(&no_body, "INVALID_INPUT");
    assert!(err.to_string().contains("body"), "{err}");
    let mut bad_body = file.clone();
    bad_body.dispositions.get_mut("epsilon").unwrap().body = Some(serde_json::json!({
        "sections": {"identity": "merged", "purpose": "seed"},
        "metadata": {"level": "M9"}
    }));
    refuse(&bad_body, "INVALID_ENUM_VALUE");
    // `adopt_with_changes` on a deletion.
    let mut on_delete = file.clone();
    let slot = on_delete.dispositions.get_mut("gamma").unwrap();
    slot.disposition = "adopt_with_changes".to_string();
    slot.reason = "why".to_string();
    slot.body = Some(serde_json::json!({"sections": {"identity": "x"}}));
    refuse(&on_delete, "INVALID_INPUT");

    // A recorded target tip that differs from the branch's.
    let mut stale = file.clone();
    stale.target_tip = "0".repeat(40);
    let err = refuse(&stale, "PROPOSAL_STALE");
    assert_eq!(err.details()["side"], "target");
    assert_eq!(err.details()["recorded"], "0".repeat(40));
    assert_eq!(err.details()["current"], file.target_tip);
    assert!(err.to_string().contains("proposal brief"), "{err}");
    // A fork tip that moved is stale too.
    let mut stale_fork = file.clone();
    stale_fork.fork_tip = "0".repeat(40);
    let err = refuse(&stale_fork, "PROPOSAL_STALE");
    assert_eq!(err.details()["side"], "fork");

    // A file rendered for another fork.
    let mut other = file.clone();
    other.fork = "specs".to_string();
    refuse(&other, "INVALID_INPUT");

    // A mem with no recorded ancestor, and an unknown mem.
    let mut non_fork = file.clone();
    non_fork.fork = "specs".to_string();
    let err = engine
        .proposal_merge("specs", &non_fork, Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_INPUT", "{err}");
    assert!(err.to_string().contains("forkedFrom"), "{err}");
    let mut unknown = file.clone();
    unknown.fork = "nobody".to_string();
    let err = engine
        .proposal_merge("nobody", &unknown, Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "UNKNOWN_MEM", "{err}");

    // Nothing moved.
    assert_eq!(
        before,
        refs_and_sidecars(&gitdir),
        "a refusal lands nothing"
    );
    assert_eq!(state_before, workspace_state(tmp.path()));
    assert!(record_on(&gitdir, "specs").is_none());

    // The target moved after the render: stale, naming both shas.
    engine.set_identity(Some(OWNER.to_string()));
    update_sections(
        &mut engine,
        ("specs", "alpha"),
        &[("identity", "alpha, touched after the review")],
        &[],
    );
    engine.set_identity(Some(MERGER.to_string()));
    let moved = sha_of(&gitdir, "refs/heads/specs").unwrap();
    let err = engine
        .proposal_merge("specs-fork", &file, Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "PROPOSAL_STALE", "{err}");
    assert_eq!(err.details()["recorded"], file.target_tip);
    assert_eq!(err.details()["current"], moved);
}

/// AC2's refusal complement: a complete, valid file whose tips match is
/// never refused; a `reject` of a conflict entity is admitted; the same
/// file merged twice refuses the second time as stale, never applying
/// twice.
#[test]
fn a_valid_file_merges_once_and_refuses_the_second_time_as_stale() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());
    let file = filled(&mut engine);
    let first = engine
        .proposal_merge("specs-fork", &file, Actor::Cli, None, None)
        .expect("a valid file is never refused");
    assert_eq!(
        first
            .entities
            .iter()
            .find(|e| e.slug == "zeta")
            .unwrap()
            .action,
        "none"
    );
    let after_first = refs_and_sidecars(&gitdir);
    let err = engine
        .proposal_merge("specs-fork", &file, Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "PROPOSAL_STALE", "{err}");
    assert_eq!(err.details()["recorded"], file.target_tip);
    assert_eq!(err.details()["current"], first.target_tip_after);
    assert_eq!(
        after_first,
        refs_and_sidecars(&gitdir),
        "never applied twice"
    );
    assert_eq!(record_on(&gitdir, "specs").unwrap().proposals.len(), 1);
}

// ---------------------------------------------------------------------
// AC3: the record rides the target branch and every surface reads it
// ---------------------------------------------------------------------

/// Two successive proposals against one target: the first rejects eta,
/// the second fork re-proposes eta's body verbatim (marked by content
/// hash) beside a different card (never marked); the second merge
/// appends to the record; a third fork rewording eta is marked by id
/// against the first rejection; `proposal list` and the provenance read
/// name each; the member travels into an archive and is read off an
/// archive.
#[test]
fn the_record_marks_re_proposals_travels_into_archives_and_is_read_everywhere() {
    let (tmp, mut engine) = staged();
    let gitdir = gitdir_of(tmp.path());

    // First proposal: eta rejected, the rest as in AC1.
    let mut first = filled(&mut engine);
    let slot = first.dispositions.get_mut("eta").unwrap();
    slot.disposition = "reject".to_string();
    slot.reason = "weak sourcing".to_string();
    let eta_hash = first
        .entries
        .iter()
        .find(|e| e.slug == "eta")
        .and_then(|e| e.content_hash.clone())
        .unwrap();
    let first_outcome = engine
        .proposal_merge("specs-fork", &first, Actor::Cli, None, None)
        .unwrap();
    assert!(engine.store().get(&EntityId::new("specs", "eta")).is_none());

    // Second fork: eta again, verbatim, and the same body as "Eta Two".
    engine.set_identity(Some(OWNER.to_string()));
    mem_management::fork_mem(
        &mut engine,
        MemForkParams {
            source: "specs".to_string(),
            sha: None,
            name: "specs-fork2".to_string(),
            remote: None,
            note: None,
            operator_mode: true,
            actor: Actor::Cli,
            client: None,
        },
    )
    .unwrap();
    engine.set_identity(Some("proposer-p2".to_string()));
    create(
        &mut engine,
        "specs-fork2",
        "Eta",
        sections("eta is new", "a new card"),
        Vec::new(),
    );
    create(
        &mut engine,
        "specs-fork2",
        "Eta Two",
        sections("eta is new", "a new card"),
        Vec::new(),
    );
    engine.set_identity(Some(MERGER.to_string()));

    let brief = engine.proposal_brief("specs-fork2").unwrap();
    let marks = |slug: &str| -> Vec<(String, String)> {
        brief
            .entries
            .iter()
            .find(|e| e.slug == slug)
            .unwrap()
            .re_proposal
            .iter()
            .map(|m| (m.matched_by.clone(), m.proposal.clone()))
            .collect()
    };
    assert_eq!(
        marks("eta"),
        vec![("content_hash".to_string(), first.proposal_id.clone())]
    );
    // A different card (another title, so another body) is never a
    // re-proposal.
    assert!(marks("eta-two").is_empty());
    assert_eq!(
        brief
            .entries
            .iter()
            .find(|e| e.slug == "eta")
            .unwrap()
            .content_hash,
        Some(eta_hash.clone()),
        "the same body proposes the same hash, whenever it was written"
    );

    // Second merge: eta adopted after all, eta-two rejected by the mark.
    let mut second = brief.clone();
    second.dispositions.get_mut("eta").unwrap().disposition = "adopt".to_string();
    let slot = second.dispositions.get_mut("eta-two").unwrap();
    slot.disposition = "reject".to_string();
    slot.reason = "a duplicate of eta".to_string();
    let second_outcome = engine
        .proposal_merge("specs-fork2", &second, Actor::Cli, None, None)
        .unwrap();
    assert_eq!(second_outcome.merge_commits[0].identity, "proposer-p2");

    // The record holds both, in order; `proposal list` renders both.
    let record = engine.proposal_list("specs").unwrap();
    assert_eq!(record.proposals.len(), 2);
    assert_eq!(record.proposals[0].id, first.proposal_id);
    assert_eq!(record.proposals[1].id, second.proposal_id);
    assert_eq!(record.proposals[1].entities["eta"].disposition, "adopt");
    assert_eq!(
        record.proposals[1].entities["eta-two"].disposition,
        "reject"
    );
    let md = memstead_base::ops::render_proposal_record("specs", &record);
    assert!(md.contains(&format!("## `{}`", first.proposal_id)), "{md}");
    assert!(md.contains(&format!("## `{}`", second.proposal_id)), "{md}");
    assert!(md.contains("- `eta`: reject (weak sourcing)"), "{md}");
    assert!(
        md.contains("- `eta-two`: reject (a duplicate of eta)"),
        "{md}"
    );
    assert!(!md.contains('\u{2014}'));
    assert_eq!(record_on(&gitdir, "specs").unwrap(), record);
    assert!(record_on(&gitdir, "specs-fork").is_none());
    assert!(record_on(&gitdir, "specs-fork2").is_none());

    // A third fork rewording eta: marked by id against the first
    // rejection (the second proposal adopted it, and an adoption never
    // marks).
    engine.set_identity(Some(OWNER.to_string()));
    mem_management::fork_mem(
        &mut engine,
        MemForkParams {
            source: "specs".to_string(),
            sha: None,
            name: "specs-fork3".to_string(),
            remote: None,
            note: None,
            operator_mode: true,
            actor: Actor::Cli,
            client: None,
        },
    )
    .unwrap();
    engine.set_identity(Some("proposer-p3".to_string()));
    update_sections(
        &mut engine,
        ("specs-fork3", "eta"),
        &[("identity", "eta, reworded")],
        &[],
    );
    engine.set_identity(Some(MERGER.to_string()));
    let third = engine.proposal_brief("specs-fork3").unwrap();
    let eta3 = third.entries.iter().find(|e| e.slug == "eta").unwrap();
    assert_eq!(
        eta3.re_proposal
            .iter()
            .map(|m| (m.matched_by.clone(), m.proposal.clone()))
            .collect::<Vec<_>>(),
        vec![("id".to_string(), first.proposal_id.clone())]
    );

    // The provenance read names the proposal eta came from.
    let prov = engine.entity_provenance("specs", "specs--eta").unwrap();
    let created = prov.created_by.unwrap();
    assert_eq!(
        created.proposal.as_deref(),
        Some(second.proposal_id.as_str())
    );
    assert_eq!(created.identity.as_deref(), Some("proposer-p2"));
    assert_eq!(created.disposition.as_deref(), Some("adopt"));
    // The record is not part of any entity's content hash: beta's hash
    // after the first merge is its hash after the second, which changed
    // the record and not beta.
    let beta = engine.store().get(&EntityId::new("specs", "beta")).unwrap();
    assert_eq!(
        beta.content_hash,
        first_outcome
            .entities
            .iter()
            .find(|e| e.slug == "beta")
            .unwrap()
            .content_hash
            .clone()
            .unwrap()
    );

    // The member travels into a sealed archive as a recognised meta
    // member, survives the canonical re-pack, and is read off an archive
    // mount's backend; the entities and the record agree.
    for mem in ["specs", "specs-fork2"] {
        engine
            .set_mem_version(mem, semver::Version::new(0, 1, 0), None)
            .unwrap();
    }
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let validated = memstead_base::validator::validate_and_normalize_archive(&bytes).unwrap();
    let sealed = validated
        .proposals_bytes
        .expect("the archive carries the record");
    assert_eq!(ProposalRecord::from_bytes(&sealed).unwrap(), record);
    let revalidated =
        memstead_base::validator::validate_and_normalize_archive(&validated.canonical_bytes)
            .unwrap();
    assert_eq!(revalidated.proposals_bytes.as_deref(), Some(&sealed[..]));
    let archive_path = tmp.path().join("specs.mem");
    std::fs::write(&archive_path, &validated.canonical_bytes).unwrap();
    let archive = memstead_base::storage::ArchiveBackend::new(archive_path);
    let off_archive = archive.read_proposal_record().unwrap().unwrap();
    assert_eq!(ProposalRecord::from_bytes(&off_archive).unwrap(), record);
    // A mem with no record seals no member: the second fork was made
    // after the first merge and the fork commit dropped the inherited
    // record (a fork carries none).
    let fork2 = engine.export_mem_to_bytes("specs-fork2").unwrap();
    assert!(
        memstead_base::validator::validate_and_normalize_archive(&fork2)
            .unwrap()
            .proposals_bytes
            .is_none()
    );
}
