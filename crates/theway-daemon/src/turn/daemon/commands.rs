// `WireCommand` dispatch and the per-command `TurnHost` handlers.
//
// This file is an `include!` fragment of the `turn::daemon` module, like its
// siblings `input.rs`, `queue.rs`, `runtime.rs`, `snapshot.rs`, `state.rs`, and
// `extensions.rs`. Every handler therefore stays a private `TurnHost` method of
// `turn::daemon`, which is what the mirrored unit suites under
// `tests/turn/daemon/` reach through the `turn/daemon` bridge in `daemon.rs`
// (`super` = `crate::turn::daemon`). Domain submodules live under
// `src/turn/daemon/commands/`; keeping them as `include!` fragments rather than
// `mod` declarations is required because `daemon` already binds the name
// `commands` to `crate::commands` (see `daemon.rs`).

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/dispatch.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/session_lifecycle.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/credentials.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/configure.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/skill_dirs.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/triggers.rs"
));
