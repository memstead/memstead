//! The engine never fetches. A `url` anchor's content reaches the engine as
//! an observation an outside observer supplies (`verify-anchors
//! --observations`, `AnchorInput::content`); nothing in the base crate opens
//! a socket, and no network client is a dependency. This scan pins that
//! posture at the source: a network primitive or client crate appearing in
//! the base crate's code or manifest fails here, naming the line.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN_IN_SOURCE: &[&str] = &[
    "std::net::",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "reqwest",
    "ureq::",
    "hyper::",
    "curl::",
    "isahc",
    "attohttpc",
];

const FORBIDDEN_DEPENDENCIES: &[&str] = &[
    "reqwest",
    "ureq",
    "hyper",
    "curl",
    "isahc",
    "attohttpc",
    "surf",
    "h2",
];

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_base_crate_opens_no_socket_and_pulls_no_network_client() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    rust_sources(&root.join("src"), &mut sources);
    assert!(
        sources.len() > 50,
        "the scan walked {} files",
        sources.len()
    );

    let mut hits: Vec<String> = Vec::new();
    for path in &sources {
        let text = fs::read_to_string(path).expect("read source");
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            // Prose may name what the engine never does; code may not do it.
            if code.starts_with("//") || code.starts_with("*") {
                continue;
            }
            for token in FORBIDDEN_IN_SOURCE {
                if code.contains(token) {
                    hits.push(format!(
                        "{}:{}: `{token}` — {}",
                        path.strip_prefix(root).unwrap_or(path).display(),
                        n + 1,
                        code.trim()
                    ));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "the engine never fetches, but the base crate names a network primitive:\n{}",
        hits.join("\n")
    );

    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    let deps: Vec<&str> = manifest
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && l.contains('='))
        .filter_map(|l| l.split('=').next())
        .map(str::trim)
        .collect();
    let offending: Vec<&str> = deps
        .iter()
        .copied()
        .filter(|d| FORBIDDEN_DEPENDENCIES.contains(d))
        .collect();
    assert!(
        offending.is_empty(),
        "the base crate depends on a network client: {offending:?}"
    );
}
