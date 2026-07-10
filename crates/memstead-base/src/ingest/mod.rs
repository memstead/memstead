//! The deterministic ingest-orchestration surface.
//!
//! Ingest today has two implementations that have drifted apart: a full
//! orchestration (selection, backoff, change-detection, brief assembly,
//! process-state lifecycle) living in the Claude-Code plugin's Node.js, and
//! a hard-coded natural-language charge in the macOS app that uses none of
//! it. This module is the engine-side home the deterministic half is being
//! ported into, so that the plugin skill and the macOS app both reduce to
//! thin clients pulling the *same* rendered run-brief from the engine —
//! honouring "every engine-reachable operation is reachable over UniFFI
//! *and* CLI" and "the engine owns mem-repo state".
//!
//! The boundary is load-bearing: **deterministic → engine** (selection,
//! backoff, change-detection, brief assembly, cursor/process-state);
//! **LLM-inherent → agent** (reading sources, mutating the graph). The
//! engine renders the brief and advances deterministic state — it never
//! hosts the agent.
//!
//! The deterministic half is ported and wired: the structural [`resolve`]
//! layer (joins an ingest config to its projection, facets, and mediums), the
//! [`change_detection`] primitives, the [`cursor`] driver that assembles a
//! per-source changed slice across the git / graph / mtime strategies, and
//! [`brief`] assembly — all reachable through `memstead ingest brief` (CLI)
//! and the UniFFI surface, which the plugin skill and macOS app consume as
//! thin clients.

pub mod brief;
pub mod change_detection;
pub mod cursor;
pub mod guidance;
pub mod refinement;
pub mod render;
pub mod resolve;
pub mod selection;
pub mod slice;

pub use brief::{
    NoSignalNote, PROCESS_MEM_SCHEMA, ProcessMemInfo, SourceCursor, SyncCommand,
    assemble_discovery_brief, assemble_one_shot_brief, assemble_refinement_brief,
    render_changed_slice, render_goal_and_avoid, render_intent, render_one_shot_lens,
    render_operative_data, render_situation,
};
pub use change_detection::{
    Digest, StatDiff, StatEntry, StatMap, compute_stat_map, diff_stat_maps, digest_stat_map,
    digests_equal, parse_digest_token, serialize_digest_token,
};
pub use cursor::{compute_source_cursor, enumerate_facet_files, source_moved};
pub use guidance::{
    GuidanceDefaults, MemGuidance, ResolvedGuidance, merge_guidance_block, resolve_writing_guidance,
};
pub use refinement::{
    Batch, next_batch, read_pending_findings, render_refinement_scout, render_refinement_writer,
};
pub use render::{RenderBriefError, mode_name, render_ingest_brief};
pub use resolve::{
    ChangeStrategy, ResolveError, ResolvedIngest, ResolvedPrimarySource, ResolvedSource,
    find_git_root, resolve_change_strategy, resolve_ingest,
};
pub use selection::{BackoffEntry, MAX_SKIP_LEVEL, apply_backoff, select_next_due, should_skip};
pub use slice::{
    Slice, SliceOutcome, graph_changes_to_slice, graph_slice_outcome, is_git_token,
    mtime_slice_outcome,
};
