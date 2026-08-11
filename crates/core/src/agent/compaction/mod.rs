//! Auto-compaction + branch summarization. TODO: 1:1 port of
//! `packages/agent/src/harness/compaction/`. Pending until Agent + session land.

pub mod algorithm;
pub mod branch_summarization;
pub mod compaction;
pub mod triggers;
pub mod utils;

pub use algorithm::{
    BuiltinCompactAlgorithm, CompactAlgorithm, CompactAlgorithmRegistry, SummarizeRequest,
    SummaryOutcome,
};
pub use compaction::{CompactionSettings, DEFAULT_COMPACTION_SETTINGS};
