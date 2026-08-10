//! The due-brief (first-author-path plan 08): a schema declares its
//! deadline axis ([`memstead_schema::DueAxis`]), the engine renders
//! "what is due next" as one deterministic markdown brief.
//!
//! Deterministic given (store, today): no model call, no scoring —
//! filter, sort, render. The only environmental input is the current
//! date, taken once per invocation and passed in (injectable in
//! tests). Ordering: overdue first, then ascending by date, ties
//! broken by entity id. The renderer lives here so the CLI and UniFFI
//! serve byte-identical content (the projection-brief precedent);
//! there is deliberately no MCP tool — briefs are the CLI/app family.
//!
//! Read-only mounts participate: a due date in an installed
//! compliance mem is precisely the multi-stakeholder case, and a
//! brief is a read surface. Third-party entries carry their origin
//! label and render as quoted data — a stranger's mem states a
//! deadline, it does not instruct.

use crate::engine::Engine;
use crate::entity::MetadataValue;
use crate::workspace::MountCapability;

/// The default window applied when `--within` is omitted — stated in
/// the CLI help and the changelog rather than unbounded.
pub const DEFAULT_DUE_WINDOW: &str = "90d";

/// A parsed relative window: `<N>d` days, `<N>m` calendar months,
/// `<N>y` calendar years.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueWindow {
    Days(u32),
    Months(u32),
    Years(u32),
}

/// Parse a relative window (`90d`, `6m`, `2y`). The error names the
/// accepted forms — the caller wraps it in its surface's typed
/// envelope.
pub fn parse_due_window(input: &str) -> Result<DueWindow, String> {
    let err = || {
        format!(
            "invalid window {input:?}: expected <N>d (days), <N>m (months), or <N>y (years) — \
             e.g. 90d, 6m, 2y"
        )
    };
    let (num, unit) = input.split_at(input.len().saturating_sub(1));
    let n: u32 = num.parse().map_err(|_| err())?;
    match unit {
        "d" => Ok(DueWindow::Days(n)),
        "m" => Ok(DueWindow::Months(n)),
        "y" => Ok(DueWindow::Years(n)),
        _ => Err(err()),
    }
}

/// Civil-date helpers over ISO `YYYY-MM-DD` strings — exact calendar
/// math without a date dependency (Howard Hinnant's `days_from_civil`
/// algorithm). Entities store dates as ISO strings, which order
/// lexically; arithmetic converts through day numbers.
fn parse_ymd(s: &str) -> Option<(i64, u32, u32)> {
    let mut it = s.splitn(3, '-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.get(..2).unwrap_or(it.next().unwrap_or("")).parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = ((m + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Public epoch-days → civil-date conversion for callers that derive
/// "today" from `SystemTime` (the UniFFI surface). Same algorithm the
/// window math uses.
pub fn civil_from_days_pub(days_since_epoch: i64) -> (i64, u32, u32) {
    civil_from_days(days_since_epoch)
}

fn last_day_of_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// `today + window`, as an ISO date string. Month/year addition
/// clamps the day-of-month (Jan 31 + 1m = Feb 28/29).
fn window_end(today: (i64, u32, u32), window: &DueWindow) -> String {
    let (y, m, d) = today;
    let (ey, em, ed) = match window {
        DueWindow::Days(n) => civil_from_days(days_from_civil(y, m, d) + *n as i64),
        DueWindow::Months(n) => {
            let total = (y * 12 + (m as i64 - 1)) + *n as i64;
            let ny = total.div_euclid(12);
            let nm = (total.rem_euclid(12) + 1) as u32;
            (ny, nm, d.min(last_day_of_month(ny, nm)))
        }
        DueWindow::Years(n) => {
            let ny = y + *n as i64;
            (ny, m, d.min(last_day_of_month(ny, m)))
        }
    };
    format!("{ey:04}-{em:02}-{ed:02}")
}

/// One brief entry, collected before rendering.
struct DueEntry {
    mem: String,
    third_party: bool,
    id: String,
    title: String,
    date: String,
    status: String,
    lead: Option<(String, String)>,
    overdue: bool,
}

impl Engine {
    /// Render the due-brief: every open entity whose declared due date
    /// falls inside `(-∞, today + window]`, across every mem whose
    /// pinned schema declares the axis (writable and read-only mounts
    /// alike), optionally filtered to one mem. `today` is an ISO
    /// `YYYY-MM-DD` string — the caller takes it once per invocation,
    /// tests inject it. A workspace with no declaring schema renders
    /// an honest empty brief, not an error.
    pub fn render_due_brief(
        &self,
        today: &str,
        window: &DueWindow,
        mem_filter: Option<&str>,
    ) -> Result<String, String> {
        let today_ymd =
            parse_ymd(today).ok_or_else(|| format!("invalid date {today:?}: expected YYYY-MM-DD"))?;
        let today_iso = format!(
            "{:04}-{:02}-{:02}",
            today_ymd.0, today_ymd.1, today_ymd.2
        );
        let end = window_end(today_ymd, window);

        // Mem → (schema, third_party) for every mount, capability
        // included: read-only mounts participate as labelled quoted
        // data.
        let mut entries: Vec<DueEntry> = Vec::new();
        let mut declaring_mems: Vec<String> = Vec::new();
        for mounted in &self.mounts {
            let mem = mounted.mount.mem.as_str();
            if let Some(filter) = mem_filter
                && mem != filter
            {
                continue;
            }
            let Some(schema) = self.schemas.get(mem) else {
                continue;
            };
            let declares = schema.types.values().any(|t| t.due.is_some());
            if !declares {
                continue;
            }
            declaring_mems.push(mem.to_string());
            let third_party = mounted.mount.capability == MountCapability::ReadOnly;
            for entity in self.store.all_entities() {
                if entity.mem != mem || entity.stub {
                    continue;
                }
                let Some(td) = schema.types.get(&entity.entity_type) else {
                    continue;
                };
                let Some(due) = &td.due else { continue };
                let status = match entity.metadata.get(&due.status_field) {
                    Some(MetadataValue::String(s)) => s.clone(),
                    _ => continue,
                };
                if !due.open_values.contains(&status) {
                    continue;
                }
                let date = match entity.metadata.get(&due.date_field) {
                    Some(MetadataValue::String(s)) => s.clone(),
                    _ => continue,
                };
                // Dates are ISO strings — take the date part, refuse
                // malformed values silently (they never entered via
                // the validated write path).
                let date_part = date.get(..10).unwrap_or(&date).to_string();
                if parse_ymd(&date_part).is_none() {
                    continue;
                }
                if date_part.as_str() > end.as_str() {
                    continue;
                }
                let lead = due.lead_section.as_ref().and_then(|key| {
                    entity
                        .sections
                        .get(key)
                        .filter(|body| !body.trim().is_empty())
                        .map(|body| (key.clone(), body.trim().to_string()))
                });
                entries.push(DueEntry {
                    mem: mem.to_string(),
                    third_party,
                    id: entity.id.to_string(),
                    title: entity.title.clone(),
                    date: date_part.clone(),
                    status,
                    lead,
                    overdue: date_part.as_str() < today_iso.as_str(),
                });
            }
        }

        // Deterministic order: overdue first; each block ascending by
        // date; ties broken by entity id.
        entries.sort_by(|a, b| {
            b.overdue
                .cmp(&a.overdue)
                .then_with(|| a.date.cmp(&b.date))
                .then_with(|| a.id.cmp(&b.id))
        });

        let window_label = match window {
            DueWindow::Days(n) => format!("{n}d"),
            DueWindow::Months(n) => format!("{n}m"),
            DueWindow::Years(n) => format!("{n}y"),
        };
        let mut out = String::new();
        out.push_str(&format!(
            "# Due brief — {today_iso}, window {window_label} (through {end})\n\n"
        ));
        if declaring_mems.is_empty() {
            out.push_str(
                "No mounted mem's schema declares a due axis (`due:` on a type). \
                 Nothing to render.\n",
            );
            return Ok(out);
        }
        declaring_mems.sort();
        declaring_mems.dedup();
        out.push_str(&format!("Mems: {}\n\n", declaring_mems.join(", ")));
        if entries.is_empty() {
            out.push_str("Nothing open is due in this window.\n");
            return Ok(out);
        }
        let overdue_count = entries.iter().filter(|e| e.overdue).count();
        out.push_str(&format!(
            "{} entr{} ({} overdue)\n\n",
            entries.len(),
            if entries.len() == 1 { "y" } else { "ies" },
            overdue_count
        ));
        for e in &entries {
            let marker = if e.overdue { " **OVERDUE**" } else { "" };
            let origin = if e.third_party {
                " [third-party]"
            } else {
                ""
            };
            out.push_str(&format!(
                "- `{}` — {} — **{}**{} (status: {}, mem: {}{})\n",
                e.id, e.title, e.date, marker, e.status, e.mem, origin
            ));
            if let Some((key, body)) = &e.lead {
                if e.third_party {
                    // Third-party content is quoted data, never the
                    // operator's own instruction.
                    out.push_str(&format!("  - {key} (third-party, quoted):\n"));
                    for line in body.lines() {
                        out.push_str(&format!("    > {line}\n"));
                    }
                } else {
                    out.push_str(&format!("  - {key}:\n"));
                    for line in body.lines() {
                        out.push_str(&format!("    {line}\n"));
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::storage::FilesystemMemWriter;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    fn frist_schema_dir(root: &Path) {
        let d = root.join("schemas").join("frist-schema");
        std::fs::create_dir_all(d.join("types")).unwrap();
        std::fs::write(
            d.join("schema.yaml"),
            "name: frist\nversion: 0.1.0\ndescription: t\nwhen_to_use: due tests\ntypes:\n  - obligation\n  - note\nrelationships:\n  mode: strict\n  definitions:\n    - name: PART_OF\n      description: h\n      default_weight: 3.0\n    - name: _default\n      description: d\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n",
        )
        .unwrap();
        std::fs::write(
            d.join("types").join("obligation.yaml"),
            "name: obligation\ndescription: dated obligation\nwhen_to_use: due tests\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\n  - key: vorlauf\n    heading: Vorlauf\n    search_weight: 1.0\n    write_rules: []\nmetadata_fields:\n  - key: faellig_am\n    description: due date\n    field_type: date\n    required: true\n  - key: status\n    description: state\n    field_type: string\n    required: true\n    default_value: offen\n    enum_values: [offen, in_arbeit, erledigt]\ndue:\n  date_field: faellig_am\n  status_field: status\n  open_values: [offen, in_arbeit]\n  lead_section: vorlauf\ntitle_weight: 100.0\ntext_fields: [body]\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields: [title, body, status, faellig_am]\nhealth_required_fields: []\nstaleness_threshold_days: 90\nwrite_rules: []\n",
        )
        .unwrap();
        std::fs::write(
            d.join("types").join("note.yaml"),
            "name: note\ndescription: undeclared type\nwhen_to_use: due tests\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields: [body]\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields: [title, body]\nhealth_required_fields: []\nstaleness_threshold_days: 90\nwrite_rules: []\n",
        )
        .unwrap();
    }

    fn obligation_md(title: &str, date: &str, status: &str, vorlauf: Option<&str>) -> String {
        let lead = vorlauf
            .map(|v| format!("\n## Vorlauf\n\n{v}\n"))
            .unwrap_or_default();
        format!(
            "---\ntype: obligation\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nfaellig_am: {date}\nstatus: {status}\n---\n# {title}\n\n## Body\n\nB.\n{lead}"
        )
    }

    fn mount_with(mem: &str, path: std::path::PathBuf, capability: MountCapability) -> Mount {
        Mount {
            mem: mem.to_string(),
            schema: Some("frist@0.1.0".parse().unwrap()),
            storage: MountStorage::Folder { path },
            capability,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    }

    /// The single fixture of criterion 1: overdue / in-window /
    /// out-of-window / closed-status / undeclared-type entities, plus
    /// a read-only third-party mem. Asserts membership, order, and
    /// entry content, deterministically at an injected date.
    #[test]
    fn due_brief_membership_order_and_labels() {
        let tmp = TempDir::new().unwrap();
        frist_schema_dir(tmp.path());
        let own = tmp.path().join("own");
        let foreign = tmp.path().join("foreign");
        std::fs::create_dir_all(own.join(".memstead")).unwrap();
        std::fs::create_dir_all(foreign.join(".memstead")).unwrap();
        // The authoritative pin is the mem's own config, not the
        // mount's expectation assertion.
        for dir in [&own, &foreign] {
            std::fs::write(
                dir.join(".memstead/config.json"),
                "{\n  \"version\": \"1.0.0\",\n  \"description\": \"due fixture\",\n  \"schema\": \"frist@0.1.0\"\n}",
            )
            .unwrap();
        }
        std::fs::write(
            own.join("wartung.md"),
            obligation_md("Wartung", "2026-09-01", "offen", Some("Handwerker beauftragen")),
        )
        .unwrap();
        std::fs::write(
            own.join("frist-alt.md"),
            obligation_md("Frist Alt", "2026-07-01", "in_arbeit", None),
        )
        .unwrap();
        std::fs::write(
            own.join("weit-weg.md"),
            obligation_md("Weit Weg", "2027-06-01", "offen", None),
        )
        .unwrap();
        std::fs::write(
            own.join("erledigt.md"),
            obligation_md("Erledigt", "2026-08-20", "erledigt", None),
        )
        .unwrap();
        std::fs::write(
            own.join("plain-note.md"),
            "---\ntype: note\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Plain Note\n\n## Body\n\nB.\n",
        )
        .unwrap();
        // Same-date tiebreak pair (ids decide).
        std::fs::write(
            own.join("b-gleich.md"),
            obligation_md("B Gleich", "2026-09-10", "offen", None),
        )
        .unwrap();
        std::fs::write(
            own.join("a-gleich.md"),
            obligation_md("A Gleich", "2026-09-10", "offen", None),
        )
        .unwrap();
        // Third-party read-only mem with an overdue entry.
        std::fs::write(
            foreign.join("fremd-frist.md"),
            obligation_md("Fremd Frist", "2026-06-15", "offen", Some("Nur zur Kenntnis")),
        )
        .unwrap();

        let own_writer = FilesystemMemWriter::new(own.clone());
        let foreign_writer = FilesystemMemWriter::new(foreign.clone());
        let engine = Engine::from_mounts_with_schemas_dir(
            vec![
                (
                    mount_with("own", own, MountCapability::Write),
                    Box::new(own_writer) as Box<dyn MemBackend>,
                ),
                (
                    mount_with("foreign", foreign, MountCapability::ReadOnly),
                    Box::new(foreign_writer) as Box<dyn MemBackend>,
                ),
            ],
            Some(&tmp.path().join("schemas")),
        )
        .unwrap();

        let brief = engine
            .render_due_brief("2026-08-10", &DueWindow::Days(90), None)
            .unwrap();

        // Membership: in-window + overdue present; out-of-window,
        // closed, undeclared-type absent.
        for present in [
            "own--wartung",
            "own--frist-alt",
            "foreign--fremd-frist",
            "own--a-gleich",
            "own--b-gleich",
        ] {
            assert!(brief.contains(present), "{present} missing:\n{brief}");
        }
        for absent in ["weit-weg", "erledigt", "plain-note"] {
            assert!(!brief.contains(absent), "{absent} leaked:\n{brief}");
        }

        // Order: overdue ascending first, then in-window ascending,
        // ties by id.
        let pos = |needle: &str| brief.find(needle).unwrap();
        assert!(pos("foreign--fremd-frist") < pos("own--frist-alt"), "{brief}");
        assert!(pos("own--frist-alt") < pos("own--wartung"), "{brief}");
        assert!(pos("own--wartung") < pos("own--a-gleich"), "{brief}");
        assert!(pos("own--a-gleich") < pos("own--b-gleich"), "{brief}");

        // Overdue marking, entry content, lead section.
        assert!(brief.contains("**2026-07-01** **OVERDUE**"), "{brief}");
        assert!(brief.contains("Handwerker beauftragen"), "{brief}");
        assert!(brief.contains("status: offen"), "{brief}");

        // Third-party labelling: origin label + quoted lead content.
        assert!(brief.contains("[third-party]"), "{brief}");
        assert!(brief.contains("vorlauf (third-party, quoted):"), "{brief}");
        assert!(brief.contains("> Nur zur Kenntnis"), "{brief}");
        // Own lead content is NOT quoted.
        assert!(brief.contains("    Handwerker beauftragen"), "{brief}");

        // Determinism: same inputs, same bytes.
        let again = engine
            .render_due_brief("2026-08-10", &DueWindow::Days(90), None)
            .unwrap();
        assert_eq!(brief, again);

        // Mem filter narrows to one mem.
        let own_only = engine
            .render_due_brief("2026-08-10", &DueWindow::Days(90), Some("own"))
            .unwrap();
        assert!(!own_only.contains("foreign--fremd-frist"), "{own_only}");
        assert!(own_only.contains("own--wartung"), "{own_only}");
    }

    /// Window parsing and calendar arithmetic.
    #[test]
    fn window_parse_and_calendar_math() {
        assert_eq!(parse_due_window("90d").unwrap(), DueWindow::Days(90));
        assert_eq!(parse_due_window("6m").unwrap(), DueWindow::Months(6));
        assert_eq!(parse_due_window("2y").unwrap(), DueWindow::Years(2));
        for bad in ["", "d", "90", "90w", "-1d", "1.5m"] {
            let err = parse_due_window(bad).unwrap_err();
            assert!(err.contains("<N>d"), "error names accepted forms: {err}");
        }
        // Day clamping: Jan 31 + 1m = Feb 28 (2026 is not a leap year).
        assert_eq!(window_end((2026, 1, 31), &DueWindow::Months(1)), "2026-02-28");
        assert_eq!(window_end((2024, 1, 31), &DueWindow::Months(1)), "2024-02-29");
        assert_eq!(window_end((2026, 8, 10), &DueWindow::Days(90)), "2026-11-08");
        assert_eq!(window_end((2026, 11, 15), &DueWindow::Months(2)), "2027-01-15");
        assert_eq!(window_end((2024, 2, 29), &DueWindow::Years(1)), "2025-02-28");
    }

    /// A workspace with no declaring schema renders the honest empty
    /// brief, not an error.
    #[test]
    fn no_declaring_schema_renders_honest_empty_brief() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            crate::engine::test_helpers::folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let brief = engine
            .render_due_brief("2026-08-10", &DueWindow::Days(90), None)
            .unwrap();
        assert!(brief.contains("No mounted mem's schema declares a due axis"), "{brief}");
    }
}

#[cfg(test)]
mod obligation_builtin_tests {
    use super::*;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::{cli_actor, empty_create_args};
    use crate::storage::FilesystemMemWriter;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    fn obligation_mount(mem: &str, path: std::path::PathBuf) -> Mount {
        Mount {
            mem: mem.to_string(),
            schema: Some("obligation@0.1.0".parse().unwrap()),
            storage: MountStorage::Folder { path },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    }

    fn obligation_engine(tmp: &TempDir) -> Engine {
        let mem_dir = tmp.path().join("duties");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead/config.json"),
            "{\n  \"version\": \"1.0.0\",\n  \"description\": \"obligation fixture\",\n  \"schema\": \"obligation@0.1.0\"\n}",
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        Engine::from_mounts(vec![(
            obligation_mount("duties", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap()
    }

    fn obligation_args(
        mem: &str,
        title: &str,
        due_date: &str,
        status: &str,
    ) -> crate::engine::CreateEntityArgs {
        let mut args = empty_create_args(mem, title);
        args.entity_type = "obligation".to_string();
        args.sections = indexmap::IndexMap::from_iter([
            ("duty".to_string(), "Who owes what.".to_string()),
            ("consequence".to_string(), "What forfeits.".to_string()),
        ]);
        args.metadata = indexmap::IndexMap::from_iter([
            ("due_date".to_string(), due_date.to_string()),
            ("status".to_string(), status.to_string()),
        ]);
        args.relations = vec![crate::ops::RelateArg {
            to: crate::entity::EntityId::new(mem, "some-subject"),
            rel_type: "CONCERNS".to_string(),
            description: None,
        }];
        args
    }

    /// Criterion 1 + 3: a mem pinned to the shipped builtin accepts a
    /// conformant obligation, refuses a nonconformant one with the
    /// standard envelope, and `render_due_brief` renders the fixture
    /// correctly under the declared axis.
    #[test]
    fn shipped_obligation_schema_accepts_refuses_and_renders_due() {
        let tmp = TempDir::new().unwrap();
        let mut engine = obligation_engine(&tmp);
        let (actor, client) = cli_actor();

        // Conformant — widened-grammar title with '&' and '.'.
        engine
            .create_entity(
                obligation_args(
                    "duties",
                    "Renew Registration No. 4711 & File Proof",
                    "2026-09-01",
                    "open",
                ),
                actor,
                Some(&client),
                None,
            )
            .expect("conformant obligation lands");
        engine
            .create_entity(
                obligation_args("duties", "Overdue Filing", "2026-07-01", "in_progress"),
                actor,
                Some(&client),
                None,
            )
            .expect("second obligation lands");
        engine
            .create_entity(
                obligation_args("duties", "Done Duty", "2026-08-01", "done"),
                actor,
                Some(&client),
                None,
            )
            .map(|_| ())
            .unwrap_err(); // done without completed_on → block (see below)

        // Nonconformant: unknown enum value refuses with the standard
        // recovery envelope.
        let err = engine
            .create_entity(
                obligation_args("duties", "Bad Status", "2026-09-01", "unknown"),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_ENUM_VALUE", "{err}");

        // Due brief renders the two open entities, overdue first.
        let brief = engine
            .render_due_brief("2026-08-10", &DueWindow::Days(90), None)
            .unwrap();
        let pos = |n: &str| brief.find(n).unwrap_or(usize::MAX);
        assert!(brief.contains("duties--overdue-filing"), "{brief}");
        assert!(
            brief.contains("duties--renew-registration-no-4711-file-proof"),
            "{brief}"
        );
        assert!(pos("duties--overdue-filing") < pos("duties--renew-registration-no-4711"));
        assert!(brief.contains("**OVERDUE**"), "{brief}");
    }

    /// Criterion 4: the shipped `requires_when` pair and the
    /// block-severity `required_outgoing` refuse exactly the
    /// field-schema writes.
    #[test]
    fn shipped_constraints_refuse_like_the_field_schema() {
        let tmp = TempDir::new().unwrap();
        let mut engine = obligation_engine(&tmp);
        let (actor, client) = cli_actor();

        // done without completed_on → block-tier requires_when.
        let err = engine
            .create_entity(
                obligation_args("duties", "Done Without Date", "2026-08-01", "done"),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED", "{err}");
        assert!(err.to_string().contains("completed_on"), "{err}");

        // criticality high without responsible → block-tier requires_when.
        let mut args = obligation_args("duties", "Critical Unowned", "2026-09-01", "open");
        args.metadata
            .insert("criticality".to_string(), "high".to_string());
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED", "{err}");
        assert!(err.to_string().contains("responsible"), "{err}");

        // No CONCERNS edge → block-severity required_outgoing refusal.
        let mut args = obligation_args("duties", "About Nothing", "2026-09-01", "open");
        args.relations.clear();
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert!(
            err.to_string().contains("CONCERNS") || err.code().contains("REQUIRED_OUTGOING"),
            "required_outgoing must refuse: {err} ({})",
            err.code()
        );

        // Complements: done WITH completed_on lands; high WITH
        // responsible lands.
        let mut args = obligation_args("duties", "Done Properly", "2026-08-01", "done");
        args.metadata
            .insert("completed_on".to_string(), "2026-08-01".to_string());
        engine
            .create_entity(args, actor, Some(&client), None)
            .expect("done with completed_on lands");
        let mut args = obligation_args("duties", "Critical Owned", "2026-09-01", "open");
        args.metadata
            .insert("criticality".to_string(), "high".to_string());
        args.metadata
            .insert("responsible".to_string(), "Operations".to_string());
        engine
            .create_entity(args, actor, Some(&client), None)
            .expect("high with responsible lands");
    }
}
