//! Which schemas a view resolves, and which pin it may write.
//!
//! A store is a workspace with a mem-repo of its own; a view is a
//! workspace of engine state only that mounts mems out of the store's
//! git directory. Two rules are held here, on the CLI and through the
//! `memstead-mcp` binary:
//!
//! - From the store's schema registry a view takes the schemas that the
//!   mems it mounts pin, and nothing else. A schema that only a mem the
//!   view does not mount pins answers exactly as a name that exists
//!   nowhere. Built-in schemas, a schema installed in the view's own
//!   folder, and whatever a mounted mem's configuration pins keep
//!   resolving; the store itself resolves all it holds.
//! - A pin is written where the mem lives. A view may switch a mounted
//!   mem to a built-in schema or to one the store's registry holds; a
//!   schema only the view holds is refused before anything is written.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command as StdCommand, Output, Stdio};

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

const AUTHOR: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "test"),
    ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
    ("GIT_COMMITTER_NAME", "test"),
    ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
];

fn memstead(ws: &Path) -> Command {
    let mut cmd = Command::cargo_bin("memstead").expect("memstead binary must be built by cargo");
    cmd.current_dir(ws)
        .envs(AUTHOR)
        .env_remove("MEMSTEAD_WORKSPACE")
        .env_remove("MEMSTEAD_IDENTITY");
    cmd
}

fn run(ws: &Path, args: &[&str]) -> Output {
    memstead(ws).args(args).arg("--quiet").output().unwrap()
}

fn printed(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

fn ok(ws: &Path, args: &[&str]) -> String {
    let out = run(ws, args);
    assert!(
        out.status.success(),
        "`memstead {}` failed:\n{}",
        args.join(" "),
        printed(&out)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run with `--json` and parse stdout, whether the command answered or
/// refused; the flag says which.
fn json_run(ws: &Path, args: &[&str]) -> (bool, Value) {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let out = run(ws, &full);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "`memstead --json {}` printed no JSON ({e}):\n{}",
            args.join(" "),
            printed(&out)
        )
    });
    (out.status.success(), value)
}

fn json_ok(ws: &Path, args: &[&str]) -> Value {
    let (succeeded, value) = json_run(ws, args);
    assert!(
        succeeded,
        "`memstead --json {}` refused: {value}",
        args.join(" ")
    );
    value
}

fn json_refused(ws: &Path, args: &[&str], code: &str) -> Value {
    let (succeeded, value) = json_run(ws, args);
    assert!(
        !succeeded,
        "`memstead --json {}` answered: {value}",
        args.join(" ")
    );
    assert_eq!(value["code"], code, "{value}");
    value
}

fn git(gitdir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
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
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// One session with the real `memstead-mcp` binary over stdio.
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(ws: &Path) -> Self {
        let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("memstead-mcp"))
            .arg("--workspace")
            .arg(ws)
            .current_dir(ws)
            .envs(AUTHOR)
            .env_remove("MEMSTEAD_WORKSPACE")
            .env_remove("MEMSTEAD_IDENTITY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("memstead-mcp spawns");
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
                "clientInfo": { "name": "view-schemas-test", "version": "0" }
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

    /// The `result` of one `tools/call`.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        assert!(
            reply.get("result").is_some(),
            "{tool} answered no result: {reply}"
        );
        reply["result"].clone()
    }

    /// A call that must answer; its structured content.
    fn answered(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self.call(tool, arguments.clone());
        assert!(
            !result["isError"].as_bool().unwrap_or(false),
            "{tool} {arguments} refused: {result}"
        );
        result["structuredContent"].clone()
    }

    /// A call that must refuse with `code`; its structured content.
    fn refused(&mut self, tool: &str, arguments: Value, code: &str) -> Value {
        let result = self.call(tool, arguments.clone());
        assert!(
            result["isError"].as_bool().unwrap_or(false),
            "{tool} {arguments} answered: {result}"
        );
        assert_eq!(result["structuredContent"]["code"], code, "{result}");
        result["structuredContent"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A scaffolded schema package `<pkg>/<name>` (one type `note` with the
/// required section `summary`).
fn scaffold(pkg: &Path, name: &str) -> String {
    fs::create_dir_all(pkg).unwrap();
    ok(pkg, &["schema", "new", name]);
    pkg.join(name).to_str().unwrap().to_string()
}

/// A scaffolded package that declares the type `part` beside `note`.
fn scaffold_with_part(pkg: &Path, name: &str) -> String {
    let dir = PathBuf::from(scaffold(pkg, name));
    let manifest = fs::read_to_string(dir.join("schema.yaml")).unwrap();
    assert!(manifest.contains("types:\n  - note\n"), "{manifest}");
    fs::write(
        dir.join("schema.yaml"),
        manifest.replace("types:\n  - note\n", "types:\n  - note\n  - part\n"),
    )
    .unwrap();
    let note = fs::read_to_string(dir.join("types/note.yaml")).unwrap();
    assert!(note.contains("\nname: note\n"), "{note}");
    fs::write(
        dir.join("types/part.yaml"),
        note.replace("\nname: note\n", "\nname: part\n"),
    )
    .unwrap();
    dir.to_str().unwrap().to_string()
}

fn init_store(store: &Path) -> PathBuf {
    fs::create_dir_all(store).unwrap();
    ok(store, &["mem-repo", "init", "."]);
    ok(store, &["workspace", "allow-create", "--schema", "*", "*"]);
    store.join("mem-repo/.git").canonicalize().unwrap()
}

/// Mount `mem` of the store's git directory into `view`, as the first
/// mount of a new workspace when `view` holds none yet.
fn mount(view: &Path, gitdir: &Path, mem: &str, capability: &str) {
    let fresh = !view.join(".memstead").exists();
    fs::create_dir_all(view).unwrap();
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
    if fresh {
        args.push("--new-workspace");
    }
    ok(view, &args);
    assert!(!view.join("mem-repo").exists(), "a view has no mem-repo");
}

fn create_note(ws: &Path, mem: &str, title: &str, entity_type: &str) -> Output {
    run(
        ws,
        &[
            "create",
            "--mem",
            mem,
            "--title",
            title,
            "--type",
            entity_type,
            "--section",
            "summary=Hello.",
        ],
    )
}

/// The mems a workspace serves, as `name -> schema pin`, after
/// asserting that none is quarantined.
fn serving(ws: &Path) -> Vec<(String, String)> {
    let listed = json_ok(ws, &["mem", "list"]);
    assert_eq!(listed["quarantined"], json!([]), "{listed}");
    listed["mems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["name"].as_str().unwrap().to_string(),
                m["schema_ref"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pin_of(ws: &Path, mem: &str) -> String {
    serving(ws)
        .into_iter()
        .find(|(name, _)| name == mem)
        .unwrap_or_else(|| panic!("{mem} is not served in {}", ws.display()))
        .1
}

struct Fixture {
    _root: TempDir,
    store: PathBuf,
    view: PathBuf,
    pkg: PathBuf,
    gitdir: PathBuf,
}

/// A store with the schemas `widgets` (pinned by the mem `kept`),
/// `gadgets` (pinned by the mem `other`) and `sprockets` (installed,
/// pinned by no mem), and a view that mounts `kept` alone.
fn two_schemas() -> Fixture {
    let root = TempDir::new().unwrap();
    let store = root.path().join("store");
    let view = root.path().join("view");
    let pkg = root.path().join("pkg");
    let gitdir = init_store(&store);
    for name in ["widgets", "gadgets", "sprockets"] {
        let package = scaffold(&pkg, name);
        ok(&store, &["schema", "install", &package]);
    }
    ok(
        &store,
        &["mem", "init", "kept", "--schema", "widgets@0.1.0"],
    );
    ok(
        &store,
        &["mem", "init", "other", "--schema", "gadgets@0.1.0"],
    );
    assert!(create_note(&store, "kept", "Held", "note").status.success());
    mount(&view, &gitdir, "kept", "write");
    Fixture {
        _root: root,
        store,
        view,
        pkg,
        gitdir,
    }
}

/// `value` as text with both names of a pair replaced by one token.
fn one_token(value: &impl ToString, pair: [&str; 2]) -> String {
    value
        .to_string()
        .replace(pair[0], "<NAME>")
        .replace(pair[1], "<NAME>")
}

/// The schema `gadgets` is installed in the store and pinned there by a
/// mem the view does not mount; `gizmos` exists nowhere.
const HIDDEN: &str = "gadgets";
const NOWHERE: &str = "gizmos";

#[test]
fn a_schema_no_mounted_mem_pins_answers_as_one_that_exists_nowhere() {
    let fx = two_schemas();
    let refs_before = git(&fx.gitdir, &["for-each-ref"]);
    let hidden_pin = format!("{HIDDEN}@0.1.0");
    let nowhere_pin = format!("{NOWHERE}@0.1.0");

    // The CLI, in both output forms. A refusal for the hidden schema is
    // the refusal for the absent one, and never names the hidden schema
    // where the request did not.
    for args in [
        ["mem", "set-schema", "kept"].as_slice(),
        ["schema"].as_slice(),
    ] {
        let with = |pin: &str| {
            let mut full = args.to_vec();
            full.push(pin);
            let plain = run(&fx.view, &full);
            assert!(!plain.status.success(), "{}", printed(&plain));
            let structured = json_refused(&fx.view, &full, "SCHEMA_NOT_FOUND");
            (printed(&plain), structured)
        };
        let (hidden_plain, hidden_json) = with(&hidden_pin);
        let (nowhere_plain, nowhere_json) = with(&nowhere_pin);
        assert!(
            hidden_json["message"].as_str().unwrap().contains(HIDDEN),
            "{hidden_json}"
        );
        assert_eq!(
            one_token(&hidden_plain, [HIDDEN, NOWHERE]),
            one_token(&nowhere_plain, [HIDDEN, NOWHERE]),
            "`memstead {}`",
            args.join(" ")
        );
        assert_eq!(
            one_token(&hidden_json, [HIDDEN, NOWHERE]),
            one_token(&nowhere_json, [HIDDEN, NOWHERE]),
            "`memstead --json {}`",
            args.join(" ")
        );
        assert!(!nowhere_plain.contains(HIDDEN), "{nowhere_plain}");
        assert!(!nowhere_json.to_string().contains(HIDDEN), "{nowhere_json}");
    }

    // The same through the MCP server: a schema read by pin and by bare
    // name, a conformance read against a target schema, a pin switch.
    let mut mcp = Mcp::start(&fx.view);
    type Arguments = fn(&str) -> Value;
    let calls: [(&str, Arguments); 4] = [
        (
            "memstead_schema",
            |name| json!({ "name": format!("{name}@0.1.0") }),
        ),
        ("memstead_schema", |name| json!({ "name": name })),
        (
            "memstead_health",
            |name| json!({ "include": ["conformance"], "target_schema": format!("{name}@0.1.0") }),
        ),
        (
            "memstead_mem_set_schema",
            |name| json!({ "mem": "kept", "schema": format!("{name}@0.1.0") }),
        ),
    ];
    for (tool, arguments) in &calls {
        let hidden = mcp.call(tool, arguments(HIDDEN));
        let nowhere = mcp.call(tool, arguments(NOWHERE));
        assert_eq!(
            hidden["structuredContent"]["code"], "SCHEMA_NOT_FOUND",
            "{tool}: {hidden}"
        );
        assert_eq!(hidden["isError"], true, "{tool}: {hidden}");
        assert!(
            hidden["structuredContent"]["message"]
                .as_str()
                .unwrap()
                .contains(HIDDEN),
            "{tool}: {hidden}"
        );
        assert!(
            hidden["structuredContent"]["details"].is_object(),
            "{tool}: {hidden}"
        );
        // The whole reply: text, message and details.
        assert_eq!(
            one_token(&hidden, [HIDDEN, NOWHERE]),
            one_token(&nowhere, [HIDDEN, NOWHERE]),
            "{tool} {}",
            arguments(HIDDEN)
        );
        let nowhere = nowhere.to_string();
        for held_back in [HIDDEN, "sprockets"] {
            assert!(
                !nowhere.contains(held_back),
                "{tool} names {held_back}: {nowhere}"
            );
        }
    }

    // The list of what is available names the schema the mounted mem
    // pins, and neither of the two that no mounted mem pins.
    let refusal = mcp.refused(
        "memstead_schema",
        json!({ "name": nowhere_pin }),
        "SCHEMA_NOT_FOUND",
    );
    let available: Vec<&str> = refusal["details"]["available"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    assert!(available.contains(&"widgets"), "{available:?}");
    assert!(available.contains(&"default"), "{available:?}");
    assert!(!available.contains(&"gadgets"), "{available:?}");
    assert!(!available.contains(&"sprockets"), "{available:?}");

    // The schema the mounted mem pins resolves on every one of these
    // ways.
    let schema = mcp.answered("memstead_schema", json!({ "name": "widgets@0.1.0" }));
    assert_eq!(schema["ref"], "widgets@0.1.0", "{schema}");
    let schema = mcp.answered("memstead_schema", json!({ "name": "widgets" }));
    assert_eq!(schema["ref"], "widgets@0.1.0", "{schema}");
    mcp.answered(
        "memstead_health",
        json!({ "include": ["conformance"], "target_schema": "widgets@0.1.0" }),
    );
    let switched = mcp.answered(
        "memstead_mem_set_schema",
        json!({ "mem": "kept", "schema": "widgets@0.1.0" }),
    );
    assert_eq!(switched["schema_pin"], "widgets@0.1.0", "{switched}");
    drop(mcp);
    let readme = json_ok(&fx.view, &["schema", "widgets@0.1.0"]);
    assert_eq!(readme["schema"], "widgets@0.1.0", "{readme}");
    assert!(
        create_note(&fx.view, "kept", "Second", "note")
            .status
            .success()
    );

    // No refusal wrote anything where the mems live but the one entity.
    let refs_after = git(&fx.gitdir, &["for-each-ref"]);
    let moved: Vec<&str> = refs_before
        .lines()
        .zip(refs_after.lines())
        .filter(|(before, after)| before != after)
        .map(|(before, _)| before.split_whitespace().last().unwrap())
        .collect();
    assert_eq!(
        refs_before.lines().count(),
        refs_after.lines().count(),
        "{refs_after}"
    );
    assert!(
        moved
            .iter()
            .all(|name| *name == "refs/heads/kept" || *name == "refs/heads/__MEMSTEAD"),
        "{moved:?}"
    );
    assert_eq!(pin_of(&fx.store, "kept"), "widgets@0.1.0");
    assert_eq!(pin_of(&fx.store, "other"), "gadgets@0.1.0");
}

#[test]
fn built_in_schemas_and_a_schema_installed_in_the_view_resolve_there() {
    let fx = two_schemas();
    let builtins = memstead_schema::builtins::load_builtin_schemas().unwrap();
    assert!(!builtins.is_empty());

    let mut mcp = Mcp::start(&fx.view);
    for schema in &builtins {
        let (name, version) = schema.id();
        let pin = format!("{name}@{version}");
        let read = mcp.answered("memstead_schema", json!({ "name": pin }));
        assert_eq!(read["ref"], pin.as_str(), "{read}");
        mcp.answered("memstead_schema", json!({ "name": name }));
    }
    mcp.refused(
        "memstead_schema",
        json!({ "name": "trinkets@0.1.0" }),
        "SCHEMA_NOT_FOUND",
    );
    drop(mcp);

    // Installed in the view, the package lands in the view's own folder
    // and nowhere in the store.
    let refs_before = git(&fx.gitdir, &["for-each-ref"]);
    let package = scaffold(&fx.pkg, "trinkets");
    ok(&fx.view, &["schema", "install", &package]);
    assert!(
        fx.view
            .join(".memstead/schemas/trinkets@0.1.0/schema.yaml")
            .is_file()
    );
    assert_eq!(git(&fx.gitdir, &["for-each-ref"]), refs_before);

    let readme = json_ok(&fx.view, &["schema", "trinkets@0.1.0"]);
    assert_eq!(readme["schema"], "trinkets@0.1.0", "{readme}");
    let mut mcp = Mcp::start(&fx.view);
    let read = mcp.answered("memstead_schema", json!({ "name": "trinkets@0.1.0" }));
    assert_eq!(read["ref"], "trinkets@0.1.0", "{read}");
    mcp.answered(
        "memstead_health",
        json!({ "include": ["conformance"], "target_schema": "trinkets@0.1.0" }),
    );
    // The store does not hold what the view installed for itself.
    drop(mcp);
    json_refused(&fx.store, &["schema", "trinkets@0.1.0"], "SCHEMA_NOT_FOUND");
}

/// The roster row's `schema` is an assertion a row need not carry: the
/// pin in the mem's configuration, read where the mem lives, counts
/// for what the view takes from the store's registry.
#[test]
fn a_mount_whose_roster_row_carries_no_pin_serves_under_its_configuration() {
    let fx = two_schemas();
    let roster_path = fx.view.join(".memstead/state/mounts.json");
    let mut roster: Value =
        serde_json::from_str(&fs::read_to_string(&roster_path).unwrap()).unwrap();
    let row = roster["mounts"][0].as_object_mut().unwrap();
    assert_eq!(row.remove("schema"), Some(json!("widgets@0.1.0")));
    fs::write(&roster_path, serde_json::to_string_pretty(&roster).unwrap()).unwrap();

    assert_eq!(pin_of(&fx.view, "kept"), "widgets@0.1.0");
    let created = create_note(&fx.view, "kept", "From The View", "note");
    assert!(created.status.success(), "{}", printed(&created));
    let mut mcp = Mcp::start(&fx.view);
    let read = mcp.answered("memstead_schema", json!({ "name": "widgets@0.1.0" }));
    assert_eq!(read["ref"], "widgets@0.1.0", "{read}");
    let made = mcp.answered(
        "memstead_create",
        json!({
            "mem": "kept",
            "title": "Over MCP",
            "entity_type": "note",
            "sections": { "summary": "Hello." }
        }),
    );
    assert_eq!(made["id"], "kept--over-mcp", "{made}");
    // Still nothing of the schema no mounted mem pins.
    mcp.refused(
        "memstead_schema",
        json!({ "name": "gadgets@0.1.0" }),
        "SCHEMA_NOT_FOUND",
    );
}

/// The store moves the mounted mem's pin to another schema it holds.
/// An engine that boots in the view afterwards serves the mem under the
/// new pin, although the view's roster row still names the old one.
#[test]
fn a_pin_the_store_moved_is_followed_by_the_next_boot_of_the_view() {
    let fx = two_schemas();
    let package = scaffold_with_part(&fx.pkg, "cogs");
    ok(&fx.store, &["schema", "install", &package]);

    // Before the move the view knows neither the type nor the schema.
    let early = create_note(&fx.view, "kept", "Too Early", "part");
    assert!(
        printed(&early).contains("UNKNOWN_ENTITY_TYPE"),
        "{}",
        printed(&early)
    );
    json_refused(&fx.view, &["schema", "cogs@0.1.0"], "SCHEMA_NOT_FOUND");

    let moved = json_ok(&fx.store, &["mem", "set-schema", "kept", "cogs@0.1.0"]);
    assert_eq!(moved["outcome"], "switched", "{moved}");

    // A write of the new schema's own type lands from the view.
    let created = create_note(&fx.view, "kept", "A Part", "part");
    assert!(created.status.success(), "{}", printed(&created));
    assert!(
        printed(&created).contains("kept--a-part"),
        "{}",
        printed(&created)
    );
    assert_eq!(pin_of(&fx.view, "kept"), "cogs@0.1.0");
    let roster = fs::read_to_string(fx.view.join(".memstead/state/mounts.json")).unwrap();
    assert!(
        roster.contains("\"schema\": \"widgets@0.1.0\""),
        "the roster row keeps the pin it was mounted under: {roster}"
    );
    assert!(!roster.contains("cogs"), "{roster}");

    // And over MCP, in a server that boots after the move.
    let mut mcp = Mcp::start(&fx.view);
    let made = mcp.answered(
        "memstead_create",
        json!({
            "mem": "kept",
            "title": "Another Part",
            "entity_type": "part",
            "sections": { "summary": "Hello." }
        }),
    );
    assert_eq!(made["id"], "kept--another-part", "{made}");
    let seen = ok(&fx.store, &["entity", "kept--another-part"]);
    assert!(seen.contains("Another Part"), "{seen}");
}

#[test]
fn the_store_resolves_every_schema_installed_in_it() {
    let fx = two_schemas();
    let mut mcp = Mcp::start(&fx.store);
    for name in ["widgets", "gadgets", "sprockets"] {
        let pin = format!("{name}@0.1.0");
        let read = mcp.answered("memstead_schema", json!({ "name": pin }));
        assert_eq!(read["ref"], pin.as_str(), "{read}");
        mcp.answered(
            "memstead_health",
            json!({ "include": ["conformance"], "target_schema": pin }),
        );
        let readme = json_ok(&fx.store, &["schema", &pin]);
        assert_eq!(readme["schema"], pin.as_str(), "{readme}");
    }
    drop(mcp);

    // An empty mem of the store switches to a schema that is installed
    // there and that no mem pinned so far.
    let switched = json_ok(
        &fx.store,
        &["mem", "set-schema", "other", "sprockets@0.1.0"],
    );
    assert_eq!(switched["outcome"], "switched", "{switched}");
    assert_eq!(pin_of(&fx.store, "other"), "sprockets@0.1.0");
}

/// The warm path: a running server in the view refreshes after the
/// store installed a schema. Nothing the view mounts pins it, so the
/// refresh adds no schema; once a mounted mem pins it, the refresh
/// does.
#[test]
fn a_warm_refresh_adds_no_schema_that_no_mounted_mem_pins() {
    let fx = two_schemas();
    let mut mcp = Mcp::start(&fx.view);
    let added = |mcp: &mut Mcp| {
        let reply = mcp.answered("memstead_reload", json!({ "full": true }));
        assert_eq!(reply["refresh"]["failures"], json!([]), "{reply}");
        reply["refresh"]["schemas_added"].clone()
    };
    assert_eq!(added(&mut mcp), json!([]));

    let package = scaffold(&fx.pkg, "levers");
    ok(&fx.store, &["schema", "install", &package]);
    assert_eq!(
        added(&mut mcp),
        json!([]),
        "a schema no mounted mem pins is not the view's to take"
    );
    let hidden = mcp.call("memstead_schema", json!({ "name": "levers@0.1.0" }));
    let nowhere = mcp.call("memstead_schema", json!({ "name": "gizmos@0.1.0" }));
    assert_eq!(hidden["structuredContent"]["code"], "SCHEMA_NOT_FOUND");
    assert_eq!(
        one_token(&hidden, ["levers", NOWHERE]),
        one_token(&nowhere, ["levers", NOWHERE])
    );
    assert!(!nowhere.to_string().contains("levers"), "{nowhere}");

    // The store pins the mounted mem to it: now it is the view's.
    let moved = json_ok(&fx.store, &["mem", "set-schema", "kept", "levers@0.1.0"]);
    assert_eq!(moved["outcome"], "switched", "{moved}");
    assert_eq!(added(&mut mcp), json!(["levers@0.1.0"]));
    let read = mcp.answered("memstead_schema", json!({ "name": "levers@0.1.0" }));
    assert_eq!(read["ref"], "levers@0.1.0", "{read}");
}

// ---------------------------------------------------------------------
// The pin switch
// ---------------------------------------------------------------------

/// What a refused pin switch must leave as it was: every ref of the
/// store's git directory, the mem's configuration on the registry ref,
/// and the store serving the mem.
fn where_the_mem_lives(store: &Path, gitdir: &Path, mem: &str) -> (String, String, String) {
    (
        git(gitdir, &["for-each-ref"]),
        git(
            gitdir,
            &["show", &format!("__MEMSTEAD:mems/{mem}/config.json")],
        ),
        pin_of(store, mem),
    )
}

/// The roster with its first row twice: the boot fails on the duplicate
/// name, so `mem set-schema` takes the switch below boot.
fn roster_that_fails_to_boot(view: &Path) -> Vec<u8> {
    let path = view.join(".memstead/state/mounts.json");
    let original = fs::read(&path).unwrap();
    let mut roster: Value = serde_json::from_slice(&original).unwrap();
    let first = roster["mounts"][0].clone();
    roster["mounts"].as_array_mut().unwrap().push(first);
    fs::write(&path, serde_json::to_string_pretty(&roster).unwrap()).unwrap();
    let out = run(view, &["mem", "list"]);
    assert!(
        !out.status.success() && printed(&out).contains("duplicate mem"),
        "the doubled roster must fail the boot: {}",
        printed(&out)
    );
    original
}

/// A store with the mem `kept` on a built-in schema, a view that mounts
/// it with `capability`, and the schema `trinkets` installed in the
/// view's own folder alone.
fn view_with_a_schema_of_its_own(capability: &str) -> Fixture {
    let root = TempDir::new().unwrap();
    let store = root.path().join("store");
    let view = root.path().join("view");
    let pkg = root.path().join("pkg");
    let gitdir = init_store(&store);
    ok(
        &store,
        &["mem", "init", "kept", "--schema", "default@1.3.0"],
    );
    mount(&view, &gitdir, "kept", capability);
    let package = scaffold(&pkg, "trinkets");
    ok(&view, &["schema", "install", &package]);
    json_ok(&view, &["schema", "trinkets@0.1.0"]);
    Fixture {
        _root: root,
        store,
        view,
        pkg,
        gitdir,
    }
}

fn assert_refused_where_the_mem_lives(refusal: &Value, gitdir: &Path) {
    assert_eq!(refusal["code"], "SCHEMA_NOT_FOUND", "{refusal}");
    assert!(
        refusal["message"]
            .as_str()
            .unwrap()
            .contains("where the mem lives"),
        "{refusal}"
    );
    assert_eq!(
        refusal["details"],
        json!({
            "mem": "kept",
            "pin": "trinkets@0.1.0",
            "sources": [],
            "mem_git_directory": gitdir.to_str().unwrap(),
        }),
        "{refusal}"
    );
}

/// Switch the view's writable mount to the schema only the view holds,
/// by `switch`, and hold the refusal: its code, message and details,
/// and that neither the store nor the view's roster changed.
fn refused_and_nothing_written(fx: &Fixture, switch: impl FnOnce(&Fixture) -> Value) {
    let before = where_the_mem_lives(&fx.store, &fx.gitdir, "kept");
    assert_eq!(before.2, "default@1.3.0");
    let roster_path = fx.view.join(".memstead/state/mounts.json");
    let roster_before = fs::read(&roster_path).unwrap();

    let refusal = switch(fx);

    assert_refused_where_the_mem_lives(&refusal, &fx.gitdir);
    assert_eq!(fs::read(&roster_path).unwrap(), roster_before);
    assert_eq!(where_the_mem_lives(&fx.store, &fx.gitdir, "kept"), before);
}

#[test]
fn a_pin_only_the_view_resolves_is_refused_over_mcp() {
    let fx = view_with_a_schema_of_its_own("write");
    refused_and_nothing_written(&fx, |fx| {
        Mcp::start(&fx.view).refused(
            "memstead_mem_set_schema",
            json!({ "mem": "kept", "schema": "trinkets@0.1.0" }),
            "SCHEMA_NOT_FOUND",
        )
    });
    assert_eq!(pin_of(&fx.view, "kept"), "default@1.3.0");
}

#[test]
fn a_pin_only_the_view_resolves_is_refused_on_the_cli() {
    let fx = view_with_a_schema_of_its_own("write");
    refused_and_nothing_written(&fx, |fx| {
        json_refused(
            &fx.view,
            &["mem", "set-schema", "kept", "trinkets@0.1.0"],
            "SCHEMA_NOT_FOUND",
        )
    });
    assert_eq!(pin_of(&fx.view, "kept"), "default@1.3.0");
}

#[test]
fn a_pin_only_the_view_resolves_is_refused_below_boot() {
    let fx = view_with_a_schema_of_its_own("write");
    let original = roster_that_fails_to_boot(&fx.view);
    refused_and_nothing_written(&fx, |fx| {
        json_refused(
            &fx.view,
            &["mem", "set-schema", "kept", "trinkets@0.1.0"],
            "SCHEMA_NOT_FOUND",
        )
    });

    // The same doubled roster does take the switch below boot: a
    // built-in target lands there, which also shows that the refusal
    // above came from that path.
    let switched = json_ok(&fx.view, &["mem", "set-schema", "kept", "default@1.2.0"]);
    assert_eq!(switched["below_boot"], true, "{switched}");
    assert_eq!(pin_of(&fx.store, "kept"), "default@1.2.0");
    fs::write(fx.view.join(".memstead/state/mounts.json"), original).unwrap();
    assert_eq!(pin_of(&fx.view, "kept"), "default@1.2.0");
}

/// The third place a pin is switched: the repair of a quarantined mem
/// in a booted engine. The mem's configuration pins a schema nobody
/// holds, so the view boots with the mem quarantined; a repair onto the
/// view's own schema is refused as any other switch, a repair onto a
/// built-in schema brings the mem back.
#[test]
fn a_quarantined_mount_is_not_repaired_onto_a_pin_only_the_view_resolves() {
    let root = TempDir::new().unwrap();
    let store = root.path().join("store");
    let view = root.path().join("view");
    fs::create_dir_all(&store).unwrap();
    memstead_git_branch::test_support::init_real_mem_repo(&store, &[("kept", "absent@0.1.0")]);
    let gitdir = store.join("mem-repo/.git").canonicalize().unwrap();
    fs::create_dir_all(view.join(".memstead/state")).unwrap();
    fs::write(
        view.join(".memstead/workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    fs::write(
        view.join(".memstead/state/mounts.json"),
        json!({
            "format": "memstead-mounts-3",
            "mounts": [{
                "mem": "kept",
                "schema": "absent@0.1.0",
                "storage": { "type": "git-branch", "gitdir": gitdir, "branch": "refs/heads/kept" },
                "capability": "write",
                "lifecycle": "eager",
                "cross_linkable": true
            }]
        })
        .to_string(),
    )
    .unwrap();
    let package = scaffold(&root.path().join("pkg"), "trinkets");
    ok(&view, &["schema", "install", &package]);
    let quarantined = |ws: &Path| {
        let listed = json_ok(ws, &["mem", "list"]);
        listed["quarantined"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q.to_string())
            .collect::<Vec<_>>()
    };
    let held = quarantined(&view);
    assert_eq!(held.len(), 1, "{held:?}");
    assert!(
        held[0].contains("kept") && held[0].contains("SCHEMA_NOT_FOUND"),
        "{held:?}"
    );

    let refs_before = git(&gitdir, &["for-each-ref"]);
    let refusal = json_refused(
        &view,
        &["mem", "set-schema", "kept", "trinkets@0.1.0"],
        "SCHEMA_NOT_FOUND",
    );
    assert_refused_where_the_mem_lives(&refusal, &gitdir);
    assert_eq!(git(&gitdir, &["for-each-ref"]), refs_before);
    assert_eq!(quarantined(&view), held);

    json_ok(&view, &["mem", "set-schema", "kept", "default@1.3.0"]);
    assert_eq!(pin_of(&view, "kept"), "default@1.3.0");
}

#[test]
fn a_view_still_switches_to_a_pin_the_store_holds_and_sets_what_it_may() {
    // A built-in target from a write mount.
    let fx = view_with_a_schema_of_its_own("write");
    let switched = json_ok(&fx.view, &["mem", "set-schema", "kept", "default@1.2.0"]);
    assert_eq!(switched["outcome"], "switched", "{switched}");
    assert_eq!(pin_of(&fx.store, "kept"), "default@1.2.0");
    assert_eq!(pin_of(&fx.view, "kept"), "default@1.2.0");

    // A custom target that the store's registry holds and the view
    // resolves, because another mem it mounts from there pins it.
    let package = scaffold(&fx.pkg, "widgets");
    ok(&fx.store, &["schema", "install", &package]);
    ok(
        &fx.store,
        &["mem", "init", "other", "--schema", "widgets@0.1.0"],
    );
    json_refused(&fx.view, &["schema", "widgets@0.1.0"], "SCHEMA_NOT_FOUND");
    mount(&fx.view, &fx.gitdir, "other", "read-only");
    {
        let mut mcp = Mcp::start(&fx.view);
        let switched = mcp.answered(
            "memstead_mem_set_schema",
            json!({ "mem": "kept", "schema": "widgets@0.1.0" }),
        );
        assert_eq!(switched["outcome"], "switched", "{switched}");
        assert_eq!(switched["schema_pin"], "widgets@0.1.0", "{switched}");
    }
    assert_eq!(pin_of(&fx.store, "kept"), "widgets@0.1.0");
    assert_eq!(pin_of(&fx.view, "kept"), "widgets@0.1.0");
    assert!(
        create_note(&fx.view, "kept", "Held", "note")
            .status
            .success()
    );

    // The other two settings of a mem, from the view on its write mount.
    let registry_before = git(&fx.gitdir, &["rev-parse", "refs/heads/__MEMSTEAD"]);
    {
        let mut mcp = Mcp::start(&fx.view);
        let versioned = mcp.answered(
            "memstead_mem_set_version",
            json!({ "name": "kept", "version": "0.2.0" }),
        );
        assert_eq!(versioned["new_version"], "0.2.0", "{versioned}");
        let configured = mcp.answered(
            "memstead_mem_configure",
            json!({ "name": "kept", "description": "Kept things." }),
        );
        assert_eq!(configured["description"], "Kept things.", "{configured}");
    }
    assert_ne!(
        git(&fx.gitdir, &["rev-parse", "refs/heads/__MEMSTEAD"]),
        registry_before
    );
    let listed = json_ok(&fx.store, &["mem", "list"]);
    assert_eq!(listed["mems"][0]["version"], "0.2.0", "{listed}");
    assert_eq!(listed["mems"][0]["description"], "Kept things.", "{listed}");
}

#[test]
fn a_read_only_mount_refuses_every_pin_switch() {
    let fx = view_with_a_schema_of_its_own("read-only");
    let before = where_the_mem_lives(&fx.store, &fx.gitdir, "kept");
    let read_only = |refusal: &Value| {
        assert_eq!(refusal["details"], json!({ "mem": "kept" }), "{refusal}");
    };

    // A built-in target, which a write mount takes, and the view's own
    // schema: the capability answers first for both.
    for target in ["default@1.2.0", "trinkets@0.1.0"] {
        {
            let mut mcp = Mcp::start(&fx.view);
            read_only(&mcp.refused(
                "memstead_mem_set_schema",
                json!({ "mem": "kept", "schema": target }),
                "READ_ONLY_MOUNT",
            ));
        }
        read_only(&json_refused(
            &fx.view,
            &["mem", "set-schema", "kept", target],
            "READ_ONLY_MOUNT",
        ));
        assert_eq!(where_the_mem_lives(&fx.store, &fx.gitdir, "kept"), before);
    }

    let original = roster_that_fails_to_boot(&fx.view);
    for target in ["default@1.2.0", "trinkets@0.1.0"] {
        read_only(&json_refused(
            &fx.view,
            &["mem", "set-schema", "kept", target],
            "READ_ONLY_MOUNT",
        ));
        assert_eq!(where_the_mem_lives(&fx.store, &fx.gitdir, "kept"), before);
    }
    fs::write(fx.view.join(".memstead/state/mounts.json"), original).unwrap();
    assert_eq!(pin_of(&fx.view, "kept"), "default@1.3.0");
}
