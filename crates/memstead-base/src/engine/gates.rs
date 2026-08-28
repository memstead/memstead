//! The gates brief — the engine-rendered standing of every declared
//! `transition_requires_checks` gate (graph-plans plan 03, the yes
//! branch of the gated-transition spike).
//!
//! For each mounted mem whose schema declares the constraint on at
//! least one type, the brief lists every non-stub entity of a gated
//! type with its gated field's current value, whether it stands at the
//! gated value ("closed") or before it ("open"), and — for open
//! entities — the related-set coverage the transition would require:
//! how many related entities the declared edges reach and which of
//! them lack a fresh confirming check record. The related-set
//! enumeration is [`crate::ops::health::transition_gate_standing`],
//! the same code the write-time refusal runs, so the brief can never
//! disagree with the gate.
//!
//! Open entities are listed in dependency order: a topological sort
//! over the edges BETWEEN gated entities whose rel-type the schema
//! declares acyclic (in the planning schema that is plan → plan
//! REQUIRES; any schema's acyclic ordering vocabulary works the same),
//! prerequisites first, ties by id. The brief reports state, never
//! policy: which open entity a consumer acts on next — and which it
//! must refuse (a human-gate marker, a parked status) — stays the
//! consumer's judgement over its schema's vocabulary.
//!
//! The renderer is the shared engine entry point
//! ([`Engine::render_gates_brief`]) per the brief-family precedent
//! (due, ingest, sync): CLI verb, byte-identical everywhere,
//! deliberately no MCP tool.

use std::collections::HashMap;

use memstead_schema::ConstraintDef;

use super::Engine;
use crate::entity::MetadataValue;

impl Engine {
    /// Render the gates brief as markdown. `mem_filter` restricts to
    /// one mem; the default walks every mounted mem whose schema
    /// declares the constraint. Deterministic given the store and the
    /// check ledger.
    pub fn render_gates_brief(&self, mem_filter: Option<&str>) -> String {
        let checks = self.check_state_provider();
        let mut sections: Vec<String> = Vec::new();
        let mut declaring_mems: Vec<String> = Vec::new();

        let mut mems: Vec<&str> = self
            .mounts
            .iter()
            .map(|m| m.mount.mem.as_str())
            .filter(|m| mem_filter.is_none_or(|f| f == *m))
            .collect();
        mems.sort_unstable();

        for mem in mems {
            let Some(schema) = self.schemas.get(mem) else {
                continue;
            };
            // The gated types of this schema, with their declarations.
            let mut gated: Vec<(&str, &ConstraintDef)> = Vec::new();
            for td in schema.types.values() {
                for c in &td.constraints {
                    if matches!(c, ConstraintDef::TransitionRequiresChecks { .. }) {
                        gated.push((td.name.as_str(), c));
                    }
                }
            }
            if gated.is_empty() {
                continue;
            }
            gated.sort_by_key(|(name, _)| *name);
            declaring_mems.push(mem.to_string());

            let mut lines: Vec<String> = Vec::new();
            lines.push(format!("## {mem}"));
            lines.push(String::new());

            for (type_name, c) in &gated {
                let ConstraintDef::TransitionRequiresChecks {
                    field,
                    to_value,
                    relationships,
                    direction,
                    ..
                } = c
                else {
                    continue;
                };
                lines.push(format!(
                    "Gate: `{type_name}` — `{field}: {to_value}` requires a fresh confirming \
                     check record on every entity related via [{}] ({}).",
                    relationships.join(", "),
                    match direction {
                        memstead_schema::PropagationDirection::Incoming => "incoming",
                        memstead_schema::PropagationDirection::Outgoing => "outgoing",
                    },
                ));
                lines.push(String::new());

                // Every non-stub entity of the gated type.
                let entities: Vec<_> = self
                    .store
                    .all_entities()
                    .filter(|e| e.mem == mem && !e.stub && e.entity_type == *type_name)
                    .collect();
                let mut closed: Vec<String> = Vec::new();
                struct OpenRow {
                    id: String,
                    value: String,
                    total: usize,
                    unchecked: Vec<crate::ops::health::UncheckedRelated>,
                }
                let mut open: Vec<OpenRow> = Vec::new();
                for e in &entities {
                    let value = match e.metadata.get(field.as_str()) {
                        Some(MetadataValue::String(s)) => s.clone(),
                        Some(v) => v.to_frontmatter_string(),
                        None => String::new(),
                    };
                    if value == *to_value {
                        closed.push(e.id.0.clone());
                    } else {
                        let (total, unchecked) = crate::ops::health::transition_gate_standing(
                            &self.store,
                            e,
                            relationships,
                            *direction,
                            None,
                            Some(&checks),
                        );
                        open.push(OpenRow {
                            id: e.id.0.clone(),
                            value,
                            total,
                            unchecked,
                        });
                    }
                }
                closed.sort();

                // Dependency order over acyclic edges between gated
                // entities: an edge source depends on its target, so
                // targets (prerequisites) list first.
                let acyclic: Vec<&str> = schema
                    .manifest
                    .relationships
                    .definitions
                    .iter()
                    .filter(|r| r.acyclic)
                    .map(|r| r.name.as_str())
                    .collect();
                let open_ids: Vec<String> = open.iter().map(|r| r.id.clone()).collect();
                let mut prereqs: HashMap<String, Vec<String>> = HashMap::new();
                for e in &entities {
                    if !open_ids.contains(&e.id.0) {
                        continue;
                    }
                    for rel in &e.relationships {
                        if acyclic.contains(&rel.rel_type.as_str())
                            && open_ids.contains(&rel.target.0)
                            && rel.target.0 != e.id.0
                        {
                            prereqs
                                .entry(e.id.0.clone())
                                .or_default()
                                .push(rel.target.0.clone());
                        }
                    }
                }
                let mut ordered: Vec<&OpenRow> = Vec::new();
                let mut placed: Vec<&str> = Vec::new();
                let mut remaining: Vec<&OpenRow> = open.iter().collect();
                remaining.sort_by(|a, b| a.id.cmp(&b.id));
                while !remaining.is_empty() {
                    let idx = remaining.iter().position(|r| {
                        prereqs
                            .get(&r.id)
                            .is_none_or(|p| p.iter().all(|d| placed.contains(&d.as_str())))
                    });
                    // A cycle among open entities cannot arise (the
                    // edges are engine-enforced acyclic); the fallback
                    // keeps the loop total anyway.
                    let idx = idx.unwrap_or(0);
                    let row = remaining.remove(idx);
                    placed.push(row.id.as_str());
                    ordered.push(row);
                }

                lines.push(format!(
                    "Closed (at `{to_value}`): {}",
                    if closed.is_empty() {
                        "none".to_string()
                    } else {
                        closed
                            .iter()
                            .map(|id| format!("`{id}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ));
                lines.push(String::new());
                if ordered.is_empty() {
                    lines.push("Open: none — every gated entity stands closed.".to_string());
                } else {
                    lines.push("Open, in dependency order (prerequisites first):".to_string());
                    for row in &ordered {
                        let coverage = if row.total == 0 {
                            "no related entities — the gate is vacuously satisfiable".to_string()
                        } else if row.unchecked.is_empty() {
                            format!(
                                "{}/{} related confirmed — gate satisfiable",
                                row.total, row.total
                            )
                        } else {
                            format!(
                                "{}/{} related confirmed — unconfirmed: {}",
                                row.total - row.unchecked.len(),
                                row.total,
                                row.unchecked
                                    .iter()
                                    .map(|u| format!("`{}` ({})", u.id, u.state))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        lines.push(format!(
                            "- `{}` — {field}: {} — {coverage}",
                            row.id,
                            if row.value.is_empty() {
                                "(unset)"
                            } else {
                                row.value.as_str()
                            },
                        ));
                    }
                }
                lines.push(String::new());
            }
            sections.push(lines.join("\n"));
        }

        let mut out = String::from("# Gates brief\n\n");
        if declaring_mems.is_empty() {
            out.push_str(match mem_filter {
                Some(f) => {
                    sections.push(format!(
                        "No `transition_requires_checks` gate declared by the schema of `{f}` \
                         (or the mem is not mounted)."
                    ));
                    ""
                }
                None => "No mounted mem's schema declares a `transition_requires_checks` gate.",
            });
        }
        out.push_str(&sections.join("\n"));
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    /// A workspace with no gate-declaring schema renders the honest
    /// empty brief — never an empty string, never an invented section.
    /// The substantive standing logic is covered by
    /// `ops::health::tests::transition_requires_checks_gates_on_derived_state`
    /// (shared enumeration), and the rendered shape by the live
    /// dogfood workspace.
    #[test]
    fn brief_names_the_no_gates_case() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();
        let engine = crate::Engine::from_mounts(vec![(
            crate::engine::test_helpers::folder_mount("plain", dir.clone()),
            Box::new(crate::storage::FilesystemMemWriter::new(dir))
                as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        let brief = engine.render_gates_brief(None);
        assert!(
            brief.contains("No mounted mem's schema declares"),
            "{brief}"
        );
        let filtered = engine.render_gates_brief(Some("plain"));
        assert!(
            filtered.contains("No `transition_requires_checks` gate declared"),
            "{filtered}"
        );
    }
}
