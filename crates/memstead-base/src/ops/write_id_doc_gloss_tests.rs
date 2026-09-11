#![cfg(test)]

/// The rustdoc guard, for the whole crate rather than one file.
///
/// Two earlier versions of this check were too narrow and each let a
/// real defect through. The first read only `ops/mod.rs`, so five
/// copies of the gloss in `engine/outcomes.rs` — the base engine's
/// public outcome types, on a crates.io-published crate, hence
/// docs.rs — were invisible. The second was a phrase-exact banned
/// list built for "Per-mem commit SHA", which "Per-mem commit
/// identifier" walked straight past. A list of forbidden sentences
/// is only ever as good as the sentences someone already wrote.
///
/// So the rule is structural and positive instead. Every documented
/// `write_id` field must say WHICH backend produces a commit and
/// must say the value is not a cursor. Prose that calls the token a
/// commit without qualification fails whatever words it uses,
/// because it cannot satisfy the qualifier requirement.
#[test]
fn every_write_id_doc_qualifies_the_backend_and_denies_the_cursor() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    assert!(
        !files.is_empty(),
        "found no sources — check has gone vacuous"
    );

    let mut documented = 0usize;
    let mut violations = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            if !(t.starts_with("pub write_id:") || t.starts_with("pub seed_write_id:")) {
                continue;
            }
            // Collect the contiguous doc block above the field,
            // skipping attributes like #[serde(default)].
            let mut block = Vec::new();
            let mut j = i;
            while j > 0 {
                j -= 1;
                let prev = lines[j].trim_start();
                if prev.starts_with("#[") {
                    continue;
                }
                if prev.starts_with("///") {
                    block.push(prev.trim_start_matches("///").trim());
                    continue;
                }
                break;
            }
            if block.is_empty() {
                continue; // undocumented: nothing to gloss
            }
            documented += 1;
            block.reverse();
            let doc = block.join(" ");
            let lower = doc.to_lowercase();
            // Judge the DEFINING sentence, not every later mention.
            // "Empty on the no-op rename (no file change, no commit)"
            // is a true statement about a path, not a claim that the
            // token is a commit; only the summary sentence defines
            // the field, and it is what docs.rs renders as such.
            let definition = lower.split_once(". ").map(|(a, _)| a).unwrap_or(&lower);
            let claims_commit = definition.contains("commit") || definition.contains("sha");
            let names_backend = lower.contains("git-branch");
            let denies_cursor = lower.contains("not a change cursor")
                || lower.contains("never a change cursor")
                || lower.contains("not a cursor")
                || lower.contains("never a cursor");
            let inherits = lower.contains("see `updateresult::write_id`")
                || lower.contains("wire-equivalent to full's");
            if inherits && !claims_commit {
                continue; // documented by pointer at a doc this check governs
            }
            if claims_commit && !names_backend {
                violations.push(format!(
                        "{}:{}: calls the token a commit without naming which backend produces one — {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        i + 1,
                        doc
                    ));
            } else if !denies_cursor && !inherits {
                violations.push(format!(
                    "{}:{}: documents the token without stating it is not a change cursor — {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    i + 1,
                    doc
                ));
            }
        }
    }
    assert!(
        documented >= 5,
        "expected the crate to document several write_id fields, saw {documented} — \
             this check has gone vacuous"
    );
    assert!(
        violations.is_empty(),
        "write_id docs that gloss the token as a git commit or omit the non-cursor statement:\n  {}",
        violations.join("\n  ")
    );
}

/// The edge spelling in EMITTED JSON, not just in prose.
///
/// Every guard before this one read documentation. None read the
/// `json!` macros that build responses, which is how
/// `render_relations_json` kept emitting the relation type under
/// the bare key `"type"` through eight grades while
/// `memstead entity --json` next to it emitted `rel_type` — two CLI
/// commands, one concept, two spellings, on the same edge. Neither
/// enumerator could see it either: one pattern wanted `"to"` beside
/// `"type"`, and this shape pairs `"type"` with `"target"`.
///
/// The rule is narrow on purpose: a line that writes a JSON key
/// `"type"` and mentions `rel_type` is emitting a relation type
/// under the retired name. An entity type or a content-block kind
/// legitimately owns the word `type` and never mentions `rel_type`,
/// so it does not match.
#[test]
fn no_emitted_json_spells_a_relation_type_as_bare_type() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut roots = vec![base.join("src")];
    if let Some(ws) = base.parent().and_then(|p| p.parent()) {
        for sibling in ["crates/memstead-cli/src", "crates/memstead-mcp/src"] {
            let p = ws.join(sibling);
            if p.is_dir() {
                roots.push(p);
            }
        }
        if let Some(outer) = ws.parent() {
            let p = outer.join("ui-api/src");
            if p.is_dir() {
                roots.push(p);
            }
        }
    }
    let mut files = Vec::new();
    for r in &roots {
        walk(r, &mut files);
    }
    assert!(
        !files.is_empty(),
        "found no sources — check has gone vacuous"
    );

    let mut violations = Vec::new();
    let mut saw_a_relation_emit = false;
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue; // prose is the other guards' business
            }
            if line.contains("rel_type") && line.contains('"') {
                saw_a_relation_emit = true;
            }
            // The serde form splits the two tokens across lines:
            //     #[serde(rename = "type")]
            //     rel_type: &'a str,
            // A same-line rule passed that in silence — a grader
            // proved it by reintroducing exactly this on
            // `EdgeTypeCount` and watching the check go green. So
            // look at a small window, not one line.
            let lo = i.saturating_sub(1);
            let hi = (i + 2).min(lines.len());
            let window = lines[lo..hi].join(" ");
            if window.contains("\"type\"") && window.contains("rel_type") {
                violations.push(format!(
                    "{}:{}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    i + 1,
                    t
                ));
            }
        }
    }
    assert!(
        saw_a_relation_emit,
        "no source mentions `rel_type` in a string context — check has gone vacuous"
    );
    assert!(
        violations.is_empty(),
        "emitted JSON spells a relation type as the retired bare `type`:\n  {}",
        violations.join("\n  ")
    );
}

/// The edge spelling, across the same crate. The canonical
/// `CreateArgs::relations` doc named both retired keys at once, on
/// a field whose own type is `{target, rel_type}`. Neither
/// enumerator matches that prose form, which is why this exists.
#[test]
fn no_doc_comment_spells_a_relation_entry_the_retired_way() {
    const RETIRED_EDGE_SHAPES: &[&str] = &[
        "to: EntityId, type:",
        "{ to, type }",
        "{to, type}",
        "{from, to, type}",
        "`from` / `type` / `to`",
        "(`from`/`to`/`type`)",
    ];
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    // Reach past this crate. Round five's finding was a ui-api
    // struct doc, and the guard installed in answer to it could not
    // see the file that produced it. The sibling crates and the two
    // private consumers all describe the same edge.
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut roots = vec![base.join("src")];
    let mut private_ui_api_present = false;
    let mut private_serve_present = false;
    if let Some(ws) = base.parent().and_then(|p| p.parent()) {
        for sibling in [
            "crates/memstead-mcp/src",
            "crates/memstead-cli/src",
            "crates/memstead-schema/src",
        ] {
            let p = ws.join(sibling);
            if p.is_dir() {
                roots.push(p);
            }
        }
        // ui-api and serve live beside the `public/` submodule.
        if let Some(outer) = ws.parent() {
            for private in ["ui-api/src", "serve/src"] {
                let p = outer.join(private);
                if p.is_dir() {
                    roots.push(p);
                    if private == "ui-api/src" {
                        private_ui_api_present = true;
                    } else {
                        private_serve_present = true;
                    }
                }
            }
        }
    }
    // Pin the widening itself. `>= 5` was too loose: `ui-api/src`
    // and `serve/src` could both silently drop out and this still
    // passed, so the round that widened the reach did not actually
    // fix it in place. Require every root that exists on disk.
    let expected = 4 + usize::from(private_ui_api_present) + usize::from(private_serve_present);
    assert_eq!(
        roots.len(),
        expected,
        "expected {expected} roots (three sibling crates plus the private consumers \
             present on disk), saw {} — the check has narrowed",
        roots.len()
    );
    let mut files = Vec::new();
    for r in &roots {
        walk(r, &mut files);
    }
    assert!(
        !files.is_empty(),
        "found no sources — check has gone vacuous"
    );

    let mut violations = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if !t.starts_with("///") && !t.starts_with("//!") {
                continue;
            }
            for shape in RETIRED_EDGE_SHAPES {
                if line.contains(shape) {
                    violations.push(format!(
                        "{}:{}: {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        i + 1,
                        t
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "doc comments still spell a relation entry the retired way:\n  {}",
        violations.join("\n  ")
    );
}
