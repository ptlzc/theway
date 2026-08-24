//! Application orchestration for daemon process and session-scoped runtimes.
//!
//! This layer owns construction order and service lifetimes. Reusable runtime policy
//! remains in `theway-core`; host effects remain in daemon adapters.

mod services;
pub mod session;
mod startup;

pub use services::DaemonServices;
pub use session::{SessionExecutionContext, SessionRuntime, SessionRuntimeBuilder};
pub use startup::{DaemonOptions, DaemonTransport, SessionSelection, run};
