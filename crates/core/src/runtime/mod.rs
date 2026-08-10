//! Opinionated assembly around the bare `Agent`. 1:1 mirror of
//! `packages/agent/src/harness/`. The single-agent runtime lives in
//! [`agent`] (harness, session, skills, compaction, permission, …); everything
//! above it — spawning nested agent runs, the job registry, the DAG/goal graph
//! engine — lives in [`multiagent`]. Dependency direction: `multiagent` uses
//! `agent`'s public API, never the reverse.

pub mod agent;
pub mod multiagent;
