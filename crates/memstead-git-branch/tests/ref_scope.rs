//! A ref given to a read verb stays inside what the workspace mounts.
//!
//! One git directory holds the mems `alpha` and `gamma`. A "view"
//! workspace mounts `alpha` alone; the "store" workspace mounts both.
//! `changes_since` takes as its cursor the empty-tree hash or a commit
//! of the named mem's own history; `diff` takes as either ref the
//! empty-tree hash or a commit in the history of a mem the workspace
//! mounts. Every other ref refuses exactly as a ref that names nothing
//! does, and nothing of `gamma` reaches a workspace that does not mount
//! it: not its text, not its titles, not the name of its branch.

use std::path::{Path, PathBuf};

use memstead_base::mem_management::{MemMountParams, MountSource, mount_mem};
use memstead_base::ops::{DiffConfig, EMPTY_TREE_SHA};
use memstead_base::vcs::Actor;
use memstead_base::{CreateEntityArgs, Engine, EngineError, MountCapability};
use memstead_git_branch::ops::agent_notes::agent_notes_since;
use memstead_git_branch::ops::transport::resolve_ref_in_gitdir;
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

fn create(engine: &mut Engine, mem: &str, title: &str) {
    let mut sections = indexmap::IndexMap::new();
    sections.insert("identity".to_string(), format!("identity of {title}"));
    sections.insert("purpose".to_string(), format!("purpose of {title}"));
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
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

struct Fixture {
    _tmp: TempDir,
    store: PathBuf,
    view: PathBuf,
    gitdir: PathBuf,
}

impl Fixture {
    fn tip(&self, branch: &str) -> String {
        resolve_ref_in_gitdir(&self.gitdir, &format!("refs/heads/{branch}"))
            .unwrap()
            .unwrap_or_else(|| panic!("branch {branch} has a tip"))
    }

    fn store_engine(&self) -> Engine {
        engine_from_workspace_root(&self.store).unwrap()
    }

    fn view_engine(&self) -> Engine {
        engine_from_workspace_root(&self.view).unwrap()
    }

    /// The refs that lie outside `alpha`'s history and outside every mem
    /// the view mounts: the closed list the plan grades against.
    fn outside(&self) -> Vec<String> {
        let gamma = self.tip("gamma");
        vec![
            "gamma".to_string(),
            "refs/heads/gamma".to_string(),
            gamma.clone(),
            format!("{gamma}~1"),
            "refs/heads/__MEMSTEAD".to_string(),
            "main".to_string(),
        ]
    }
}

fn mount(view: &Path, gitdir: &Path, name: &str, capability: MountCapability) {
    let mut engine = engine_from_workspace_root(view).unwrap();
    mount_mem(
        &mut engine,
        MemMountParams {
            name: name.to_string(),
            source: MountSource::Branch {
                branch: name.to_string(),
                gitdir: Some(gitdir.to_path_buf()),
            },
            capability,
            schema: None,
            note: None,
            operator_mode: true,
            actor: Actor::Cli,
            client: None,
        },
    )
    .unwrap_or_else(|e| panic!("mount {name}: {e:?}"));
}

/// Two mems with two entities each in one git directory, and a view
/// that mounts `alpha` alone.
fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let view = tmp.path().join("view");
    init_real_mem_repo(
        &store,
        &[("alpha", "default@1.0.0"), ("gamma", "default@1.0.0")],
    );
    {
        let mut engine = engine_from_workspace_root(&store).unwrap();
        create(&mut engine, "alpha", "Alpha One");
        create(&mut engine, "alpha", "Alpha Two");
        create(&mut engine, "gamma", "Gamma One");
        create(&mut engine, "gamma", "Gamma Two");
    }
    let gitdir = std::fs::canonicalize(store.join("mem-repo").join(".git")).unwrap();
    memstead_git_branch::lay_workspace_marker(&view).unwrap();
    mount(&view, &gitdir, "alpha", MountCapability::Write);
    Fixture {
        _tmp: tmp,
        store,
        view,
        gitdir,
    }
}

/// What a caller can tell two refusals apart by: the code, the keys of
/// the details, and the message with the ref it echoes blanked.
fn shape(error: &EngineError, given: &str) -> (String, Vec<String>, String) {
    let details = error.details();
    let mut keys: Vec<String> = details
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    (
        error.code().to_string(),
        keys,
        error.to_string().replace(given, "<ref>"),
    )
}

/// Two refs that exist nowhere in the git directory: a name no ref
/// carries and a hash that names no object.
const NOWHERE: [&str; 2] = ["no-such-branch", "0123456789abcdef0123456789abcdef01234567"];

fn says_gamma(text: &str) -> bool {
    text.to_lowercase().contains("gamma one") || text.to_lowercase().contains("gamma two")
}

#[test]
fn changes_since_refuses_a_cursor_outside_the_named_mems_own_history() {
    let f = fixture();
    let view = f.view_engine();
    let nowhere: Vec<_> = NOWHERE
        .iter()
        .map(|cursor| {
            let error = view
                .changes_since("alpha", cursor, None)
                .expect_err("a cursor that names nothing refuses");
            assert_eq!(error.code(), "INVALID_CURSOR", "{error}");
            shape(&error, cursor)
        })
        .collect();
    assert_eq!(
        nowhere[0], nowhere[1],
        "one refusal for what exists nowhere"
    );

    for cursor in f.outside() {
        let error = view
            .changes_since("alpha", &cursor, None)
            .expect_err("a cursor outside alpha's history refuses");
        assert_eq!(error.code(), "INVALID_CURSOR", "{cursor}: {error}");
        assert_eq!(
            shape(&error, &cursor),
            nowhere[0],
            "{cursor}: no different from a cursor that names nothing"
        );
        assert!(!says_gamma(&error.to_string()), "{error}");
    }

    // A cursor is the named mem's own history, also where both mems
    // are mounted.
    let store = f.store_engine();
    for cursor in f.outside() {
        let error = store
            .changes_since("alpha", &cursor, None)
            .expect_err("a cursor outside alpha's history refuses");
        assert_eq!(
            shape(&error, &cursor),
            nowhere[0],
            "{cursor}: the same refusal where gamma is mounted too"
        );
    }
}

#[test]
fn changes_since_keeps_serving_cursors_of_the_mems_own_history() {
    let f = fixture();
    let view = f.view_engine();
    let tip = f.tip("alpha");

    let all = view.changes_since("alpha", EMPTY_TREE_SHA, None).unwrap();
    assert_eq!(all.changes.len(), 2, "{:?}", all.changes);
    assert_eq!(all.head, tip);

    let none = view.changes_since("alpha", &tip, None).unwrap();
    assert!(none.changes.is_empty(), "{:?}", none.changes);
    // The head a call returned is the next cursor.
    let again = view.changes_since("alpha", &all.head, None).unwrap();
    assert!(again.changes.is_empty());

    let parent = resolve_ref_in_gitdir(&f.gitdir, &format!("{tip}~1"))
        .unwrap()
        .expect("alpha's tip has a parent");
    for cursor in [
        parent.clone(),
        parent[..12].to_string(),
        "HEAD~1".to_string(),
    ] {
        let one = view
            .changes_since("alpha", &cursor, None)
            .unwrap_or_else(|e| panic!("{cursor}: {e}"));
        assert_eq!(one.changes.len(), 1, "{cursor}: {:?}", one.changes);
    }
}

#[test]
fn a_cursor_of_a_history_that_was_replaced_is_not_found() {
    let f = fixture();
    let mut store = f.store_engine();
    let old_tip = f.tip("alpha");
    let parent = resolve_ref_in_gitdir(&f.gitdir, &format!("{old_tip}~1"))
        .unwrap()
        .unwrap();
    store
        .branch_reset("alpha", &parent, Some(&old_tip))
        .expect("the branch moves back");
    create(&mut store, "alpha", "Alpha Three");
    assert_ne!(f.tip("alpha"), old_tip);

    let nowhere = store
        .changes_since("alpha", NOWHERE[0], None)
        .expect_err("refuses");
    let error = store
        .changes_since("alpha", &old_tip, None)
        .expect_err("the replaced tip is no ancestor any more");
    assert_eq!(error.code(), "INVALID_CURSOR", "{error}");
    assert_eq!(shape(&error, &old_tip), shape(&nowhere, NOWHERE[0]));
    // The cursor that survived the move still works.
    assert_eq!(
        store
            .changes_since("alpha", &parent, None)
            .unwrap()
            .changes
            .len(),
        1
    );
}

#[test]
fn the_commit_note_reader_takes_the_same_cursors() {
    let f = fixture();
    let head = Some("refs/heads/alpha");
    let tip = f.tip("alpha");

    let all = agent_notes_since("alpha", &f.gitdir, EMPTY_TREE_SHA, head).unwrap();
    assert!(all.notes.len() >= 2, "{}", all.notes.len());
    let parent = resolve_ref_in_gitdir(&f.gitdir, &format!("{tip}~1"))
        .unwrap()
        .unwrap();
    let one = agent_notes_since("alpha", &f.gitdir, &parent, head).unwrap();
    assert_eq!(one.notes.len(), 1);
    assert!(
        agent_notes_since("alpha", &f.gitdir, &tip, head)
            .unwrap()
            .notes
            .is_empty()
    );

    let nowhere = agent_notes_since("alpha", &f.gitdir, NOWHERE[0], head)
        .expect_err("refuses")
        .to_string()
        .replace(NOWHERE[0], "<ref>");
    for cursor in f.outside() {
        let error = agent_notes_since("alpha", &f.gitdir, &cursor, head)
            .expect_err("a cursor outside the walked history refuses")
            .to_string();
        assert_eq!(error.replace(cursor.as_str(), "<ref>"), nowhere, "{cursor}");
    }
}

fn content() -> Option<DiffConfig> {
    Some(DiffConfig {
        include_content: true,
        ..Default::default()
    })
}

#[test]
fn diff_refuses_a_ref_outside_the_mems_the_workspace_mounts() {
    let f = fixture();
    let view = f.view_engine();
    let tip = f.tip("alpha");
    let nowhere: Vec<_> = NOWHERE
        .iter()
        .flat_map(|given| {
            let first = view
                .diff("alpha", given, &tip, content())
                .expect_err("a ref that names nothing refuses");
            let second = view
                .diff("alpha", &tip, given, content())
                .expect_err("a ref that names nothing refuses");
            assert_eq!(first.code(), "UNKNOWN_REF", "{first}");
            [shape(&first, given), shape(&second, given)]
        })
        .collect();
    assert!(
        nowhere.iter().all(|s| s == &nowhere[0]),
        "one refusal for what exists nowhere: {nowhere:?}"
    );

    for given in f.outside() {
        for (ref_a, ref_b) in [
            (given.as_str(), tip.as_str()),
            (tip.as_str(), given.as_str()),
        ] {
            match view.diff("alpha", ref_a, ref_b, content()) {
                Ok(diff) => panic!(
                    "{given}: a ref outside the mounted mems was read: {}",
                    serde_json::to_string(&diff).unwrap()
                ),
                Err(error) => {
                    assert_eq!(error.code(), "UNKNOWN_REF", "{given}: {error}");
                    assert_eq!(
                        shape(&error, &given),
                        nowhere[0],
                        "{given}: no different from a ref that names nothing"
                    );
                    assert!(!says_gamma(&error.to_string()), "{error}");
                }
            }
        }
    }
}

#[test]
fn diff_keeps_serving_the_histories_the_workspace_mounts() {
    let f = fixture();
    let view = f.view_engine();
    let tip = f.tip("alpha");
    let parent = resolve_ref_in_gitdir(&f.gitdir, &format!("{tip}~1"))
        .unwrap()
        .unwrap();

    // The empty tree as either ref.
    let from_nothing = view.diff("alpha", EMPTY_TREE_SHA, &tip, content()).unwrap();
    assert_eq!(from_nothing.entries.len(), 2);
    let to_nothing = view.diff("alpha", &tip, EMPTY_TREE_SHA, content()).unwrap();
    assert_eq!(to_nothing.entries.len(), 2);

    // Two commits of alpha's own history, by hash and by `HEAD~1`.
    for ref_a in [parent.as_str(), "HEAD~1"] {
        let one = view
            .diff("alpha", ref_a, "HEAD", content())
            .unwrap_or_else(|e| panic!("{ref_a}: {e}"));
        assert_eq!(one.entries.len(), 1, "{ref_a}");
        assert_eq!(one.resolved_a_sha, parent);
        assert_eq!(one.resolved_b_sha, tip);
    }

    // A peer the workspace mounts, by its branch name: writable in the
    // store, read-only in a second view.
    let store = f.store_engine();
    for peer in ["gamma", "refs/heads/gamma"] {
        let across = store
            .diff("alpha", &tip, peer, content())
            .unwrap_or_else(|e| panic!("{peer}: {e}"));
        assert!(
            says_gamma(&serde_json::to_string(&across).unwrap()),
            "{peer}: the peer's entities are in the diff"
        );
    }
    mount(&f.view, &f.gitdir, "gamma", MountCapability::ReadOnly);
    let both = f.view_engine();
    for peer in ["gamma", "refs/heads/gamma"] {
        let across = both
            .diff("alpha", &tip, peer, content())
            .unwrap_or_else(|e| panic!("read-only {peer}: {e}"));
        assert!(says_gamma(&serde_json::to_string(&across).unwrap()));
    }
    // And the registry ref stays out, also there.
    let error = both
        .diff("alpha", &tip, "refs/heads/__MEMSTEAD", content())
        .expect_err("the registry is no mem");
    assert_eq!(error.code(), "UNKNOWN_REF");
}

fn git(gitdir: &Path, args: &[&str]) {
    // An annotated tag and a commit need an identity, and a machine
    // with no git configuration (a CI runner) has none to offer.
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(gitdir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "memstead-test")
        .env("GIT_AUTHOR_EMAIL", "test@memstead.invalid")
        .env("GIT_COMMITTER_NAME", "memstead-test")
        .env("GIT_COMMITTER_EMAIL", "test@memstead.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The output of a raw git command against the fixture's git directory.
fn git_out(gitdir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(gitdir)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.com",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A ref is anchored in scope or it is no ref. The anchors that resolve:
/// the mem's own branch by name, `HEAD`, a commit hash of its history,
/// full or abbreviated, each with parent steps. Everything else refuses
/// as a ref that names nothing does, also when walking from it would
/// arrive inside the mem's history: a branch the workspace does not
/// mount that shares history with the mem (a fork, a stale branch, a
/// merge), a tag, a remote-tracking ref, and every revision syntax
/// beyond parent steps.
#[test]
fn a_ref_is_anchored_in_scope_or_it_is_no_ref() {
    let f = fixture();
    let tip = f.tip("alpha");
    let parent = resolve_ref_in_gitdir(&f.gitdir, &format!("{tip}~1"))
        .unwrap()
        .unwrap();
    // Branches no workspace mounts, sharing history with alpha: one
    // that points into alpha's history, one with a commit of its own on
    // top of alpha's tip, and a merge of alpha and gamma.
    git(&f.gitdir, &["branch", "stale", &parent]);
    let tree = git_out(&f.gitdir, &["rev-parse", &format!("{tip}^{{tree}}")]);
    let own = git_out(
        &f.gitdir,
        &[
            "commit-tree",
            &tree,
            "-p",
            &tip,
            "-m",
            "a commit of the fork's own",
        ],
    );
    git(&f.gitdir, &["update-ref", "refs/heads/forky", &own]);
    let merged = git_out(
        &f.gitdir,
        &[
            "commit-tree",
            &tree,
            "-p",
            &tip,
            "-p",
            &f.tip("gamma"),
            "-m",
            "a merge of both",
        ],
    );
    git(&f.gitdir, &["update-ref", "refs/heads/mix", &merged]);
    git(&f.gitdir, &["tag", "inside", &parent]);
    git(
        &f.gitdir,
        &["tag", "-a", "inside-annotated", "-m", "a tag", &parent],
    );
    git(&f.gitdir, &["tag", "outside", &f.tip("gamma")]);
    git(
        &f.gitdir,
        &["update-ref", "refs/remotes/origin/gamma", &f.tip("gamma")],
    );
    let view = f.view_engine();

    for (inside, resolved) in [
        (parent[..12].to_string(), parent.as_str()),
        (parent.to_uppercase(), parent.as_str()),
        ("HEAD^".to_string(), parent.as_str()),
        ("HEAD~1^0".to_string(), parent.as_str()),
        ("alpha~1".to_string(), parent.as_str()),
        ("refs/heads/alpha^1".to_string(), parent.as_str()),
        (format!("{tip}~1"), parent.as_str()),
        ("alpha".to_string(), tip.as_str()),
    ] {
        let diff = view
            .diff("alpha", &inside, "HEAD", content())
            .unwrap_or_else(|e| panic!("{inside}: {e}"));
        assert_eq!(diff.resolved_a_sha, resolved, "{inside}");
        let events = view
            .changes_since("alpha", &inside, None)
            .unwrap_or_else(|e| panic!("{inside}: {e}"))
            .changes
            .len();
        assert_eq!(events, usize::from(resolved == parent), "{inside}");
    }

    let nowhere = view
        .diff("alpha", NOWHERE[0], "HEAD", content())
        .expect_err("refuses");
    let mut outside: Vec<String> = [
        // An unmounted branch that shares history with alpha.
        "stale",
        "refs/heads/stale",
        "stale~0",
        "stale^0",
        "forky",
        "forky~1",
        "forky^",
        "forky~2",
        "forky^^",
        "forky@{1}",
        "forky^{/Alpha}",
        "forky^{/create}",
        "mix",
        "mix^1",
        "mix^2",
        "mix~1",
        // The same forms on a name that exists nowhere.
        "nosuchfork~1",
        "nosuchfork^^",
        "nosuchfork^{/Alpha}",
        // Tags, into alpha's history and outside it.
        "inside",
        "inside-annotated",
        "inside~0",
        "refs/tags/inside",
        "outside",
        // Remote-tracking refs.
        "refs/remotes/origin/gamma",
        "origin/gamma",
        // Revision syntax beyond parent steps.
        "HEAD:alpha-one.md",
        "gamma:gamma-one.md",
        ":/Gamma",
        ":/Alpha",
        "HEAD^{tree}",
        "HEAD^{commit}",
        "HEAD^{/Alpha}",
        "HEAD@{1}",
        "alpha@{0}",
        "@",
        "alpha..gamma",
        "alpha...gamma",
        "^gamma",
        // What is no step: a character of more than one byte, a count
        // out of range. Refused like the rest, never a panic.
        "HEAD~é",
        "HEAD~1é",
        "alpha^€",
        "HEAD~１",
        "HEAD~99999999999999999999999999999",
        // A step past the root.
        "HEAD~50",
        "HEAD^2",
        // Other spellings of names.
        "heads/alpha",
        "ALPHA",
        "refs/heads/GAMMA",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    outside.extend([
        own.clone(),
        own[..12].to_string(),
        format!("{own}~1"),
        merged.clone(),
        format!("{merged}^1"),
        f.tip("gamma")[..12].to_string(),
        format!("{tip}:alpha-one.md"),
        tree.clone(),
    ]);
    for outside in outside {
        let error = view
            .diff("alpha", &outside, "HEAD", content())
            .expect_err("a ref that is anchored out of scope refuses");
        // A `HEAD` form is echoed re-anchored onto the mem's branch, so
        // the message is compared with that spelling blanked.
        let echoed = if outside.starts_with("HEAD") {
            outside.replacen("HEAD", "refs/heads/alpha", 1)
        } else {
            outside.clone()
        };
        assert_eq!(
            shape(&error, &echoed),
            shape(&nowhere, NOWHERE[0]),
            "{outside}"
        );
        let error = view
            .changes_since("alpha", &outside, None)
            .expect_err("no cursor");
        assert_eq!(error.code(), "INVALID_CURSOR", "{outside}: {error}");
        assert!(
            agent_notes_since("alpha", &f.gitdir, &outside, Some("refs/heads/alpha")).is_err(),
            "{outside}"
        );
    }
}

/// A branch that is spelled like the hash of a commit in scope is never
/// consulted. The diff hands the two commits its refs resolved to on to
/// the pass that pairs renames from commit notes, and that pass reads
/// them as object ids: a rename note on an unmounted branch named like
/// the tip's hash changes no reply.
#[test]
fn a_branch_spelled_like_a_hash_in_scope_changes_no_reply() {
    let f = fixture();
    let old_tip = f.tip("alpha");
    {
        let mut store = f.store_engine();
        let id = memstead_base::EntityId("alpha--alpha-two".into());
        let hash = store
            .get_entity(&id)
            .expect("the entity is there")
            .content_hash
            .clone();
        store
            .delete_entity(
                memstead_base::DeleteEntityArgs {
                    id,
                    expected_hash: memstead_base::ExpectedHash::Current(hash),
                },
                Actor::Cli,
                None,
                None,
            )
            .expect("the entity is deleted");
        create(&mut store, "alpha", "Alpha Zed");
    }
    let new_tip = f.tip("alpha");
    let replies = |view: &Engine| -> Vec<String> {
        vec![
            serde_json::to_string(&view.diff("alpha", &old_tip, &new_tip, content()).unwrap())
                .unwrap(),
            serde_json::to_string(&view.diff("alpha", "HEAD~2", "HEAD", content()).unwrap())
                .unwrap(),
            serde_json::to_string(&view.changes_since("alpha", &old_tip, None).unwrap().changes)
                .unwrap(),
        ]
    };
    let before = replies(&f.view_engine());
    assert!(before[0].contains("alpha--alpha-zed"), "{}", before[0]);

    // An unmounted branch named like the new tip's hash, one commit
    // above it, whose note claims the delete and the create were a
    // rename; and one named like the old tip's hash.
    let tree = git_out(&f.gitdir, &["rev-parse", &format!("{new_tip}^{{tree}}")]);
    let claim = git_out(
        &f.gitdir,
        &[
            "commit-tree",
            &tree,
            "-p",
            &new_tip,
            "-m",
            "memstead: rename alpha--alpha-two → alpha--alpha-zed",
        ],
    );
    git(
        &f.gitdir,
        &["update-ref", &format!("refs/heads/{new_tip}"), &claim],
    );
    git(
        &f.gitdir,
        &["update-ref", &format!("refs/heads/{old_tip}"), &claim],
    );
    assert_eq!(
        replies(&f.view_engine()),
        before,
        "a branch spelled like a hash in scope is not consulted"
    );
}
