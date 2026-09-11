#![cfg(test)]

use super::*;
use memstead_base::binding_run::Source;
use memstead_base::pipeline::{IngestTrigger, PatternEntry};

fn guidance(goal: Option<&str>, avoid: Option<&str>) -> ResolvedGuidance {
    ResolvedGuidance {
        goal: goal.map(str::to_string),
        avoid: avoid.map(str::to_string),
    }
}

/// Goal and avoid both present: two headers, trimmed prose, block ends in
/// a blank line — byte-for-byte the plugin's `goalAndAvoidBlock`.
#[test]
fn renders_goal_and_avoid_blocks() {
    let out = render_goal_and_avoid(&guidance(Some("  build coverage  "), Some("no stubs")));
    assert_eq!(
        out,
        "## Goal\n\nbuild coverage\n\n## Failure modes to avoid\n\nno stubs\n\n"
    );
}

/// Goal only: a single header block ending in a blank line.
#[test]
fn renders_goal_only() {
    assert_eq!(
        render_goal_and_avoid(&guidance(Some("build coverage"), None)),
        "## Goal\n\nbuild coverage\n\n"
    );
}

/// Avoid only: a single header block ending in a blank line.
#[test]
fn renders_avoid_only() {
    assert_eq!(
        render_goal_and_avoid(&guidance(None, Some("no stubs"))),
        "## Failure modes to avoid\n\nno stubs\n\n"
    );
}

/// Neither present (and no pass-through): a lone newline, matching the
/// plugin's `lines.join('\n') + '\n'` on an empty block.
#[test]
fn empty_guidance_yields_a_newline() {
    assert_eq!(render_goal_and_avoid(&guidance(None, None)), "\n");
    // An all-whitespace field is treated as absent.
    assert_eq!(render_goal_and_avoid(&guidance(Some("   "), None)), "\n");
}

fn primary(medium_type: MediumType, scope: Vec<PatternEntry>) -> ResolvedSource {
    ResolvedSource::Primary(Source {
        name: "f".to_string(),
        medium_type,
        pointer: "../src".to_string(),
        change_detection: None,
        scope,
        engagement: None,
        preparation: None,
    })
}

fn resolved(name: &str, intent: Option<&str>, sources: Vec<ResolvedSource>) -> ResolvedIngest {
    ResolvedIngest {
        name: name.to_string(),
        mode: BuildMode::Discovery,
        trigger: IngestTrigger::Loop,
        batch_size: 20,
        deny_paths: vec![],
        projection_ref: format!("{name}/p"),
        projection_mem: name.to_string(),
        projection_name: "p".to_string(),
        intent: intent.map(str::to_string),
        sources,
        destination_mem: name.to_string(),
        rules: None,
        post_actions: None,
    }
}

fn process_present(name: &str) -> ProcessMemInfo {
    ProcessMemInfo {
        present: true,
        skipped: false,
        notice: None,
        leaf_name: name.to_string(),
        mem_label: format!("ingest/{name}"),
    }
}

fn allow(path: &str) -> PatternEntry {
    PatternEntry {
        path: path.to_string(),
        mode: PatternMode::Allow,
    }
}

fn deny(path: &str) -> PatternEntry {
    PatternEntry {
        path: path.to_string(),
        mode: PatternMode::Deny,
    }
}

/// The about-the-source block trims the intent; no intent → empty string.
#[test]
fn renders_intent() {
    let r = resolved("macos", Some("  Swift app source.  "), vec![]);
    assert_eq!(
        render_intent(&r),
        "## About the source\n\nSwift app source.\n\n"
    );
    let none = resolved("macos", None, vec![]);
    assert_eq!(render_intent(&none), "");
}

/// The situation block prints the name/mode, the three fixed paragraphs,
/// and the present-process-mem line, ending in a blank line.
#[test]
fn renders_situation_with_present_process_mem() {
    let r = resolved("macos", None, vec![]);
    let out = render_situation(&r, &process_present("macos"));
    assert!(out.starts_with(
        "## Situation\n\nYou are running one iteration of `macos` (discovery mode) inside a loop."
    ));
    assert!(out.contains("Mutating the destination is this run's mandate:"));
    assert!(out.contains("The `PreCompact` hook fires near the limit"));
    assert!(out.contains("A paired process mem `ingest/macos` (schema `ingest@0.5.0`) carries destination-quality debt"));
    assert!(
        out.ends_with("write rules.\n\n"),
        "block ends in a blank line"
    );
}

/// The skipped (one-shot) and failed-to-create process-mem branches each
/// render their own note.
#[test]
fn situation_process_mem_branches() {
    let mut r = resolved("os", None, vec![]);
    r.mode = BuildMode::OneShot;
    let skipped = ProcessMemInfo {
        present: false,
        skipped: true,
        notice: None,
        leaf_name: "os".to_string(),
        mem_label: "ingest/os".to_string(),
    };
    assert!(
        render_situation(&r, &skipped)
            .contains("No process mem is paired with this ingest (mode=one-shot;")
    );

    let failed = ProcessMemInfo {
        present: false,
        skipped: false,
        notice: Some("engine offline".to_string()),
        leaf_name: "os".to_string(),
        mem_label: "ingest/os".to_string(),
    };
    let out = render_situation(&resolved("os", None, vec![]), &failed);
    assert!(out.contains("could not be auto-created — engine offline."));
    assert!(out.contains("memstead mem init os --org-path ingest --schema ingest@0.5.0"));
}

/// Operative data: a primary source with paths/ignore, a reference mem
/// with its cross-mem note, the destination with its schema, and the
/// paired process mem — byte-for-byte the plugin's block.
#[test]
fn renders_operative_data_full() {
    let r = resolved(
        "macos",
        None,
        vec![
            primary(
                MediumType::Codebase,
                vec![allow("src/**/*.swift"), deny("src/gen/**")],
            ),
            ResolvedSource::Reference {
                mem: "engine".to_string(),
            },
        ],
    );
    let out = render_operative_data(
        &r,
        &process_present("macos"),
        Some("macos-code@0.1.0"),
        None,
        &[],
    );
    let expected = "\
## Operative data

### Sources

- **f** (codebase, primary) — `../src`
  - Paths: src/**/*.swift
  - Ignore: src/gen/**
- **graph** (reference) — mem: engine

Sources tagged `(reference)` are read-only context for cross-mem edges — search them, never write into them. Only `(primary)` sources are ingested into the destination.

**Cross-mem references:** consult `memstead_search mem=engine` before authoring cross-mem edges. The target entity must exist — a wiki-link or relationship to a missing target either auto-stubs (silent) or fails authorization (`CROSS_MEM_RELATION`).

### Destination

- **macos** — schema: `macos-code@0.1.0`

### Paired process mem

- **ingest/macos** — schema: `ingest@0.5.0`. Inspect via `memstead_overview` / `memstead_search mem=macos`.
\n";
    assert_eq!(out, expected);
}

/// Operative data without references or a destination schema: no cross-mem
/// note, a bare destination line.
#[test]
fn renders_operative_data_minimal() {
    let r = resolved("g", None, vec![primary(MediumType::Filesystem, vec![])]);
    let skipped = ProcessMemInfo {
        present: false,
        skipped: true,
        notice: None,
        leaf_name: "g".to_string(),
        mem_label: "ingest/g".to_string(),
    };
    let out = render_operative_data(&r, &skipped, None, Some("**absent** — probe"), &[]);
    // The bullet carries the pointer: an agent told to read a source
    // must be able to see WHICH tree it was pointed at.
    assert!(out.contains("- **f** (filesystem, primary) — `"));
    assert!(!out.contains("Cross-mem references"), "no reference note");
    assert!(out.contains("### Destination\n\n- **g**\n"));
    // The caller decides the destination note — this renderer only
    // places it, because the remedy depends on the workspace shape.
    assert!(
        out.contains("**absent** — probe"),
        "the caller's destination note must be rendered: {out}",
    );
    assert!(
        !out.contains("Paired process mem"),
        "skipped process mem omitted"
    );
}

/// A source whose scope still speaks the retired workspace-relative
/// dialect is warned about IN THE BRIEF — the operative-data block, the
/// one surface a binding running only build and sync ever reads. Without
/// it the notes reach only the verify report and the `--full` refusal,
/// and such a binding is never told its scope selects nothing.
#[test]
fn operative_data_warns_on_retired_scope_dialect() {
    let r = resolved(
        "g",
        None,
        // Pointer `../src` (the helper's default): one pattern in the
        // retired dialect (begins with the pointer), one converged.
        vec![primary(
            MediumType::Filesystem,
            vec![allow("../src/**/*.md"), allow("notes/**")],
        )],
    );
    let skipped = ProcessMemInfo {
        present: false,
        skipped: true,
        notice: None,
        leaf_name: "g".to_string(),
        mem_label: "ingest/g".to_string(),
    };
    let out = render_operative_data(&r, &skipped, None, None, &[]);
    assert!(
        out.contains("workspace root"),
        "the block names the retired dialect: {out}"
    );
    assert!(
        out.contains("../src/**/*.md"),
        "the offending pattern is named: {out}"
    );
    assert!(
        out.contains("`**/*.md`"),
        "the mechanical rewrite is offered: {out}"
    );

    // A converged scope renders no warning.
    let clean = resolved(
        "g",
        None,
        vec![primary(MediumType::Filesystem, vec![allow("**/*.md")])],
    );
    let out2 = render_operative_data(&clean, &skipped, None, None, &[]);
    assert!(
        !out2.contains("workspace root"),
        "no warning without a retired-dialect pattern: {out2}"
    );
}

/// The discovery assembly concatenates the truthy blocks in order; an
/// empty changed-slice preface (source unmoved) drops out.
#[test]
fn assembles_discovery_brief() {
    let r = resolved(
        "macos",
        Some("Swift source."),
        vec![primary(MediumType::Codebase, vec![allow("src/**")])],
    );
    let g = guidance(Some("build coverage"), None);
    let pm = process_present("macos");
    let brief = assemble_discovery_brief(&r, &[], &g, &pm, Some("s@1"), None, &[], "");

    // Blocks appear in order and the empty preface is dropped.
    let sit = brief.find("## Situation").unwrap();
    let src = brief.find("## About the source").unwrap();
    let goal = brief.find("## Goal").unwrap();
    let op = brief.find("## Operative data").unwrap();
    let anchors = brief.find("## Provenance — anchor your writes").unwrap();
    assert!(
        sit < src && src < goal && goal < op && op < anchors,
        "blocks in brief order"
    );
    assert!(
        !brief.contains("## Source changes"),
        "no changed-slice block when preface empty"
    );

    // A non-empty preface is appended verbatim at the end.
    let with_slice = assemble_discovery_brief(
        &r,
        &[],
        &g,
        &pm,
        Some("s@1"),
        None,
        &[],
        "## Source changes\n\n…\n\n",
    );
    assert!(with_slice.ends_with("## Source changes\n\n…\n\n"));
}

fn slice(deleted: &[&str], modified: &[&str], added: &[&str]) -> Slice {
    Slice {
        deleted: deleted.iter().map(|s| s.to_string()).collect(),
        modified: modified.iter().map(|s| s.to_string()).collect(),
        added: added.iter().map(|s| s.to_string()).collect(),
    }
}

fn cmd(key: &str, token: &str) -> SyncCommand {
    SyncCommand {
        key: key.to_string(),
        token: token.to_string(),
    }
}

fn note(source: &str, reason: NoSignalReason) -> NoSignalNote {
    NoSignalNote {
        medium_type: None,
        source: source.to_string(),
        reason,
    }
}

/// The provenance block names a prepared-form source and scopes the
/// `content` advice to file and span anchors; a binding without a
/// preparation carries no such paragraph.
#[test]
fn anchor_instruction_names_prepared_form_sources() {
    let mut resolved = resolved("home", None, vec![primary(MediumType::Codebase, vec![])]);
    let plain = render_anchor_instruction(&resolved);
    assert!(!plain.contains("hash a prepared form"));
    if let Some(ResolvedSource::Primary(src)) = resolved.sources.first_mut() {
        src.preparation = Some(memstead_base::preparation::CODE_MAP.to_string());
    }
    let prepared = render_anchor_instruction(&resolved);
    assert!(
        prepared.contains("hash a prepared form (`code-map`)"),
        "{prepared}"
    );
    assert!(prepared.contains("interface digest"));
    assert!(prepared.contains("for a `file` or `span` anchor pass the artifact's `content`"));
    assert!(prepared.contains("a `tree` anchor takes no content"));
}

/// A delivery sequence renders in its total order, numbered by position,
/// capped at the batch with the remainder counted, disposed units
/// subtracted but counted, and its unit ids kept out of the alphabetical
/// class lists (a file-level id in the same slice still lists there).
#[test]
fn changed_slice_renders_delivery_sequences_in_order() {
    use memstead_base::preparation::UnitChange;
    let unit = |id: &str, order: &str, change: UnitChange, disposed: bool| DeliveredUnit {
        id: id.to_string(),
        order_key: order.to_string(),
        change,
        disposed,
    };
    let units = vec![
        unit(
            "log/b.md#2026-08-20T00:00:00",
            "2026-08-20T00:00:00",
            UnitChange::Added,
            true,
        ),
        unit(
            "log/a.md#2026-08-21T00:00:00",
            "2026-08-21T00:00:00",
            UnitChange::Deleted,
            false,
        ),
        unit(
            "log/b.md#2026-08-22T00:00:00",
            "2026-08-22T00:00:00",
            UnitChange::Modified,
            false,
        ),
        unit(
            "log/a.md#2026-08-23T00:00:00",
            "2026-08-23T00:00:00",
            UnitChange::Added,
            false,
        ),
    ];
    let cursor = SourceCursor {
        // `slice(deleted, modified, added)`.
        union: slice(
            &["log/a.md#2026-08-21T00:00:00"],
            &["log/b.md#2026-08-22T00:00:00"],
            &[
                "log/a.md#2026-08-23T00:00:00",
                "log/b.md#2026-08-20T00:00:00",
                "other/x.rs",
            ],
        ),
        write_commands: vec![],
        reseed: vec![],
        no_signal: vec![],
        any_changes: true,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "home".to_string(),
        binding_id: "home/log".to_string(),
        delivery: vec![DeliverySequence {
            source: "log".to_string(),
            preparation: "dated-entries".to_string(),
            first_run: false,
            degraded: true,
            batch: 2,
            units,
        }],
    };
    let out = render_changed_slice(&cursor);
    assert!(
        out.contains("### Delivery sequence: `log` (`dated-entries`)"),
        "{out}"
    );
    assert!(out.contains("The units that changed since the last pass"));
    assert!(out.contains("No baseline content was retrievable"));
    let listed: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    assert_eq!(
        listed,
        vec![
            "2. `log/a.md#2026-08-21T00:00:00` (deleted)",
            "3. `log/b.md#2026-08-22T00:00:00` (changed)",
        ],
        "positions are total-order positions; the disposed first unit is skipped"
    );
    assert!(out.contains("…and 1 more, presented in order once these are disposed"));
    assert!(out.contains("1 unit of this sequence already disposed"));
    // The class lists carry only the file-level id.
    assert!(out.contains("**Added:**\n- `other/x.rs`\n"), "{out}");
    assert!(!out.contains("**Modified:**"));
    assert!(!out.contains("**Deleted:**"));
}

/// No changes and no reseed → the block is empty (brief stays a plain roam).
#[test]
fn changed_slice_empty_when_nothing_moved() {
    let cursor = SourceCursor {
        union: slice(&[], &[], &[]),
        write_commands: vec![],
        reseed: vec![],
        no_signal: vec![],
        any_changes: false,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "engine".to_string(),
        binding_id: "engine/graph".to_string(),
        delivery: vec![],
    };
    assert_eq!(render_changed_slice(&cursor), "");
}

/// A zero-selecting deny entry surfaces as a rendered warning even when
/// nothing else moved — it is never a silent no-op. The entry name and the
/// migration hint both appear.
#[test]
fn changed_slice_renders_dead_deny_warning() {
    let cursor = SourceCursor {
        union: slice(&[], &[], &[]),
        write_commands: vec![],
        reseed: vec![],
        no_signal: vec![],
        any_changes: false,
        degraded: false,
        dead_denies: vec!["dev".to_string(), "typo/**".to_string()],
        dest_mem: "engine".to_string(),
        binding_id: "engine/graph".to_string(),
        delivery: vec![],
    };
    let out = render_changed_slice(&cursor);
    assert!(out.contains("deny_paths` entries match nothing"));
    assert!(out.contains("- `dev`"));
    assert!(out.contains("- `typo/**`"));
}

/// A changed pass renders deleted-first, then the recording block — built
/// here from single-line literals transcribed from the plugin so any
/// line-continuation drift in the impl is caught.
#[test]
fn changed_slice_renders_slice_and_recording() {
    let cursor = SourceCursor {
        union: slice(&["a.rs"], &["b.rs"], &[]),
        write_commands: vec![cmd("engine-graph/source", "HEADSHA")],
        reseed: vec![],
        no_signal: vec![],
        any_changes: true,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "engine".to_string(),
        binding_id: "engine/graph".to_string(),
        delivery: vec![],
    };
    let expected_lines = [
        "## Source changes since the last sync\n",
        "The source moved since this graph was last synced. Steer this pass at these changed artifacts **first** — they are where the graph is most likely now wrong.\n",
        "**Deleted:**",
        "- `a.rs`",
        "",
        "**Modified:**",
        "- `b.rs`",
        "",
        "### Recording your dispositions (do this LAST)\n",
        "Only after you have worked the changed artifacts above — and only for the artifacts you actually judged — record a disposition for each, so the next pass targets just what changes next. This advance is resumable and non-stalling: a partial pass is honored, and if the source moves mid-pass the remaining slice re-presents (remaining + new) without losing your recorded work.\n",
        "Anchored work disposes itself: at advance time, every listed artifact that an anchor in the destination mem references is marked `worked` automatically (an explicit disposition you pass wins over the auto-mark), except an artifact with mention-steered entities, which waits for your disposition. Supply dispositions only for the residue — artifacts you skipped, judged out of intent, worked without anchors, or walked by mention. The gate accepts only artifact ids listed above — an unknown id refuses the whole call. When every artifact is disposed, the sync baseline advances automatically. Run:\n",
        "```sh",
        r#"memstead projection advance engine/graph --dispositions '{"<artifact>": "<disposition>", ...}'"#,
        "```",
        "If you were interrupted before finishing, that is fine — your recorded dispositions persist, and the next run re-presents only what is left.\n",
    ];
    assert_eq!(
        render_changed_slice(&cursor),
        format!("{}\n", expected_lines.join("\n"))
    );
}

/// The reseed-only path names the first-sync keys and still emits the
/// recording block (the reseed baselines).
#[test]
fn changed_slice_reseed_only() {
    let cursor = SourceCursor {
        union: slice(&[], &[], &[]),
        write_commands: vec![],
        reseed: vec![cmd("ing/f", "TOK")],
        no_signal: vec![],
        any_changes: false,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "d".to_string(),
        binding_id: "d/p".to_string(),
        delivery: vec![],
    };
    let out = render_changed_slice(&cursor);
    assert!(out.starts_with("## Source changes since the last sync\n\n"));
    assert!(out.contains(
            "No usable sync baseline exists for `ing/f` — none was recorded, or the recorded one is not a commit of the source's repo (foreign or garbage-collected). Treating the current source state as the baseline. No priority slice from it this pass; proceed as usual."
        ));
    assert!(out.contains(
        r#"memstead projection advance d/p --dispositions '{"<artifact>": "<disposition>", ...}'"#
    ));
    assert!(
        !out.contains("The source moved"),
        "no 'moved' copy when only reseeding"
    );
}

/// Every no-signal reason renders a distinct, named note under the preface,
/// distinguishable from one another and from a genuinely-unchanged source
/// (which renders nothing). With no changes and no reseed there is no
/// recording block, but the preface is non-empty — a source's blindness is
/// visible. `signal:none` renders literally.
#[test]
fn changed_slice_renders_no_signal_reasons_distinguishably() {
    let cursor = SourceCursor {
        union: slice(&[], &[], &[]),
        write_commands: vec![],
        reseed: vec![],
        no_signal: vec![
            note("code-facet", NoSignalReason::Unscoped),
            note("plan-facet", NoSignalReason::DetectionNone),
            note("git-facet", NoSignalReason::GitUnavailable),
            note("ref-mem", NoSignalReason::GraphSnapshotMissing),
        ],
        any_changes: false,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "d".to_string(),
        binding_id: "d/p".to_string(),
        delivery: vec![],
    };
    let out = render_changed_slice(&cursor);
    assert!(out.starts_with("## Source changes since the last sync\n"));
    assert!(out.contains("Some sources produced **no change signal**"));
    // Each source is named and carries its own distinct reason text.
    assert!(out.contains("- `code-facet`: unscoped facet (no allow patterns)"));
    assert!(
        out.contains("- `plan-facet`: `signal:none`"),
        "detection-none renders the literal signal:none state"
    );
    assert!(out.contains("- `git-facet`: git signal unavailable"));
    assert!(out.contains("- `ref-mem`: graph snapshot missing"));
    // The four reason texts are mutually distinct.
    let texts = [
        no_signal_reason_text(NoSignalReason::Unscoped, None),
        no_signal_reason_text(NoSignalReason::DetectionNone, None),
        no_signal_reason_text(NoSignalReason::GitUnavailable, None),
        no_signal_reason_text(NoSignalReason::GraphSnapshotMissing, None),
    ];
    for (i, a) in texts.iter().enumerate() {
        for b in &texts[i + 1..] {
            assert_ne!(a, b, "each no-signal reason must render distinctly");
        }
    }
    // No baseline to advance → no recording block, no "moved" copy.
    assert!(!out.contains("### Recording your dispositions"));
    assert!(!out.contains("The source moved"));
}

/// A changed source and a no-signal source coexist: the changed slice AND
/// the no-signal note both render in the one preface, and the changed
/// source still emits its recording command.
#[test]
fn changed_slice_mixes_changes_and_no_signal() {
    let cursor = SourceCursor {
        union: slice(&[], &["b.rs"], &[]),
        write_commands: vec![cmd("ing/f", "HEAD")],
        reseed: vec![],
        no_signal: vec![note("other", NoSignalReason::Unscoped)],
        any_changes: true,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "d".to_string(),
        binding_id: "d/p".to_string(),
        delivery: vec![],
    };
    let out = render_changed_slice(&cursor);
    assert!(out.contains("The source moved"));
    assert!(out.contains("**Modified:**"));
    assert!(out.contains("- `other`: unscoped facet"));
    assert!(out.contains("### Recording your dispositions"));
    assert!(out.contains(
        r#"memstead projection advance d/p --dispositions '{"<artifact>": "<disposition>", ...}'"#
    ));
}

/// The one-shot lens block: destination-set table, routing rule (when set),
/// idempotency, report template, and archive note (when set).
#[test]
fn renders_one_shot_lens_block() {
    let mut r = resolved("os", Some("plan source"), vec![]);
    r.rules = Some(serde_json::json!({ "routing": "route each entity to its spec" }));
    r.post_actions = Some(serde_json::json!({ "archive_source": true }));

    let out = render_one_shot_lens(&r, Some("planning@0.1.0"), Some("the plan graph"));
    assert!(out.starts_with("## Mode: one-shot — lens routing\n\n"));
    assert!(out.contains(
            "### Destination set\n\n| Mem | Schema | Purpose |\n|-------|--------|---------|\n| os | planning@0.1.0 | the plan graph |\n"
        ));
    assert!(out.contains("### Routing rule\n\n```\nroute each entity to its spec\n```\n"));
    assert!(out.contains("### Idempotency"));
    assert!(out.contains("### Report: os"));
    assert!(out.contains("### Archive after run"));
    assert!(out.ends_with("is set on this ingest.\n\n"));

    // No routing / no archive → those sections are omitted; a bare schema
    // and default purpose fall back.
    let bare = resolved("os", None, vec![]);
    let out2 = render_one_shot_lens(&bare, None, None);
    assert!(out2.contains("| os | (none) | (no purpose declared) |"));
    assert!(!out2.contains("### Routing rule"));
    assert!(!out2.contains("### Archive after run"));
    assert!(out2.contains("### End-of-run report"));
}

/// The one-shot brief assembles situation (one-shot mode) + intent +
/// goal/avoid + operative-data + the lens block; no process mem, no slice.
#[test]
fn assembles_one_shot_brief() {
    let mut r = resolved(
        "os",
        Some("src"),
        vec![primary(MediumType::Filesystem, vec![])],
    );
    r.mode = BuildMode::OneShot;
    let g = guidance(Some("goal"), None);
    let skipped = ProcessMemInfo {
        present: false,
        skipped: true,
        notice: None,
        leaf_name: "os".to_string(),
        mem_label: "ingest/os".to_string(),
    };
    let brief = assemble_one_shot_brief(
        &r,
        &[],
        &g,
        &skipped,
        Some("s@1"),
        None,
        &[],
        Some("purpose"),
    );
    assert!(brief.contains("(one-shot mode)"));
    assert!(brief.contains("No process mem is paired with this ingest (mode=one-shot;"));
    assert!(brief.contains("## Mode: one-shot — lens routing"));
    assert!(
        brief.contains("## Provenance — anchor your writes"),
        "one-shot carries the anchor instruction"
    );
    assert!(
        !brief.contains("## Source changes"),
        "one-shot has no changed-slice"
    );
}

/// Beyond SLICE_CAP entries an overflow line stands in; the degraded flag
/// adds the coarse-targeting note. Also exercises shell-quoting a JSON
/// digest token (embedded quotes).
#[test]
fn changed_slice_caps_and_degrades_and_quotes() {
    let many: Vec<String> = (0..SLICE_CAP + 3).map(|i| format!("f{i}.rs")).collect();
    let cursor = SourceCursor {
        union: Slice {
            deleted: vec![],
            modified: vec![],
            added: many,
        },
        write_commands: vec![cmd("ing/f", r#"{"v":1,"aggregate":"x"}"#)],
        reseed: vec![],
        no_signal: vec![],
        any_changes: true,
        degraded: true,
        dead_denies: vec![],
        dest_mem: "d".to_string(),
        binding_id: "d/p".to_string(),
        delivery: vec![],
    };
    let out = render_changed_slice(&cursor);
    assert!(out.contains(&format!("- …and {} more added", 3)));
    assert!(out.contains("Precise change history for one or more facets was unavailable"));
    // The brief renders the `projection advance` line (the token is no longer
    // an operator command — the engine computes and records it, D4/D7).
    assert!(out.contains(
        r#"memstead projection advance d/p --dispositions '{"<artifact>": "<disposition>", ...}'"#
    ));
}

// ---- verify + sync briefs ----------------------------------

fn finding(class: FindingClass, target: FindingTarget, detail: &str) -> Finding {
    Finding {
        key: crate::findings::FindingKey {
            binding_hash: "h".to_string(),
            source_head: "s".to_string(),
        },
        facet: "src".to_string(),
        target,
        class,
        detail: detail.to_string(),
        created_at: "1".to_string(),
    }
}

fn anchor_target(entity: &str, artifact: &str) -> FindingTarget {
    FindingTarget::Anchor {
        entity: entity.to_string(),
        artifact: artifact.to_string(),
    }
}

fn artifact_target(artifact: &str) -> FindingTarget {
    FindingTarget::Artifact {
        artifact: artifact.to_string(),
    }
}

fn empty_cursor() -> SourceCursor {
    SourceCursor {
        union: slice(&[], &[], &[]),
        write_commands: vec![],
        reseed: vec![],
        no_signal: vec![],
        any_changes: false,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "engine".to_string(),
        binding_id: "engine/graph".to_string(),
        delivery: vec![],
    }
}

/// C1 — the verify brief measures + adjudicates, and carries NO
/// destination-mutation instruction of any kind. It names the sync brief as
/// the repair home and prints its explicit no-mutation refusal.
#[test]
fn verify_brief_measures_and_refuses_mutation() {
    let r = resolved("engine", None, vec![]);
    let out = render_verify_brief(&r, 3);
    // Measurement + capped adjudication instructions.
    assert!(out.starts_with("## Verify — measure fidelity, do not mutate"));
    assert!(out.contains("3 finding(s) are queued for adjudication"));
    assert!(out.contains("per-run adjudication cap"));
    assert!(out.contains("this is a measurement, not a repair"));
    // C1 REFUSAL: structurally no destination-mutation instruction. The
    // brief never tells the agent to write into the mem — it says the
    // opposite, and hands repairs to the sync brief.
    //
    // Reworded 2026-08-20. This assertion used to pin "Verify writes
    // **nothing** into the destination mem", which was false at the time: a
    // completed run recorded its findings store, backfilled observed anchor
    // hashes and wrote a `#verified` baseline. The refusal this test exists
    // to protect is about ENTITY CONTENT — that is what an agent reading the
    // brief must not touch — so the claim was narrowed to what is true
    // rather than deleted, and the bookkeeping asserted alongside it so the
    // correction cannot silently regress.
    //
    // Amended 2026-09-03 (C6): the `#verified` baseline now rides
    // `--advance`, so a bare run leaves the mem's CONFIG alone again. The
    // findings store and the anchor-hash backfill still happen on every
    // run, so the narrowed entity-content claim stands unchanged — it was
    // the honest one either way — and the brief must now also name the
    // flag, so a reader is never told a bare verify records a baseline it
    // does not.
    assert!(out.contains("Verify writes **no entity content**"));
    assert!(out.contains("`#verified` baseline"));
    assert!(out.contains("`--advance`"));
    assert!(out.contains("memstead projection brief --sync"));
    // No create/update/relate/delete *instruction* — the only occurrences of
    // those verbs are in the negated "do not …" refusal line.
    assert!(out.contains("do not create or delete an entity"));
    assert!(!out.contains("via `memstead_create`"));
    assert!(!out.contains("Run `memstead_update`"));

    // Backlog 0 → the spot-check phrasing, still no mutation instruction.
    let zero = render_verify_brief(&r, 0);
    assert!(zero.contains("No findings are queued for adjudication"));
    assert!(zero.contains("record any drift you observe as a finding"));
    assert!(zero.contains("Verify writes **no entity content**"));
}

/// C2 — the sync brief carries BOTH inputs in ONE render: the cursor slice
/// (the changed artifacts) AND the open findings (`current(key)`), plus the
/// commits-nothing posture.
#[test]
fn sync_brief_carries_both_cursor_and_findings() {
    let r = resolved("engine", None, vec![]);
    let cursor = SourceCursor {
        union: slice(&["gone.rs"], &["moved.rs"], &[]),
        write_commands: vec![cmd("engine/graph/src#synced", "HEAD")],
        reseed: vec![],
        no_signal: vec![],
        any_changes: true,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "engine".to_string(),
        binding_id: "engine/graph".to_string(),
        delivery: vec![],
    };
    let findings = vec![
        finding(
            FindingClass::Drifted,
            anchor_target("engine--e", "src/moved.rs"),
            "prepared-content hash drifted",
        ),
        finding(
            FindingClass::Uncovered,
            artifact_target("src/new.rs"),
            "in scope, no anchor",
        ),
    ];
    let out = render_sync_brief(
        &r,
        &cursor,
        &findings,
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    // Both inputs present in one brief (C2).
    assert!(out.contains("## Source changes since the last sync"));
    assert!(out.contains("`moved.rs`"));
    assert!(out.contains("## Open findings to repair"));
    assert!(out.contains("`engine--e` → `src/moved.rs`"));
    assert!(out.contains("`src/new.rs`"));
    // Sole-writer + commits-nothing posture (C3).
    assert!(out.contains("sole maintenance writer"));
    assert!(out.contains("commits each one **per-mutation**"));
    assert!(out.contains("Sync commits nothing."));
}

/// C3 — the sync brief carries the whole absorbed reconcile judgment: the
/// five conservatism rules, edge-removal conservatism, and
/// rationale-not-changelog. Each rule is quoted verbatim so absorption is
/// verifiable against the C4 diff artifact.
#[test]
fn sync_brief_absorbs_reconcile_conservatism() {
    let r = resolved("engine", None, vec![]);
    let findings = vec![finding(
        FindingClass::Uncovered,
        artifact_target("src/x.rs"),
        "d",
    )];
    let out = render_sync_brief(
        &r,
        &empty_cursor(),
        &findings,
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    // Five conservatism rules.
    assert!(out.contains("Unsure whether an entity is affected — skip it."));
    assert!(
        out.contains(
            "Do not create a new entity unless the change clearly introduces a new concept"
        )
    );
    assert!(
        out.contains("Do not delete an entity unless the change removes the concept entirely.")
    );
    assert!(out.contains("Never rewrite a section that has not changed"));
    assert!(
        out.contains("No speculative edges — add only relationships the diff literally introduces")
    );
    // Edge-removal conservatism — flags, never auto-removes.
    assert!(out.contains("A dropped dependency FLAGS, it does not auto-remove."));
    assert!(out.contains("Edge removal is out of scope for sync."));
    // Rationale-not-changelog.
    assert!(out.contains("Rationale is reasoning, not a changelog."));
    assert!(out.contains("`[commit <hash>]` log-style entries"));
}

/// C3 — the first-sync/adopt framing (the brief half): a mem predating its
/// binding is onboarding, expected-0%, with the backfill path — never a
/// failure. The changed-slice reseed carries the per-facet first-sync note.
#[test]
fn sync_brief_renders_adopt_framing() {
    let mut r = resolved("engine", None, vec![]);
    // In a real ResolvedIngest, `name` is the canonical binding id
    // `<mem>/<stem>` while `destination_mem` is the mem — the header uses the
    // mem, the backfill command uses the binding id.
    r.name = "engine/graph".to_string();
    let out = render_sync_brief(
        &r,
        &empty_cursor(),
        &[],
        &[],
        true,
        &crate::advance::ExclusionLedger::default(),
    );
    assert!(out.contains("## First sync — adopting `engine`"));
    assert!(out.contains("0% anchored is expected — this is onboarding, not a failure."));
    assert!(out.contains("do **not** replay the whole history"));
    assert!(out.contains("**Backfill path:**"));
    assert!(out.contains("memstead projection verify engine/graph"));
}

/// The reseed (first-sync, no cursor) framing lives in the embedded
/// changed-slice preface — the sync brief inherits it for free.
#[test]
fn sync_brief_inherits_first_sync_reseed_framing() {
    let r = resolved("engine", None, vec![]);
    let mut cursor = empty_cursor();
    cursor.reseed = vec![cmd("engine/graph/src#synced", "TOK")];
    let out = render_sync_brief(
        &r,
        &cursor,
        &[],
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    assert!(out.contains("No usable sync baseline exists for"));
    assert!(out.contains("Treating the current source state as the baseline"));
}

/// A no-work sync pass (nothing moved, no findings, not adopt) renders a
/// compact "nothing to sync" note and no repair machinery — a valid outcome.
#[test]
fn sync_brief_nothing_to_sync() {
    let r = resolved("engine", None, vec![]);
    let out = render_sync_brief(
        &r,
        &empty_cursor(),
        &[],
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    assert!(out.contains("## Nothing to sync"));
    assert!(!out.contains("## How to repair"));
    assert!(!out.contains("## Open findings"));
}

/// C2 REFUSAL complement — the sync brief is the ONLY render carrying repair
/// instructions; the verify brief carries none. The verify brief has no
/// "## How to repair" / "## Open findings to repair" block; the sync brief
/// has both.
#[test]
fn only_sync_brief_carries_repair_instructions() {
    let r = resolved("engine", None, vec![]);
    let findings = vec![finding(
        FindingClass::Drifted,
        anchor_target("engine--e", "src/a.rs"),
        "d",
    )];
    let verify = render_verify_brief(&r, 1);
    let sync = render_sync_brief(
        &r,
        &empty_cursor(),
        &findings,
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    // Verify: no repair section, no repair verbs as instructions.
    assert!(!verify.contains("## How to repair"));
    assert!(!verify.contains("Update the affected section"));
    // Sync: both repair sections present.
    assert!(sync.contains("## How to repair — be conservative"));
    assert!(sync.contains("## Open findings to repair"));
    assert!(sync.contains("Update the affected section to match"));
}

/// Criterion — a changed slice renders the bounded **stale-claim search**
/// step: extract the changed facts, search the destination mem for claims
/// about them, judge only entities whose claims mention a changed fact.
#[test]
fn sync_brief_changed_slice_renders_stale_claim_search() {
    let r = resolved("engine", None, vec![]);
    let cursor = SourceCursor {
        union: slice(&[], &["moved.rs"], &[]),
        write_commands: vec![cmd("engine/graph/src#synced", "HEAD")],
        reseed: vec![],
        no_signal: vec![],
        any_changes: true,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "engine".to_string(),
        binding_id: "engine/graph".to_string(),
        delivery: vec![],
    };
    let out = render_sync_brief(
        &r,
        &cursor,
        &[],
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    assert!(out.contains("## Stale claims beyond the slice — search, then judge"));
    // The search is bound to the changed facts and the destination mem.
    assert!(out.contains("Extract the **changed facts** from the changed artifacts above"));
    assert!(out.contains("search the destination mem `engine`"));
    assert!(out.contains("`memstead_search`"));
    assert!(out.contains("judge **only** the entities whose claims actually mention"));
    // Bounded shape, spelled out: not a live-verify, not a rewrite license,
    // and an empty fact set (cosmetic change) instructs nothing.
    assert!(out.contains("not a live-verify of every entity"));
    assert!(out.contains("not a rewrite license"));
    assert!(out.contains("the fact set is empty and this step ends with no"));
    // REFUSAL complement: the never-rewrite-unchanged-sections rule still
    // rides the same brief — idempotence stays protected.
    assert!(out.contains("Never rewrite a section that has not changed"));
}

/// REFUSAL — the stale-claim search is absent from every pass whose cursor
/// carries no changed artifacts: findings-only, reseed-only (first sync),
/// and nothing-to-sync briefs instruct no fact search and no mem sweep.
#[test]
fn sync_brief_without_changes_renders_no_stale_claim_search() {
    let r = resolved("engine", None, vec![]);
    let heading = "## Stale claims beyond the slice";

    // Findings-only pass (source unmoved).
    let findings = vec![finding(
        FindingClass::Uncovered,
        artifact_target("src/x.rs"),
        "d",
    )];
    let out = render_sync_brief(
        &r,
        &empty_cursor(),
        &findings,
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    assert!(!out.contains(heading), "findings-only pass must not search");

    // Reseed-only pass (first sync, no diffable slice).
    let mut reseed_cursor = empty_cursor();
    reseed_cursor.reseed = vec![cmd("engine/graph/src#synced", "TOK")];
    let out = render_sync_brief(
        &r,
        &reseed_cursor,
        &[],
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    assert!(!out.contains(heading), "reseed-only pass must not search");

    // Nothing-to-sync pass.
    let out = render_sync_brief(
        &r,
        &empty_cursor(),
        &[],
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    assert!(!out.contains(heading));
}

/// A large findings group caps at FINDINGS_CAP with an overflow line —
/// mirroring the changed-slice cap, so no facet renders unbounded.
#[test]
fn sync_brief_caps_large_findings_group() {
    let r = resolved("engine", None, vec![]);
    let findings: Vec<Finding> = (0..FINDINGS_CAP + 4)
        .map(|i| {
            finding(
                FindingClass::Uncovered,
                artifact_target(&format!("src/f{i}.rs")),
                "d",
            )
        })
        .collect();
    let out = render_sync_brief(
        &r,
        &empty_cursor(),
        &findings,
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    assert!(out.contains("- …and 4 more"));
    // The last few beyond the cap are not rendered inline.
    assert!(!out.contains(&format!("src/f{}.rs", FINDINGS_CAP + 3)));
}

/// Criterion 8 (loop economics) — the default loop path's sync brief is
/// **locked block-by-block** for a representative changed-slice pass: the
/// heading sequence below is the whole brief, in this order, and nothing
/// else. The only blocks this plan added to the loop path are the
/// stale-claim search and the head-durable findings
/// presentation — both locked here in place. The inventory
/// operation (`projection verify --full` + the `/sync --inventory` repair
/// loop) added NO block and NO line to this render, so a new block
/// appearing (or one moving) fails this test and must be a deliberate
/// loop-economics decision.
#[test]
fn sync_brief_block_sequence_locked_for_changed_slice() {
    let r = resolved("engine", None, vec![]);
    let cursor = SourceCursor {
        union: slice(&["gone.rs"], &["moved.rs"], &["new.rs"]),
        write_commands: vec![cmd("engine/graph/src#synced", "HEAD")],
        reseed: vec![],
        no_signal: vec![],
        any_changes: true,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "engine".to_string(),
        binding_id: "engine/graph".to_string(),
        delivery: vec![],
    };
    let findings = vec![
        finding(
            FindingClass::Drifted,
            anchor_target("engine--e", "src/moved.rs"),
            "prepared-content hash drifted",
        ),
        finding(
            FindingClass::Uncovered,
            artifact_target("src/new.rs"),
            "in scope, no anchor",
        ),
    ];
    let out = render_sync_brief(
        &r,
        &cursor,
        &findings,
        &[],
        false,
        &crate::advance::ExclusionLedger::default(),
    );
    let headings: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("## ") || l.starts_with("### "))
        .collect();
    assert_eq!(
        headings,
        vec![
            "## Sync — repair the graph to match the source",
            "## Source changes since the last sync",
            "### Recording your dispositions (do this LAST)",
            "## Stale claims beyond the slice — search, then judge",
            "## Open findings to repair",
            "### Drifted — the anchored content changed",
            "### Uncovered — a source artifact with no entity",
            // Deliberate addition (anchor-source plan): the sync
            // brief now carries the provenance instruction so
            // repair writes are anchored — and name their source.
            "## Provenance — anchor your writes",
            "## How to repair — be conservative",
        ],
        "the loop-path sync brief carries exactly these blocks, in this order"
    );
    // The brief closes on the conservatism block's final rule — nothing
    // (inventory or otherwise) rides after it.
    assert!(out.ends_with("`[commit <hash>]` log-style entries.\n\n"));
}

/// Criterion 8 REFUSAL — no brief on the default (non-inventory) path
/// carries any inventory machinery: not the build briefs (discovery /
/// one-shot), not the verify brief, not the sync brief in any of its
/// shapes (changed slice, findings-only, nothing-to-sync, adopt). The
/// inventory operation lives entirely in `projection verify --full` and
/// the `/sync --inventory` skill routing; the engine-side byte-compat of
/// the no-flag sampled verify is asserted in
/// `findings::tests::full_verify_uncaps_adjudication_and_walks_whole_source`
/// (extended there, not duplicated here). The minute-loop pays nothing
/// for inventory.
#[test]
fn no_default_path_brief_carries_inventory_machinery() {
    // Terms that exist only on the inventory surface (flag, skill mode,
    // report framing, termination rule). Matched case-insensitively.
    let inventory_terms = [
        "--full",
        "inventory",
        "full measurement",
        "did not converge",
        "quiescence",
    ];
    let assert_clean = |label: &str, text: &str| {
        let lower = text.to_lowercase();
        for term in inventory_terms {
            assert!(
                !lower.contains(term),
                "{label} must carry no inventory machinery (found {term:?})"
            );
        }
    };

    let r = resolved("engine", None, vec![]);
    let g = guidance(Some("build coverage"), None);
    let pm = process_present("engine");

    // Build briefs — with and without a changed-slice preface.
    let changed_cursor = SourceCursor {
        union: slice(&[], &["moved.rs"], &[]),
        write_commands: vec![cmd("engine/graph/src#synced", "HEAD")],
        reseed: vec![],
        no_signal: vec![],
        any_changes: true,
        degraded: false,
        dead_denies: vec![],
        dest_mem: "engine".to_string(),
        binding_id: "engine/graph".to_string(),
        delivery: vec![],
    };
    let preface = render_changed_slice(&changed_cursor);
    assert_clean(
        "discovery build brief (plain roam)",
        &assemble_discovery_brief(&r, &[], &g, &pm, Some("s@1"), None, &[], ""),
    );
    assert_clean(
        "discovery build brief (changed slice)",
        &assemble_discovery_brief(&r, &[], &g, &pm, Some("s@1"), None, &[], &preface),
    );
    assert_clean(
        "one-shot build brief",
        &assemble_one_shot_brief(&r, &[], &g, &pm, Some("s@1"), None, &[], Some("purpose")),
    );

    // Verify brief — with and without an adjudication backlog.
    assert_clean("verify brief (backlog)", &render_verify_brief(&r, 3));
    assert_clean("verify brief (no backlog)", &render_verify_brief(&r, 0));

    // Sync brief — every shape the loop renders.
    let findings = vec![finding(
        FindingClass::Drifted,
        anchor_target("engine--e", "src/moved.rs"),
        "d",
    )];
    assert_clean(
        "sync brief (changed slice + findings)",
        &render_sync_brief(
            &r,
            &changed_cursor,
            &findings,
            &[],
            false,
            &crate::advance::ExclusionLedger::default(),
        ),
    );
    assert_clean(
        "sync brief (findings-only)",
        &render_sync_brief(
            &r,
            &empty_cursor(),
            &findings,
            &[],
            false,
            &crate::advance::ExclusionLedger::default(),
        ),
    );
    assert_clean(
        "sync brief (nothing to sync)",
        &render_sync_brief(
            &r,
            &empty_cursor(),
            &[],
            &[],
            false,
            &crate::advance::ExclusionLedger::default(),
        ),
    );
    assert_clean(
        "sync brief (adopt)",
        &render_sync_brief(
            &r,
            &empty_cursor(),
            &[],
            &[],
            true,
            &crate::advance::ExclusionLedger::default(),
        ),
    );
}
