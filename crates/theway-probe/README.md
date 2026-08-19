# theway-probe

`theway-probe` is a standalone gRPC serviceability client for an already-running `thewayd`. It checks the standard gRPC health endpoint, the health watch stream, multi-session create/list behavior, and session-state retrieval without linking the daemon or transport crates.

The binary compiles its tonic client directly from the transport-owned protobuf definitions, including [`health.proto`](../theway-transport/proto/health.proto), in [`build.rs`](build.rs). This keeps the probe independent while testing the same protocol exposed by the daemon.

## Usage

```bash
cargo run -p theway-probe -- \
  --server-addr http://127.0.0.1:9091 \
  --tests all
```

`--tests` accepts `all` or a comma-separated subset of `health-check`, `health-watch`, `multi-session`, and `get-state`. The process prints a pass/fail summary and exits non-zero if any selected check fails. `--output-dir <path>` additionally writes one JSON result file per check.

The probe does not start or stop the daemon and does not call a live LLM provider.

## Documentation

- [Probe architecture](docs/architecture.md)
- [Transport protocol architecture](../theway-transport/docs/architecture.md)

## Validation

```bash
cargo check -p theway-probe
cargo doc -p theway-probe --no-deps --document-private-items
```
