//! Compaction utilities. Currently a thin re-export — the meaningful helpers
//! (`serialize_conversation`, `estimate_tokens`) live in `compaction.rs` alongside their
//! primary callers. Mirrors the TS module which is mostly a barrel.

pub use super::compaction::{
    ContextUsageEstimate, CutPointResult, estimate_context_tokens, estimate_tokens, find_cut_point,
    find_turn_start_index, serialize_conversation,
};
