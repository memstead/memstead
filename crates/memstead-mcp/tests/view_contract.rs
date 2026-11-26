//! The view contract, held over every tool the server lists.
//!
//! A store is a workspace with a mem-repo of its own. A view is a
//! workspace of engine state only that mounts some of the store's mems
//! out of the store's git directory. What a caller of
//! `memstead-mcp --workspace <view>` (without operator mode) may learn
//! and do is bounded by what the view mounts:
//!
//! - Nothing of a mem the view does not mount reaches a reply or a file
//!   of the view: not its text, not its name, not its schema, not an
//!   entity id. The one exception is an id that a mounted mem links to,
//!   which appears as a stub without content.
//! - A call that names such a mem, one of its entities, its schema or a
//!   commit of its branch answers exactly as the same call naming
//!   something that exists nowhere.
//! - A read-only mount refuses every write and its branch never moves.
//! - A write mount takes entity writes and the three settings of a mem;
//!   no other ref of the store moves, and mems are neither created nor
//!   deleted from a view.
//!
//! The fixture is built with the real `memstead` binary and the server
//! is the real `memstead-mcp` binary over stdio. The store holds:
//!
//! - `notes`, mounted writable in the view;
//! - `facts`, mounted read-only in the view;
//! - `annex`, not mounted, with links into both mounted mems and one
//!   entity (`annex--one`) that `notes` links to;
//! - `cellar`, not mounted, linked from nowhere, pinned to the schema
//!   `crateshape` that no other mem pins.
//!
//! The table of calls below is checked against `tools/list`: a tool the
//! server lists and the table does not call fails the test, so a tool
//! added later is in range from its first day.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use tempfile::TempDir;

/// The mem the view mounts writable, and its branch.
const WRITE_BRANCH: &str = "refs/heads/notes";
const REGISTRY_REF: &str = "refs/heads/__MEMSTEAD";
/// Where the write mount's check rows live, beside its branch.
const WRITE_CHECKS_REF: &str = "refs/memstead/checks/notes";

/// A mem of the store the view does not mount, and a name no mem has.
const UNMOUNTED: &str = "annex";
const NOWHERE: &str = "nomem";
/// The schema only an unmounted mem pins, and a name no schema has.
const FAR_SCHEMA: &str = "crateshape";
const NOWHERE_SCHEMA: &str = "blankshape";
/// A word only an unmounted mem's text holds, and one no text holds.
const FAR_WORD: &str = "zebrafinch";
const NOWHERE_WORD: &str = "blankword";

/// The one id of an unmounted mem the view may know: `notes--three`
/// links to it, so it is content of a mounted mem.
const STUB: &str = "annex--one";

/// What must never reach the view unless the request named it: the text
/// of the unmounted mems, the schema only they pin, the name of the mem
/// nothing links to, and the ids nothing links to.
const CANARIES: [&str; 7] = [
    FAR_WORD,
    "quetzal",
    "kingfisher",
    FAR_SCHEMA,
    "cellar",
    "annex--two",
    "annex--zebrafinch-plan",
];

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const NO_COMMIT: &str = "1234567890abcdef1234567890abcdef12345678";
const NO_HASH: &str = "0000000000000000";

const AUTHOR: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "test"),
    ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
    ("GIT_COMMITTER_NAME", "test"),
    ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
];

// ---------------------------------------------------------------------
// The binaries
// ---------------------------------------------------------------------

fn memstead_mcp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_memstead-mcp")
}

/// The `memstead` binary of the same build, beside the server's. This
/// crate does not depend on the CLI crate, so cargo names no path for
/// it; a workspace build puts both into one directory.
fn memstead_bin() -> PathBuf {
    let cli = Path::new(memstead_mcp_bin())
        .with_file_name(format!("memstead{}", std::env::consts::EXE_SUFFIX));
    assert!(
        cli.is_file(),
        "the fixture needs the `memstead` binary at {}: build the workspace, or run \
         `cargo build -p memstead-cli`",
        cli.display()
    );
    cli
}

/// One fixture step with the `memstead` binary; it must succeed.
fn memstead(ws: &Path, args: &[&str]) {
    let out = Command::new(memstead_bin())
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .envs(AUTHOR)
        .env_remove("MEMSTEAD_WORKSPACE")
        .env_remove("MEMSTEAD_IDENTITY")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "fixture step `memstead {}` failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
}

fn git(gitdir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(gitdir)
        .args(args)
        .envs(AUTHOR)
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

// ---------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------

struct Fixture {
    root: TempDir,
    store: PathBuf,
    view: PathBuf,
    gitdir: PathBuf,
}

fn spec<'a>(mem: &'a str, title: &'a str, identity: &'a str, purpose: &'a str) -> Vec<&'a str> {
    vec![
        "create",
        "--mem",
        mem,
        "--title",
        title,
        "--type",
        "spec",
        "--metadata",
        "level=M0",
        "--section",
        identity,
        "--section",
        purpose,
    ]
}

fn build() -> Fixture {
    let root = TempDir::new().unwrap();
    let store = root.path().join("store");
    let view = root.path().join("view");
    let pkg = root.path().join("pkg");
    for dir in [&store, &view, &pkg] {
        std::fs::create_dir_all(dir).unwrap();
    }
    memstead(&store, &["mem-repo", "init", "."]);
    memstead(&store, &["workspace", "allow-create", "--schema", "*", "*"]);
    memstead(&pkg, &["schema", "new", FAR_SCHEMA]);
    memstead(
        &store,
        &["schema", "install", pkg.join(FAR_SCHEMA).to_str().unwrap()],
    );
    for mem in ["notes", "facts"] {
        memstead(&store, &["mem", "init", mem, "--schema", "default@1.3.0"]);
        memstead(&store, &spec(mem, "One", "identity=i", "purpose=p"));
        memstead(&store, &spec(mem, "Two", "identity=i", "purpose=p"));
    }
    memstead(
        &store,
        &["mem", "init", UNMOUNTED, "--schema", "default@1.3.0"],
    );
    memstead(
        &store,
        &spec(
            UNMOUNTED,
            "One",
            "identity=the kingfisher identity",
            "purpose=p",
        ),
    );
    memstead(
        &store,
        &spec(
            UNMOUNTED,
            "Two",
            "identity=the kingfisher sibling",
            "purpose=p",
        ),
    );
    memstead(
        &store,
        &["mem", "init", "cellar", "--schema", "crateshape@0.1.0"],
    );
    memstead(
        &store,
        &[
            "create",
            "--mem",
            "cellar",
            "--title",
            "Quetzal Ledger",
            "--type",
            "note",
            "--section",
            "summary=the quetzal summary",
        ],
    );
    for (from, to) in [
        (UNMOUNTED, "notes"),
        ("notes", UNMOUNTED),
        (UNMOUNTED, "facts"),
    ] {
        memstead(&store, &["workspace", "grant-cross-link", from, to]);
    }
    memstead(
        &store,
        &spec(
            UNMOUNTED,
            "Zebrafinch Plan",
            "identity=the zebrafinch identity, see [[notes--one]] and [[facts--one]]",
            "purpose=zebrafinch purpose",
        ),
    );
    memstead(
        &store,
        &spec(
            "notes",
            "Three",
            "identity=points at [[annex--one]]",
            "purpose=p",
        ),
    );
    let gitdir = store.join("mem-repo/.git").canonicalize().unwrap();
    let mount = |mem: &str, capability: &str, first: bool| {
        let mut args = vec![
            "mem",
            "mount",
            mem,
            "--branch",
            mem,
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            capability,
            "--operator-mode",
        ];
        if first {
            args.push("--new-workspace");
        }
        memstead(&view, &args);
    };
    mount("notes", "write", true);
    mount("facts", "read-only", false);
    assert!(!view.join("mem-repo").exists(), "a view has no mem-repo");
    let view = view.canonicalize().unwrap();
    Fixture {
        root,
        store,
        view,
        gitdir,
    }
}

/// Every ref of the store's git directory, every file of the view, and
/// every file of the store outside its mem-repo.
#[derive(PartialEq)]
struct State {
    refs: BTreeMap<String, String>,
    view: BTreeMap<String, Vec<u8>>,
    store: BTreeMap<String, Vec<u8>>,
}

fn files(top: &Path, skip: Option<&Path>) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut stack = vec![top.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if Some(path.as_path()) == skip {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(top)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                found.insert(relative, std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    found
}

fn state(fx: &Fixture) -> State {
    let refs = git(
        &fx.gitdir,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    )
    .lines()
    .map(|line| {
        let (name, target) = line.split_once(' ').unwrap();
        (name.to_string(), target.to_string())
    })
    .collect();
    State {
        refs,
        view: files(&fx.view, None),
        store: files(&fx.store, Some(&fx.store.join("mem-repo"))),
    }
}

fn differing<V: PartialEq>(
    before: &BTreeMap<String, V>,
    after: &BTreeMap<String, V>,
) -> Vec<String> {
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter(|key| before.get(*key) != after.get(*key))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------

struct Reply {
    is_error: bool,
    /// Every text block, then the structured content as JSON.
    text: String,
}

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl Session {
    /// The server on the view, as an agent's client starts it: no
    /// operator mode.
    fn start(fx: &Fixture) -> Self {
        let mut child = Command::new(memstead_mcp_bin())
            .arg("--workspace")
            .arg(&fx.view)
            .current_dir(fx.root.path())
            .envs(AUTHOR)
            .env_remove("MEMSTEAD_WORKSPACE")
            .env_remove("MEMSTEAD_IDENTITY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn memstead-mcp");
        let stdin = child.stdin.take();
        let reader = BufReader::new(child.stdout.take().unwrap());
        let mut session = Self {
            child,
            stdin,
            reader,
            next_id: 0,
        };
        session.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "view-contract-test", "version": "0" }
            }),
        );
        session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        session
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).unwrap();
            assert!(read > 0, "memstead-mcp exited before answering {method}");
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && value["id"] == json!(id)
            {
                return value;
            }
        }
    }

    fn tools(&mut self) -> BTreeSet<String> {
        let reply = self.request("tools/list", json!({}));
        reply["result"]["tools"]
            .as_array()
            .expect("tools/list answers a list")
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn call(&mut self, tool: &str, arguments: &Value) -> Reply {
        let reply = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        let Some(result) = reply.get("result") else {
            // A refusal below the tool, such as parameters that do not
            // deserialize.
            return Reply {
                is_error: true,
                text: reply.to_string(),
            };
        };
        let mut text = result["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if let Some(structured) = result.get("structuredContent").filter(|s| !s.is_null()) {
            text.push('\n');
            text.push_str(&structured.to_string());
        }
        Reply {
            is_error: result["isError"].as_bool().unwrap_or(false),
            text,
        }
    }

    fn hash_of(&mut self, id: &str) -> String {
        let reply = self.call("memstead_entity", &json!({ "id": id }));
        let marker = "_hash: ";
        let at = reply
            .text
            .find(marker)
            .unwrap_or_else(|| panic!("no hash of {id}: {}", reply.text));
        reply.text[at + marker.len()..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------
// The table of calls
// ---------------------------------------------------------------------

/// What a paired call names: the unmounted mem and what belongs to it,
/// or the same shapes for names that exist nowhere.
#[derive(Clone)]
struct Subject {
    mem: &'static str,
    schema: &'static str,
    word: &'static str,
    /// The tip and the first commit of the mem's branch, and a blob of
    /// it.
    tip: String,
    first: String,
    blob: String,
}

impl Subject {
    fn there(fx: &Fixture) -> Self {
        let branch = format!("refs/heads/{UNMOUNTED}");
        Self {
            mem: UNMOUNTED,
            schema: FAR_SCHEMA,
            word: FAR_WORD,
            tip: git(&fx.gitdir, &["rev-parse", &branch]),
            first: git(&fx.gitdir, &["rev-list", "--max-parents=0", &branch])
                .lines()
                .next()
                .unwrap()
                .to_string(),
            blob: git(
                &fx.gitdir,
                &["rev-parse", &format!("{branch}:zebrafinch-plan.md")],
            ),
        }
    }

    fn nowhere() -> Self {
        Self {
            mem: NOWHERE,
            schema: NOWHERE_SCHEMA,
            word: NOWHERE_WORD,
            tip: NO_COMMIT.to_string(),
            first: NO_COMMIT.to_string(),
            blob: NO_COMMIT.to_string(),
        }
    }

    fn entity(&self) -> String {
        format!("{}--two", self.mem)
    }

    fn pin(&self) -> String {
        format!("{}@0.1.0", self.schema)
    }
}

/// What a call takes from the fixture it runs in.
struct Ctx {
    gitdir: String,
    notes_tip: String,
    facts_one: String,
    notes_one: String,
    notes_two: String,
    notes_three: String,
}

impl Ctx {
    fn of(fx: &Fixture, session: &mut Session) -> Self {
        Self {
            gitdir: fx.gitdir.to_str().unwrap().to_string(),
            notes_tip: git(&fx.gitdir, &["rev-parse", WRITE_BRANCH]),
            facts_one: session.hash_of("facts--one"),
            notes_one: session.hash_of("notes--one"),
            notes_two: session.hash_of("notes--two"),
            notes_three: session.hash_of("notes--three"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Expect {
    /// A read: it may answer or refuse, and it changes nothing.
    Reads,
    /// Must refuse and change nothing.
    Refuses,
    /// May answer or refuse; nothing but the write mount's branch may
    /// move.
    MayWrite,
    /// Must answer and move the write mount's branch.
    Writes,
    /// Must answer and move the write mount's checks ref, and no other.
    Records,
    /// Must answer and move the registry ref, where the settings of a
    /// mem live, and no other.
    Sets,
}

impl Expect {
    /// Whether calls of this kind may share one fixture: they are
    /// expected to change nothing.
    fn shares_a_fixture(self) -> bool {
        matches!(self, Expect::Reads | Expect::Refuses)
    }

    fn refs_that_may_move(self) -> &'static [&'static str] {
        match self {
            Expect::Reads | Expect::Refuses => &[],
            Expect::Records => &[WRITE_CHECKS_REF],
            // The registry ref beside the branch: the engine stamps its
            // version into the mem's configuration after an entity
            // write. The checks ref: a gated write carries the check
            // that licensed it.
            Expect::MayWrite | Expect::Writes => &[WRITE_BRANCH, REGISTRY_REF, WRITE_CHECKS_REF],
            Expect::Sets => &[REGISTRY_REF],
        }
    }

    fn view_files_that_may_change(self) -> &'static [&'static str] {
        // The friction ledger records every refusal, by code alone.
        const FRICTION: &str = ".memstead/state/friction/";
        const CHECKS: &str = ".memstead/state/checks/";
        const ROSTER: &str = ".memstead/state/mounts.json";
        match self {
            Expect::Reads | Expect::Refuses => &[FRICTION],
            Expect::MayWrite | Expect::Writes | Expect::Records => &[FRICTION, CHECKS],
            Expect::Sets => &[FRICTION, ROSTER],
        }
    }
}

struct Probe {
    name: &'static str,
    tool: &'static str,
    expect: Expect,
    /// Whether the call is made a second time for names that exist
    /// nowhere, and must then answer identically.
    paired: bool,
    args: fn(&Subject, &Ctx) -> Value,
}

fn full_read(id: &str) -> Value {
    json!({ "id": id, "include_relations": true, "include_context": true, "include_provenance": true })
}

fn new_spec(mem: &str, title: &str, identity: &str) -> Value {
    json!({
        "mem": mem,
        "title": title,
        "entity_type": "spec",
        "metadata": { "level": "M0" },
        "sections": { "identity": identity, "purpose": "p" }
    })
}

fn relation(from: &str, to: &str) -> Value {
    json!({ "relations": [{ "from": from, "to": to, "rel_type": "USES" }] })
}

fn check(entity: &str) -> Value {
    json!({ "entity": entity, "verdict": "ok", "method": "m" })
}

/// The arguments of a mem create, with `more` laid over them.
fn new_mem(name: &str, schema: &str, more: Value) -> Value {
    let mut args = json!({ "name": name, "location": name, "schema": schema });
    for (key, value) in more.as_object().unwrap() {
        args[key.as_str()] = value.clone();
    }
    args
}

const OVERVIEW_PARTS: [&str; 4] = [
    "community_members",
    "community_bridges",
    "mem_distribution",
    "dangling_links",
];

const HEALTH_AXES: [&str; 21] = [
    "orphans",
    "stubs",
    "most_connected",
    "missing_fields",
    "stale",
    "dangling_links",
    "tags",
    "missing_required_outgoing",
    "constraints",
    "conformance",
    "integrity",
    "config",
    "anchors",
    "friction",
    "open_questions",
    "stale_derivations",
    "checks",
    "signals",
    "labelling",
    "ledger",
    "vital_signs",
];

/// One row per call: the expectation, `paired` where the call is made a
/// second time for names that exist nowhere, the call's name, the tool,
/// and the arguments as a function of the subject and of the fixture.
/// A macro, so that a row stays on one line where it fits.
macro_rules! probes {
    (@paired) => { false };
    (@paired paired) => { true };
    ($(
        $expect:ident $($paired:ident)? $name:literal => $tool:ident
        |$subject:pat_param, $ctx:pat_param| $args:expr;
    )*) => {
        vec![$(
            Probe {
                name: $name,
                tool: stringify!($tool),
                expect: Expect::$expect,
                paired: probes!(@paired $($paired)?),
                args: |$subject: &Subject, $ctx: &Ctx| -> Value { $args },
            }
        ),*]
    };
}

fn probes() -> Vec<Probe> {
    probes! {
        // ---- Reads --------------------------------------------------
        Reads "overview" => memstead_overview |_, _| json!({});
        Reads "overview, all it can include" => memstead_overview |_, _|
            json!({ "include": OVERVIEW_PARTS, "rebuild": true });
        Reads paired "overview of the mem" => memstead_overview |s, _| json!({ "mem": s.mem });
        Reads "search for common words" => memstead_search |_, _|
            json!({ "query": { "any": ["plan", "identity", "purpose", "ledger", "summary", "one", "two"] } });
        Reads paired "search for a word of its text" => memstead_search |s, _|
            json!({ "query": { "any": [s.word] } });
        Reads paired "search for a phrase of its text" => memstead_search |s, _|
            json!({ "query": { "phrase": format!("the {} identity", s.word) } });
        Reads "search, everything" => memstead_search |_, _| json!({ "limit": 50 });
        Reads "search, stubs" => memstead_search |_, _| json!({ "stub": true, "limit": 50 });
        Reads "search by a type of its schema" => memstead_search |_, _| json!({ "entity_type": "note" });
        Reads paired "search in the mem" => memstead_search |s, _|
            json!({ "mem": s.mem, "query": { "any": ["one"] } });
        Reads paired "search related to its entity" => memstead_search |s, _|
            json!({ "related_to": s.entity() });
        Reads "search related to the stub" => memstead_search |_, _|
            json!({ "related_to": STUB, "depth": 3 });
        Reads "search related to notes--one" => memstead_search |_, _|
            json!({ "related_to": "notes--one", "depth": 3 });
        Reads "search related to facts--one" => memstead_search |_, _|
            json!({ "related_to": "facts--one", "depth": 3 });
        Reads paired "entity of the mem" => memstead_entity |s, _| full_read(&s.entity());
        Reads paired "entity of the mem that links in" => memstead_entity |s, _|
            json!({ "id": format!("{}--zebrafinch-plan", s.mem) });
        Reads "entity by its bare slug" => memstead_entity |_, _| json!({ "id": "zebrafinch-plan" });
        Reads "entity by a slug every mem has" => memstead_entity |_, _| json!({ "id": "two" });
        Reads "entity, a near miss" => memstead_entity |_, _| json!({ "id": "notes--zebrafinch" });
        Reads "entity, the stub" => memstead_entity |_, _| full_read(STUB);
        Reads "entity notes--one" => memstead_entity |_, _| full_read("notes--one");
        Reads "entity notes--three" => memstead_entity |_, _| full_read("notes--three");
        Reads "entity facts--one" => memstead_entity |_, _| full_read("facts--one");
        Reads "schema, no argument" => memstead_schema |_, _| json!({});
        Reads paired "schema of the mem" => memstead_schema |s, _| json!({ "mem": s.mem });
        Reads paired "schema by pin" => memstead_schema |s, _| json!({ "name": s.pin() });
        Reads paired "schema by bare name" => memstead_schema |s, _| json!({ "name": s.schema });
        Reads "health, every axis" => memstead_health |_, _|
            json!({ "include": HEALTH_AXES, "include_config": true, "limit": 100 });
        Reads paired "health of the mem" => memstead_health |s, _| json!({ "mem": s.mem });
        Reads paired "health against its schema" => memstead_health |s, _|
            json!({ "include": ["conformance"], "target_schema": s.pin() });
        Reads paired "changes of the mem" => memstead_changes_since |s, _|
            json!({ "mem": s.mem, "since": EMPTY_TREE });
        Reads "changes of notes" => memstead_changes_since |_, _|
            json!({ "mem": "notes", "since": EMPTY_TREE, "include_notes": true });
        Reads paired "changes since its tip" => memstead_changes_since |s, _|
            json!({ "mem": "notes", "since": s.tip });
        Reads paired "changes since its first commit" => memstead_changes_since |s, _|
            json!({ "mem": "notes", "since": s.first });
        Reads paired "diff of the mem" => memstead_diff |s, _|
            json!({ "mem": s.mem, "ref_a": EMPTY_TREE, "ref_b": "HEAD" });
        Reads paired "diff from its tip" => memstead_diff |s, c|
            json!({ "mem": "notes", "ref_a": s.tip, "ref_b": c.notes_tip, "include_content": true });
        Reads paired "diff to its tip" => memstead_diff |s, c|
            json!({ "mem": "notes", "ref_a": c.notes_tip, "ref_b": s.tip, "include_content": true });
        Reads paired "diff from a blob of it" => memstead_diff |s, c|
            json!({ "mem": "notes", "ref_a": s.blob, "ref_b": c.notes_tip });
        Reads paired "diff from its branch name" => memstead_diff |s, c|
            json!({ "mem": "notes", "ref_a": s.mem, "ref_b": c.notes_tip, "include_content": true });
        Reads paired "diff from its full ref" => memstead_diff |s, c|
            json!({
                "mem": "notes",
                "ref_a": format!("refs/heads/{}", s.mem),
                "ref_b": c.notes_tip,
                "include_content": true
            });
        Reads "diff from the registry ref" => memstead_diff |_, c|
            json!({ "mem": "notes", "ref_a": "__MEMSTEAD", "ref_b": c.notes_tip, "include_content": true });
        Reads "diff of the registry ref" => memstead_diff |_, _|
            json!({
                "mem": "notes",
                "ref_a": REGISTRY_REF,
                "ref_b": REGISTRY_REF,
                "include_content": true
            });
        Reads "diff by a commit search" => memstead_diff |_, c|
            json!({
                "mem": "notes",
                "ref_a": format!("{}^{{/zebrafinch}}", c.notes_tip),
                "ref_b": c.notes_tip
            });
        Reads paired "proposal brief of the mem" => memstead_proposal_brief |s, _|
            json!({ "fork": s.mem });
        Reads "proposal brief of notes" => memstead_proposal_brief |_, _| json!({ "fork": "notes" });
        Reads paired "reload of the mem" => memstead_reload |s, _| json!({ "mem": s.mem });
        Reads "reload, full" => memstead_reload |_, _| json!({ "full": true });

        // ---- Writes against a mem the view does not mount -----------
        Refuses paired "create in the mem" => memstead_create |s, _| new_spec(s.mem, "Fresh", "i");
        Refuses paired "update its entity" => memstead_update |s, _|
            json!({ "id": s.entity(), "sections": { "purpose": "x" }, "expected_hash": NO_HASH });
        Refuses paired "delete its entity" => memstead_delete |s, _|
            json!({ "id": s.entity(), "expected_hash": NO_HASH });
        Refuses paired "rename its entity" => memstead_rename |s, _|
            json!({ "id": s.entity(), "new_title": "Renamed", "expected_hash": NO_HASH });
        Refuses paired "retype its entity" => memstead_retype |s, _|
            json!({ "id": s.entity(), "target_type": "memo" });
        Refuses paired "relate from its entity" => memstead_relate |s, _|
            relation(&s.entity(), "notes--one");
        MayWrite paired "relate to its entity" => memstead_relate |s, _|
            relation("notes--one", &s.entity());
        MayWrite paired "create linking to its entity" => memstead_create |s, _|
            new_spec("notes", "Linker", &format!("see [[{}]]", s.entity()));
        Refuses paired "check its entity" => memstead_check |s, _| check(&s.entity());
        Refuses "check the stub" => memstead_check |_, _| check(STUB);
        Refuses paired "configure the mem" => memstead_mem_configure |s, _|
            json!({ "name": s.mem, "description": "d" });
        Refuses paired "set the mem's schema" => memstead_mem_set_schema |s, _|
            json!({ "mem": s.mem, "schema": "default@1.2.0" });
        Refuses paired "set the mem's version" => memstead_mem_set_version |s, _|
            json!({ "name": s.mem, "version": "0.2.0" });
        Refuses paired "delete the mem" => memstead_mem_delete |s, _| json!({ "name": s.mem });
        Refuses paired "create a mem of its name" => memstead_mem_create |s, _|
            new_mem(s.mem, "default@1.3.0", json!({}));
        Refuses paired "create a mem of its name, reattaching" => memstead_mem_create |s, _|
            new_mem(s.mem, "default@1.3.0", json!({ "recovery": "reattach" }));
        Refuses paired "create a mem of its name in the store's git directory" => memstead_mem_create |s, c|
            new_mem(
                s.mem,
                "default@1.3.0",
                json!({ "recovery": "reattach", "vcs": { "gitdir": c.gitdir, "worktree": "." } }),
            );
        Refuses paired "create a mem on its schema" => memstead_mem_create |s, _|
            new_mem("fresh", &s.pin(), json!({}));

        // ---- Writes against the read-only mount ---------------------
        Refuses "read-only: create" => memstead_create |_, _| new_spec("facts", "Fresh", "i");
        Refuses "read-only: update" => memstead_update |_, c|
            json!({ "id": "facts--one", "sections": { "purpose": "x" }, "expected_hash": c.facts_one });
        Refuses "read-only: update of anchors alone" => memstead_update |_, _|
            json!({
                "id": "facts--one",
                "anchors": [{ "artifact": "a.txt", "grain": "file", "class": "informed-by" }]
            });
        Refuses "read-only: delete" => memstead_delete |_, c|
            json!({ "id": "facts--one", "expected_hash": c.facts_one });
        Refuses "read-only: rename" => memstead_rename |_, c|
            json!({ "id": "facts--one", "new_title": "Renamed", "expected_hash": c.facts_one });
        Refuses "read-only: retype" => memstead_retype |_, c|
            json!({ "id": "facts--one", "target_type": "memo", "expected_hash": c.facts_one });
        Refuses "read-only: relate from" => memstead_relate |_, _| relation("facts--one", "notes--one");
        MayWrite "read-only: relate to" => memstead_relate |_, _| relation("notes--one", "facts--one");
        Refuses "read-only: check" => memstead_check |_, _| check("facts--one");
        Refuses "read-only: configure" => memstead_mem_configure |_, _|
            json!({ "name": "facts", "description": "d" });
        Refuses "read-only: set schema" => memstead_mem_set_schema |_, _|
            json!({ "mem": "facts", "schema": "default@1.2.0" });
        Refuses "read-only: set version" => memstead_mem_set_version |_, _|
            json!({ "name": "facts", "version": "0.2.0" });
        Refuses "read-only: delete the mem" => memstead_mem_delete |_, _| json!({ "name": "facts" });

        // ---- The settings of the write mount, and what no view does --
        Sets "write mount: configure" => memstead_mem_configure |_, _|
            json!({ "name": "notes", "description": "d", "title": "T" });
        Sets "write mount: set schema" => memstead_mem_set_schema |_, _|
            json!({ "mem": "notes", "schema": "default@1.2.0" });
        Sets "write mount: set version" => memstead_mem_set_version |_, _|
            json!({ "name": "notes", "version": "0.2.0" });
        Refuses paired "write mount: set the schema no mounted mem pins" => memstead_mem_set_schema |s, _|
            json!({ "mem": "notes", "schema": s.pin() });
        Refuses "write mount: delete the mem" => memstead_mem_delete |_, _| json!({ "name": "notes" });
        Refuses "create a mem" => memstead_mem_create |_, _| new_mem("fresh", "default@1.3.0", json!({}));

        // ---- The write mount takes entity writes --------------------
        Writes "write mount: create" => memstead_create |_, _| new_spec("notes", "Fresh", "i");
        Writes "write mount: update" => memstead_update |_, c|
            json!({ "id": "notes--one", "sections": { "purpose": "x" }, "expected_hash": c.notes_one });
        Writes "write mount: delete" => memstead_delete |_, c|
            json!({ "id": "notes--two", "expected_hash": c.notes_two });
        Writes "write mount: rename" => memstead_rename |_, c|
            json!({ "id": "notes--one", "new_title": "Renamed", "expected_hash": c.notes_one });
        Writes "write mount: rename the entity that links out" => memstead_rename |_, c|
            json!({ "id": "notes--three", "new_title": "Renamed", "expected_hash": c.notes_three });
        Writes "write mount: delete the entity that links out" => memstead_delete |_, c|
            json!({ "id": "notes--three", "expected_hash": c.notes_three });
        Writes "write mount: relate" => memstead_relate |_, _| relation("notes--one", "notes--two");
        Records "write mount: check" => memstead_check |_, _| check("notes--one");
    }
}

// ---------------------------------------------------------------------
// Observing one call
// ---------------------------------------------------------------------

/// The canaries `text` holds that `named` (the request, lower case)
/// does not name. The stub id is taken out first: it is content of a
/// mounted mem.
fn canaries_in(text: &str, named: &str) -> Vec<&'static str> {
    let text = text.to_lowercase().replace(STUB, "");
    CANARIES
        .into_iter()
        .filter(|canary| text.contains(canary) && !named.contains(canary))
        .collect()
}

/// Replace, after every `key` followed by `:` (quoted or not), the run
/// of bytes `value` accepts by `token`.
fn mask_after(text: &str, key: &str, value: fn(u8) -> bool, token: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut rest = 0;
    let mut from = 0;
    while let Some(found) = text[from..].find(key) {
        let mut at = from + found + key.len();
        from = at;
        if bytes.get(at) == Some(&b'"') {
            at += 1;
        }
        if bytes.get(at) != Some(&b':') {
            continue;
        }
        at += 1;
        if bytes.get(at) == Some(&b' ') {
            at += 1;
        }
        if bytes.get(at) == Some(&b'"') {
            at += 1;
        }
        let mut end = at;
        while end < bytes.len() && value(bytes[end]) {
            end += 1;
        }
        if end > at {
            out.push_str(&text[rest..at]);
            out.push_str(token);
            rest = end;
            from = end;
        }
    }
    out.push_str(&text[rest..]);
    out
}

/// The length of a timestamp `2026-01-02T03:04:05Z` (fraction allowed)
/// at the head of `bytes`.
fn timestamp_len(bytes: &[u8]) -> Option<usize> {
    const SHAPE: &[u8] = b"dddd-dd-ddTdd:dd:dd";
    if bytes.len() < SHAPE.len() + 1 {
        return None;
    }
    for (byte, shape) in bytes.iter().zip(SHAPE) {
        let fits = match shape {
            b'd' => byte.is_ascii_digit(),
            other => byte == other,
        };
        if !fits {
            return None;
        }
    }
    let mut at = SHAPE.len();
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
    }
    (bytes.get(at) == Some(&b'Z')).then_some(at + 1)
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

/// Commit hashes (a word of exactly forty hex digits), timestamps and
/// quote markers, each replaced by a token.
fn mask_runs(text: &str) -> String {
    const QUOTE: &[u8] = b"memstead-quote-";
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let word_start = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        if bytes[at..].starts_with(QUOTE) {
            at += QUOTE.len();
            while at < bytes.len() && is_lower_hex(bytes[at]) {
                at += 1;
            }
            out.extend_from_slice(b"<Q>");
        } else if word_start && let Some(len) = timestamp_len(&bytes[at..]) {
            at += len;
            out.extend_from_slice(b"<TS>");
        } else if word_start && is_lower_hex(bytes[at]) {
            let mut end = at;
            while end < bytes.len() && is_lower_hex(bytes[end]) {
                end += 1;
            }
            let word_end = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if end - at == 40 && word_end {
                out.extend_from_slice(b"<SHA>");
            } else {
                out.extend_from_slice(&bytes[at..end]);
            }
            at = end;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).expect("only ASCII runs were replaced")
}

/// A reply with everything taken out that may differ between a call for
/// the unmounted mem and the same call for names that exist nowhere:
/// the two names of each pair, the fixture's directory, commit hashes,
/// timestamps, quote markers, token counts and content hashes.
fn normalised(text: &str, fx: &Fixture) -> String {
    let mut text = text.to_string();
    for (there, nowhere, token) in [
        (FAR_SCHEMA, NOWHERE_SCHEMA, "<SCHEMA>"),
        (FAR_WORD, NOWHERE_WORD, "<WORD>"),
        (UNMOUNTED, NOWHERE, "<MEM>"),
    ] {
        text = text.replace(there, token).replace(nowhere, token);
    }
    let canonical_root = fx.root.path().canonicalize().unwrap();
    text = text
        .replace(canonical_root.to_str().unwrap(), "<ROOT>")
        .replace(fx.root.path().to_str().unwrap(), "<ROOT>");
    text = mask_runs(&text);
    text = mask_after(&text, "_tokens", |b| b.is_ascii_digit(), "<N>");
    for key in ["write_id", "hash"] {
        text = mask_after(&text, key, |b| is_lower_hex(b) || b == b'-', "<HASH>");
    }
    text
}

struct Outcome {
    is_error: bool,
    normal: String,
    text: String,
    moved_refs: Vec<String>,
}

/// Make one call and hold it to everything that does not need its
/// pair. `named` collects the requests made to this fixture so far, for
/// the files of the view: a file may hold a canary only if a request
/// named it.
fn observe(
    fx: &Fixture,
    session: &mut Session,
    probe: &Probe,
    subject: &Subject,
    ctx: &Ctx,
    named: &mut String,
    violations: &mut Vec<String>,
) -> Outcome {
    let args = (probe.args)(subject, ctx);
    let request = args.to_string().to_lowercase();
    named.push_str(&request);
    let what = format!("{} [{} {args}]", probe.name, probe.tool);

    let before = state(fx);
    let reply = session.call(probe.tool, &args);
    let after = state(fx);

    let leaked = canaries_in(&reply.text, &request);
    if !leaked.is_empty() {
        violations.push(format!(
            "CANARY {what}: the reply holds {leaked:?}:\n{}",
            reply.text
        ));
    }
    for (path, bytes) in &after.view {
        let leaked = canaries_in(&String::from_utf8_lossy(bytes), named);
        if !leaked.is_empty() {
            violations.push(format!(
                "CANARY {what}: the view's file {path} holds {leaked:?}"
            ));
        }
    }

    let moved_refs = differing(&before.refs, &after.refs);
    for name in &moved_refs {
        if !probe.expect.refs_that_may_move().contains(&name.as_str()) {
            violations.push(format!(
                "WROTE {what}: the ref {name} moved ({:?} to {:?}); reply:\n{}",
                before.refs.get(name),
                after.refs.get(name),
                reply.text
            ));
        }
    }
    let changed_view = differing(&before.view, &after.view);
    for path in &changed_view {
        let allowed = probe
            .expect
            .view_files_that_may_change()
            .iter()
            .any(|prefix| path.starts_with(prefix));
        if !allowed {
            violations.push(format!("WROTE {what}: the view's file {path} changed"));
        }
    }
    for path in differing(&before.store, &after.store) {
        violations.push(format!("WROTE {what}: the store's file {path} changed"));
    }

    let moved = |name: &str| moved_refs.iter().any(|moved| moved == name);
    match probe.expect {
        Expect::Reads | Expect::MayWrite => {}
        Expect::Refuses => {
            if !reply.is_error {
                violations.push(format!("ANSWERED {what}: must refuse:\n{}", reply.text));
            }
        }
        Expect::Writes | Expect::Records | Expect::Sets => {
            if reply.is_error {
                violations.push(format!("REFUSED {what}: must answer:\n{}", reply.text));
            }
            let (wrote, place) = match probe.expect {
                Expect::Writes => (moved(WRITE_BRANCH), "the write mount's branch"),
                Expect::Sets => (moved(REGISTRY_REF), "the registry ref"),
                _ => (moved(WRITE_CHECKS_REF), "the write mount's checks ref"),
            };
            if !wrote {
                violations.push(format!(
                    "NO WRITE {what}: {place} did not change:\n{}",
                    reply.text
                ));
            }
        }
    }

    Outcome {
        is_error: reply.is_error,
        normal: normalised(&reply.text, fx),
        text: reply.text,
        moved_refs,
    }
}

/// The call for the unmounted mem must answer as the call for names
/// that exist nowhere.
fn hold_pair(probe: &Probe, there: &Outcome, nowhere: &Outcome, violations: &mut Vec<String>) {
    if there.normal != nowhere.normal
        || there.is_error != nowhere.is_error
        || there.moved_refs != nowhere.moved_refs
    {
        violations.push(format!(
            "DIFFERS {} [{}]: for the unmounted mem (refs moved: {:?}):\n{}\nfor names that exist \
             nowhere (refs moved: {:?}):\n{}",
            probe.name, probe.tool, there.moved_refs, there.text, nowhere.moved_refs, nowhere.text
        ));
    }
}

/// Every call that is expected to change nothing, in one fixture and
/// one session; and the check of the table against `tools/list`.
fn shared_fixture(table: &[Probe]) -> Vec<String> {
    let mut violations = Vec::new();
    let fx = build();
    let mut session = Session::start(&fx);

    let listed = session.tools();
    let called: BTreeSet<String> = table.iter().map(|probe| probe.tool.to_string()).collect();
    for tool in listed.difference(&called) {
        violations.push(format!(
            "UNCOVERED the server lists the tool {tool} and the table calls it nowhere: add its \
             calls to `probes()`"
        ));
    }
    for tool in called.difference(&listed) {
        violations.push(format!(
            "UNLISTED the table calls {tool}, which the server does not list"
        ));
    }

    let ctx = Ctx::of(&fx, &mut session);
    let there = Subject::there(&fx);
    let nowhere = Subject::nowhere();
    let mut named = String::new();
    for probe in table.iter().filter(|p| p.expect.shares_a_fixture()) {
        let first = observe(
            &fx,
            &mut session,
            probe,
            &there,
            &ctx,
            &mut named,
            &mut violations,
        );
        if probe.paired {
            let second = observe(
                &fx,
                &mut session,
                probe,
                &nowhere,
                &ctx,
                &mut named,
                &mut violations,
            );
            hold_pair(probe, &first, &second, &mut violations);
        }
    }

    // The stub is the one thing of the unmounted mem the view serves:
    // it is there, marked as a stub, and (by the canaries above) empty.
    let stub = session.call("memstead_entity", &full_read(STUB));
    if stub.is_error || !stub.text.contains("_stub_kind") {
        violations.push(format!(
            "STUB the id a mounted mem links to must read as a stub:\n{}",
            stub.text
        ));
    }
    violations
}

/// One call that may write, in a fixture of its own.
fn fresh_fixture(probe: &Probe, there: bool) -> (Outcome, Vec<String>) {
    let mut violations = Vec::new();
    let fx = build();
    let mut session = Session::start(&fx);
    let ctx = Ctx::of(&fx, &mut session);
    let subject = if there {
        Subject::there(&fx)
    } else {
        Subject::nowhere()
    };
    let mut named = String::new();
    let outcome = observe(
        &fx,
        &mut session,
        probe,
        &subject,
        &ctx,
        &mut named,
        &mut violations,
    );
    (outcome, violations)
}

#[test]
fn a_view_shows_and_changes_nothing_but_what_it_mounts() {
    let table = probes();
    // (index into the table, whether the call names the unmounted mem)
    let mut jobs: Vec<(usize, bool)> = Vec::new();
    for (index, probe) in table.iter().enumerate() {
        if !probe.expect.shares_a_fixture() {
            jobs.push((index, true));
            if probe.paired {
                jobs.push((index, false));
            }
        }
    }

    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<((usize, bool), Outcome)>> = Mutex::new(Vec::new());
    let found: Mutex<Vec<String>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let violations = shared_fixture(&table);
            found.lock().unwrap().extend(violations);
        });
        for _ in 0..4 {
            scope.spawn(|| {
                while let Some(&(index, there)) = jobs.get(next.fetch_add(1, Ordering::SeqCst)) {
                    let (outcome, violations) = fresh_fixture(&table[index], there);
                    found.lock().unwrap().extend(violations);
                    done.lock().unwrap().push(((index, there), outcome));
                }
            });
        }
    });

    let done: BTreeMap<(usize, bool), Outcome> = done.into_inner().unwrap().into_iter().collect();
    let mut found = found.into_inner().unwrap();
    for (index, probe) in table.iter().enumerate() {
        if let (Some(there), Some(nowhere)) = (done.get(&(index, true)), done.get(&(index, false)))
        {
            hold_pair(probe, there, nowhere, &mut found);
        }
    }
    found.sort();
    assert!(
        found.is_empty(),
        "{} violation(s) of the view contract:\n\n{}",
        found.len(),
        found.join("\n\n")
    );
}

#[test]
fn the_normalisation_keeps_what_is_not_named() {
    let text = "at 2026-11-23T19:00:55Z and 2026-11-23T19:00:55.123Z, commit \
                1f2eb8b4fd0e24322c94afac28d1254d59318c4a, not 1f2eb8b4fd0e24322c94afac28d1254d59318c4a0 \
                nor 8a8f94e703920121, memstead-quote-54c592ac3a26758b.";
    assert_eq!(
        mask_runs(text),
        "at <TS> and <TS>, commit <SHA>, not 1f2eb8b4fd0e24322c94afac28d1254d59318c4a0 nor \
         8a8f94e703920121, <Q>."
    );
    assert_eq!(
        mask_after(
            "_tokens: 67, \"_total_tokens\": 216, _total: 6",
            "_tokens",
            |b| b.is_ascii_digit(),
            "<N>"
        ),
        "_tokens: <N>, \"_total_tokens\": <N>, _total: 6"
    );
    assert_eq!(
        mask_after(
            "_hash: 8a8f94e703920121 \"_hash\": \"8a8f94e703920121\" hashes",
            "hash",
            |b| is_lower_hex(b) || b == b'-',
            "<HASH>"
        ),
        "_hash: <HASH> \"_hash\": \"<HASH>\" hashes"
    );
    assert_eq!(
        canaries_in("annex--one and Annex--Two, crateshape", "crateshape"),
        vec!["annex--two"]
    );
}
