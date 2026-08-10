//! The harness layer of the single-agent runtime (feature `harness`, opt-out for
//! embedders that only want the bare [`crate::agent`] state machine): the
//! [`agent_harness`] itself plus everything it assembles — sessions, skills,
//! compaction, permission policy, cost tracking, hooks, triggers.
//!
//! Dependency direction: this is the base of the runtime. Orchestration
//! (`crate::runtime::multiagent`) builds on its public API, never the reverse.

pub mod agent_harness;
pub mod compaction;
pub mod cost;
pub mod hooks;
pub mod messages;
pub mod notification_hook;
pub mod permission;
pub mod prompt_templates;
pub mod session;
pub mod skills;
pub mod system_prompt;
pub mod trigger;
pub mod trigger_runtime;
pub mod types;
pub mod utils;

#[cfg(feature = "native-env")]
pub mod env;
