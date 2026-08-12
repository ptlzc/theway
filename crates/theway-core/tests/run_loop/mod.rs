//! End-to-end Agent loop test. Uses a synthetic `StreamFn` (via the AssistantMessageEventStream
//! sender API exposed by theway-llm-provider) to drive the loop deterministically — no LLM calls.

pub mod helpers;
pub mod lifecycle;
pub mod permission;
pub mod updates;
