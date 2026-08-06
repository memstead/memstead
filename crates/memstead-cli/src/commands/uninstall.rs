//! `memstead uninstall <name>` — the symmetric removal for
//! `memstead install`: unregister an installed read-mem's
//! workspace-level mount. Registration-only by default — the global
//! cache copy is shared across workspaces and survives (a later
//! `install` of the same archive re-registers without a download).

use clap::Parser;
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

/// Remove an installed read-mem's workspace-level mount. The global
/// cache copy survives; re-`install` re-registers it. Refuses while
/// entities in writable mems still hold graph edges into the read-mem
/// (`MEM_HAS_INCOMING_REFS` naming each referrer — remove those edges
/// first), and refuses writable mems (`MEM_NOT_READ_ONLY` — that is
/// `memstead mem delete` / `mem unregister` business).
#[derive(Parser, Debug)]
pub struct Args {
    /// The installed read-mem's name (the archive's internal name, as
    /// shown by `memstead mem list`).
    pub name: String,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut engine = crate::setup::full_engine(ctx)?;

    // Resolve, and refuse the two wrong-target shapes before any
    // mutation: unknown names, and writable mems (which have their own
    // lifecycle verbs).
    let Some(mount) = engine.mount(&args.name) else {
        return Err(CliError::new(
            ExitKind::NotFound,
            "UNKNOWN_MEM",
            format!(
                "no installed read-mem named `{}` — `memstead mem list` shows what is mounted",
                args.name
            ),
        )
        .with_details(json!({ "mem": args.name }))
        .into());
    };
    if mount.capability != memstead_base::MountCapability::ReadOnly {
        return Err(CliError::new(
            ExitKind::Validation,
            "MEM_NOT_READ_ONLY",
            format!(
                "`{}` is a writable mem — uninstall removes installed read-mems only; \
                 use `memstead mem unregister {}` (keep storage) or \
                 `memstead mem delete {}` (destroy storage)",
                args.name, args.name, args.name
            ),
        )
        .with_details(json!({ "mem": args.name }))
        .into());
    }

    // Incoming-refs gate, mirroring the delete/unregister posture: a
    // writable mem's entity that still holds a graph edge into the
    // read-mem would be left dangling. Same-mem and read-only-mount
    // referrers are irrelevant here (the whole mem disappears; RO
    // mounts cannot be rewritten).
    {
        use std::collections::BTreeSet;
        let store = engine.store();
        let doomed = args.name.as_str();
        let mut by_source: std::collections::BTreeMap<
            String,
            (memstead_base::EntityId, BTreeSet<String>),
        > = std::collections::BTreeMap::new();
        for entity in store.all_entities() {
            if entity.mem != doomed {
                continue;
            }
            for in_edge in store.incoming(&entity.id) {
                if in_edge.from.mem() == doomed
                    || !engine.mem_router().is_writable(in_edge.from.mem())
                {
                    continue;
                }
                by_source
                    .entry(in_edge.from.to_string())
                    .or_insert_with(|| (in_edge.from.clone(), BTreeSet::new()))
                    .1
                    .insert(in_edge.rel_type.clone());
            }
        }
        if !by_source.is_empty() {
            let referrers: Vec<memstead_base::ReferrerInfo> = by_source
                .into_values()
                .map(|(from, rel_types)| memstead_base::ReferrerInfo {
                    from_id: from.to_string(),
                    rel_types: rel_types.into_iter().collect(),
                    mem: from.mem().to_string(),
                })
                .collect();
            return Err(CliError::from_engine_op(
                memstead_base::EngineError::MemHasIncomingRefs {
                    mem: args.name,
                    referrers,
                },
            )
            .into());
        }
    }

    engine
        .unregister_read_mount(&args.name)
        .map_err(|e| anyhow::Error::from(CliError::from_engine_op(e)))?;
    engine
        .persist_state()
        .map_err(|e| anyhow::Error::from(CliError::from_engine_op(e)))?;

    if ctx.json {
        print_json(&json!({
            "mem_name": args.name,
            "unregistered": true,
            "cache_retained": true,
        }))?;
    } else {
        print_markdown(&format!(
            "# Uninstalled `{}`\n\n- Mount: unregistered from the workspace\n- Cache: \
             archive copy retained (shared across workspaces; re-`install` re-registers it)",
            args.name,
        ));
    }
    Ok(())
}
