//! The copy steps `memstead schema <ref>` serves after a built-in's
//! README produce a package that validates, for every built-in: install,
//! copy, remove the seal, rename, migrate, validate.

use std::path::Path;

use assert_cmd::Command;

fn run(ws: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap()
}

#[test]
fn the_served_copy_steps_give_every_builtin_a_validating_package() {
    let mut failures = Vec::new();
    for pkg in memstead_schema::builtins::builtin_packages() {
        let (name, version) = (pkg.name.to_string(), pkg.version.to_string());
        let pin = format!("{name}@{version}");
        // The steps as served.
        let readme =
            String::from_utf8_lossy(&run(Path::new("."), &["schema", &pin]).stdout).to_string();
        let steps = memstead_schema::builtins::fork_steps(&name, &version);
        if !readme.contains(steps.trim()) {
            failures.push(format!("{pin}: the README render does not carry the steps"));
            continue;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path();
        assert!(
            run(
                ws,
                &["init", "--name", "scratch", "--schema", "default@1.3.0"]
            )
            .status
            .success()
        );
        let install = run(ws, &["schema", "install", &pin]);
        if !install.status.success() {
            failures.push(format!(
                "{pin}: install: {}",
                String::from_utf8_lossy(&install.stdout)
            ));
            continue;
        }
        let copy = ws.join(format!("my-{name}"));
        let src = ws.join(".memstead").join("schemas").join(&pin);
        let status = std::process::Command::new("cp")
            .arg("-R")
            .arg(&src)
            .arg(&copy)
            .status()
            .unwrap();
        assert!(status.success());
        let marker = copy.join(memstead_schema::loader::SCHEMA_FORMAT_MARKER_FILE);
        // `rm -f`: older generations carry no marker.
        let _ = std::fs::remove_file(&marker);
        let manifest = copy.join("schema.yaml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        let renamed = text.replacen(&format!("name: {name}\n"), &format!("name: my-{name}\n"), 1);
        assert_ne!(renamed, text, "{pin}: the manifest names the package");
        std::fs::write(&manifest, renamed).unwrap();
        let migrate = run(
            ws,
            &["schema", "migrate", copy.to_str().unwrap(), "--write"],
        );
        if !migrate.status.success() {
            failures.push(format!(
                "{pin}: migrate: {}",
                String::from_utf8_lossy(&migrate.stdout)
            ));
            continue;
        }
        let validate = run(ws, &["schema", "validate", copy.to_str().unwrap()]);
        if !validate.status.success() {
            failures.push(format!(
                "{pin}: validate: {}",
                String::from_utf8_lossy(&validate.stdout)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
