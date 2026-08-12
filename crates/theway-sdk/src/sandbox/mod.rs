//! Sandboxed-execution surface of the SDK. Currently a stub: the sandbox
//! executor returns unsupported errors (no e2b / remote sandbox in this
//! change) but provides the real seam the daemon's tool assembly switches on.

pub mod executor;
