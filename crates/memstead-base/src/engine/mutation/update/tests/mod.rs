//! Tests of the update path, cut by concern: the batch and the basic
//! verbs, section modes and open fences, declared relations and the
//! no-op family, alias synthesis and its GC, and repair, anchors and
//! reserved keys.

use crate::backend::MemBackend;
use crate::engine::test_helpers::*;
use crate::engine::{CreateEntityArgs, Engine, EngineError, RelateEntityArgs, UpdateEntityArgs};
use crate::entity::EntityId;
use crate::storage::{ArchiveBackend, FilesystemBackend};
use crate::vcs::Actor;
use indexmap::IndexMap;
use tempfile::TempDir;

mod batch_and_basics;
mod relations_and_noops;
mod repair_anchors_reserved;
mod sections_and_fences;
mod synthesis_gc;
