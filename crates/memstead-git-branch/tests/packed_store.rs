//! A mem-repo is packed, and a pack loses nothing.
//!
//! The engine writes loose objects with the git library and starts
//! git's own automatic maintenance once enough have gathered. A user
//! may also pack by hand. Both rest on one promise: `git gc` at its
//! default prune grace is safe beside a running engine. These tests
//! hold an engine up, pack its git directory under it, and read back
//! everything it wrote.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use memstead_base::ops::EMPTY_TREE_SHA;
use memstead_base::vcs::Actor;
use memstead_base::{CreateEntityArgs, Engine, EntityId, RenameEntityArgs, UpdateEntityArgs};
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

fn create(engine: &mut Engine, mem: &str, title: &str) -> String {
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
        .unwrap_or_else(|e| panic!("create {title:?} in {mem:?}: {e:?}"))
        .id
        .0
}

fn hash_of(engine: &Engine, id: &str) -> Option<String> {
    engine
        .get_entity(&EntityId(id.to_string()))
        .map(|e| e.content_hash.clone())
}

fn update(engine: &mut Engine, id: &str, body: &str) {
    let args = UpdateEntityArgs {
        id: EntityId(id.to_string()),
        expected_hash: memstead_base::ExpectedHash::from_token(hash_of(engine, id)),
        sections: [("identity".to_string(), body.to_string())]
            .into_iter()
            .collect(),
        append_sections: Default::default(),
        patch_sections: Default::default(),
        sections_unset: Vec::new(),
        metadata: Default::default(),
        metadata_unset: Vec::new(),
        dry_run: false,
        declare_relations: Vec::new(),
        anchors: Vec::new(),
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    engine
        .update_entity(args, Actor::Cli, None, None)
        .unwrap_or_else(|e| panic!("update {id}: {e:?}"));
}

fn rename(engine: &mut Engine, id: &str, new_title: &str) -> String {
    let args = RenameEntityArgs {
        id: EntityId(id.to_string()),
        expected_hash: memstead_base::ExpectedHash::from_token(hash_of(engine, id)),
        new_title: new_title.to_string(),
    };
    engine
        .rename_entity(args, Actor::Cli, None, None)
        .unwrap_or_else(|e| panic!("rename {id}: {e:?}"))
        .new_id
        .0
}

/// A raw git command against the fixture's git directory. An identity
/// is set because a machine with no git configuration has none.
fn git(gitdir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .arg("--git-dir")
        .arg(gitdir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "memstead-test")
        .env("GIT_AUTHOR_EMAIL", "test@memstead.invalid")
        .env("GIT_COMMITTER_NAME", "memstead-test")
        .env("GIT_COMMITTER_EMAIL", "test@memstead.invalid")
        .output()
        .expect("git runs")
}

fn git_ok(gitdir: &Path, args: &[&str]) {
    let out = git(gitdir, args);
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The pack a user runs by hand beside a running engine, as the backup
/// guide gives it: git's `gc` at its default prune grace, leaving the
/// refs unpacked. A pack that packs the refs deletes a branch's ref
/// file under a reader, which is a different promise and not this one.
const PACK: &[&str] = &["-c", "gc.packRefs=false", "gc", "--quiet"];

/// The loose objects of a git directory, and its pack files.
fn objects(gitdir: &Path) -> (usize, usize) {
    let mut loose = 0;
    let mut packs = 0;
    for entry in std::fs::read_dir(gitdir.join("objects")).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() == 2 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
            loose += std::fs::read_dir(entry.path())
                .map(|d| d.count())
                .unwrap_or(0);
        } else if name == "pack" {
            packs = std::fs::read_dir(entry.path())
                .map(|d| {
                    d.flatten()
                        .filter(|e| e.file_name().to_string_lossy().ends_with(".pack"))
                        .count()
                })
                .unwrap_or(0);
        }
    }
    (loose, packs)
}

fn gitdir_of(root: &Path) -> PathBuf {
    std::fs::canonicalize(root.join("mem-repo").join(".git")).unwrap()
}

/// What the engine wrote: every live id with the hash it had at write
/// time.
type Written = BTreeMap<String, String>;

fn note(written: &mut Written, engine: &Engine, id: &str) {
    written.insert(
        id.to_string(),
        hash_of(engine, id).unwrap_or_else(|| panic!("{id} is in the view")),
    );
}

/// Everything `written` reads back from storage with its hash: the
/// engine drops its view of the mem and reads it again, so a lost
/// object is a failed load or a missing entity, not a stale answer.
fn reads_back(engine: &mut Engine, mem: &str, written: &Written, when: &str) {
    engine
        .reload_one_mem(mem)
        .unwrap_or_else(|e| panic!("{when}: the mem loads from storage: {e:?}"));
    for (id, hash) in written {
        assert_eq!(
            hash_of(engine, id).as_ref(),
            Some(hash),
            "{when}: {id} reads back with the hash it was written with"
        );
    }
    // The change feed walks every commit and tree of the history.
    let feed = engine
        .changes_since(mem, EMPTY_TREE_SHA, None)
        .unwrap_or_else(|e| panic!("{when}: the change feed answers: {e:?}"));
    let listed = serde_json::to_string(&feed).unwrap();
    for id in written.keys() {
        assert!(listed.contains(id.as_str()), "{when}: the feed lists {id}");
    }
    let health = engine.health();
    assert!(
        health.load_errors.is_empty(),
        "{when}: {:?}",
        health.load_errors
    );
    assert!(
        health.quarantined.is_empty(),
        "{when}: a mem is out of service"
    );
}

/// One engine that stays up writes a few hundred entities, with updates
/// and a rename among them. Its git directory is packed under it at
/// each point of the closed list: between two operations; several times
/// from outside while the engine is inside a burst of writes, by the
/// command the backup guide gives; and after the refs were packed
/// between two operations. After each pack the same engine reads back
/// everything it wrote with the hash it got, and writes on; a second
/// engine on the same git directory and a fresh one agree.
#[test]
fn a_running_engine_loses_nothing_when_its_git_directory_is_packed_under_it() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("alpha", "default@1.0.0")]);
    let gitdir = gitdir_of(tmp.path());
    // The packs of this test are the test's own: the engine's automatic
    // maintenance stays out of it.
    git_ok(&gitdir, &["config", "gc.auto", "0"]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    let mut second = engine_from_workspace_root(tmp.path()).expect("a second engine boots");
    let mut written = Written::new();

    for i in 0..120 {
        let id = create(&mut engine, "alpha", &format!("Early {i}"));
        note(&mut written, &engine, &id);
    }
    for i in (0..120).step_by(7) {
        let id = format!("alpha--early-{i}");
        update(&mut engine, &id, &format!("touched {i}"));
        note(&mut written, &engine, &id);
    }
    let renamed = rename(&mut engine, "alpha--early-3", "Early Three Renamed");
    written.remove("alpha--early-3");
    note(&mut written, &engine, &renamed);

    // 1. Between two operations.
    git_ok(&gitdir, &["gc", "--quiet"]);
    let (loose, packs) = objects(&gitdir);
    assert!(
        packs >= 1 && loose == 0,
        "packed: {loose} loose, {packs} packs"
    );
    reads_back(
        &mut engine,
        "alpha",
        &written,
        "after a pack between operations",
    );
    let id = create(&mut engine, "alpha", "After The First Pack");
    note(&mut written, &engine, &id);

    // 2. While the engine is inside a burst of writes, from outside,
    //    several times.
    let outside = gitdir.clone();
    let packer = std::thread::spawn(move || {
        for _ in 0..5 {
            git_ok(&outside, PACK);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    });
    for i in 0..160 {
        let id = create(&mut engine, "alpha", &format!("Burst {i}"));
        note(&mut written, &engine, &id);
        if i % 9 == 0 {
            update(&mut engine, &id, &format!("burst touch {i}"));
            note(&mut written, &engine, &id);
        }
    }
    packer.join().unwrap();
    git_ok(&gitdir, &["gc", "--quiet"]);
    reads_back(&mut engine, "alpha", &written, "after packs during a burst");
    let id = create(&mut engine, "alpha", "After The Burst");
    note(&mut written, &engine, &id);

    // 3. After the refs were packed.
    git_ok(&gitdir, &["pack-refs", "--all"]);
    git_ok(&gitdir, &["gc", "--quiet"]);
    reads_back(&mut engine, "alpha", &written, "after the refs were packed");
    let id = create(&mut engine, "alpha", "After Packed Refs");
    note(&mut written, &engine, &id);
    reads_back(
        &mut engine,
        "alpha",
        &written,
        "after a write onto packed refs",
    );

    // A second engine that was up all the while, and a fresh one.
    reads_back(&mut second, "alpha", &written, "a second engine");
    let id = create(&mut second, "alpha", "From The Second Engine");
    note(&mut written, &second, &id);
    let mut fresh = engine_from_workspace_root(tmp.path()).expect("a fresh engine boots");
    reads_back(&mut fresh, "alpha", &written, "a fresh engine");
    reads_back(
        &mut engine,
        "alpha",
        &written,
        "the first engine, at the end",
    );
}

/// The readback is a real one: with the pack that holds the engine's
/// objects taken away, the load from storage fails instead of serving
/// what the engine still has in memory.
#[test]
fn a_store_that_lost_its_pack_does_not_read_back() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("alpha", "default@1.0.0")]);
    let gitdir = gitdir_of(tmp.path());
    git_ok(&gitdir, &["config", "gc.auto", "0"]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    for i in 0..20 {
        create(&mut engine, "alpha", &format!("Kept {i}"));
    }
    git_ok(&gitdir, &["gc", "--quiet"]);
    for entry in std::fs::read_dir(gitdir.join("objects").join("pack"))
        .unwrap()
        .flatten()
    {
        std::fs::remove_file(entry.path()).unwrap();
    }
    assert!(
        engine.reload_one_mem("alpha").is_err(),
        "a mem whose objects are gone does not load"
    );
}

/// Forty entities in one commit: the blobs gather forty times as fast
/// as with one entity per commit, which keeps a run over git's default
/// threshold short.
fn create_many(engine: &mut Engine, mem: &str, prefix: &str, written: &mut Written) {
    let creates = (0..40)
        .map(|i| {
            let title = format!("{prefix} {i}");
            let mut sections = indexmap::IndexMap::new();
            sections.insert("identity".to_string(), format!("identity of {title}"));
            sections.insert("purpose".to_string(), format!("purpose of {title}"));
            (
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: mem.to_string(),
                    title,
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: Default::default(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                None,
            )
        })
        .collect();
    let report = engine
        .batch_create(creates, Actor::Cli, None, false)
        .unwrap_or_else(|e| panic!("batch {prefix}: {e:?}"));
    assert!(report.applied, "batch {prefix}: {report:?}");
    let slug = prefix.to_lowercase().replace(' ', "-");
    for i in 0..40 {
        note(written, engine, &format!("{mem}--{slug}-{i}"));
    }
}

/// With `git` on the path and git's default threshold, a git directory
/// the engine writes into is packed once its loose objects cross git's
/// own estimate, and stays bounded over as many writes again. git
/// judges by the objects whose hash begins with `17`, more than 27 of
/// them, so the number of writes it takes varies; the run is long
/// enough to cross it.
#[test]
fn a_store_packs_itself_past_gits_default_threshold() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("alpha", "default@1.0.0")]);
    let gitdir = gitdir_of(tmp.path());
    // git detaches its automatic maintenance by default; the test waits
    // for the pack instead of racing it.
    git_ok(&gitdir, &["config", "gc.autoDetach", "false"]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    let mut written = Written::new();

    let bucket = gitdir.join("objects").join("17");
    let in_bucket = || std::fs::read_dir(&bucket).map(|d| d.count()).unwrap_or(0);
    let mut high_water = 0;
    let mut most_loose = 0;
    let mut packed_after = None;
    for batch in 0..600 {
        create_many(
            &mut engine,
            "alpha",
            &format!("Early {batch}"),
            &mut written,
        );
        if objects(&gitdir).1 >= 1 {
            packed_after = Some(batch + 1);
            break;
        }
        high_water = high_water.max(in_bucket());
        most_loose = most_loose.max(objects(&gitdir).0);
    }
    let packed_after = packed_after.unwrap_or_else(|| {
        panic!("no pack after 600 commits; the bucket reached {high_water} objects")
    });
    // The commit that brings the bucket past 27 starts the pack, which
    // empties it; one commit adds a few objects to the bucket at most,
    // so a look between commits finds it close below.
    assert!(
        high_water >= 20,
        "the pack came at git's threshold, not long before: bucket at {high_water}"
    );
    assert!(
        most_loose > 4000,
        "and not below it: {most_loose} loose objects"
    );
    let (loose, _) = objects(&gitdir);
    assert!(loose < 6700, "{loose} loose objects after the pack");
    // The engine's pack leaves the refs unpacked: the branch is still a
    // ref file, which a reader never finds deleted under it.
    assert!(
        gitdir.join("refs").join("heads").join("alpha").is_file(),
        "the branch's ref file is where it was"
    );

    // As many commits again: the store stays bounded.
    let mut most_loose = 0;
    for batch in 0..packed_after {
        create_many(
            &mut engine,
            "alpha",
            &format!("Later {batch}"),
            &mut written,
        );
        most_loose = most_loose.max(objects(&gitdir).0);
    }
    assert!(
        most_loose < 2 * 6700 + 2000,
        "the loose objects stayed bounded: {most_loose} at most"
    );
    reads_back(&mut engine, "alpha", &written, "after the automatic packs");
    let mut fresh = engine_from_workspace_root(tmp.path()).expect("a fresh engine boots");
    reads_back(
        &mut fresh,
        "alpha",
        &written,
        "a fresh engine after the automatic packs",
    );
}

/// `gc.auto = 0` in the git directory's configuration switches the
/// automatic pack off: the same kind of run leaves every object loose.
#[test]
fn gits_own_switch_turns_the_automatic_pack_off() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[("alpha", "default@1.0.0"), ("beta", "default@1.0.0")],
    );
    let gitdir = gitdir_of(tmp.path());
    git_ok(&gitdir, &["config", "gc.autoDetach", "false"]);

    // A low threshold, so that a short run crosses it: with it the
    // store packs, with `0` it does not.
    for (auto, expect_pack) in [("256", true), ("0", false)] {
        let mem = if expect_pack { "alpha" } else { "beta" };
        if !expect_pack {
            // Start the second half from a store with nothing loose
            // and no pack to mistake for a new one.
            git_ok(&gitdir, &["gc", "--quiet"]);
        }
        git_ok(&gitdir, &["config", "gc.auto", auto]);
        let packs_before = objects(&gitdir).1;
        let mut engine = engine_from_workspace_root(tmp.path()).unwrap();
        for i in 0..700 {
            create(&mut engine, mem, &format!("Row {i}"));
        }
        let (loose, packs) = objects(&gitdir);
        if expect_pack {
            assert!(packs > packs_before, "gc.auto = {auto}: a pack was written");
        } else {
            assert_eq!(packs, packs_before, "gc.auto = 0: no pack was written");
            assert!(loose >= 2000, "gc.auto = 0: every object is loose: {loose}");
        }
    }
}
