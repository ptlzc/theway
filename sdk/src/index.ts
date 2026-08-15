// @theway-ai/sdk — typed TypeScript SDK for the theway gRPC daemon.
//
// Generated messages/services (ts-proto, client-only) plus the high-level
// typed client wrappers. The proto sources are shipped under proto/ and the
// generation script lives in scripts/generate.sh.

export { ThewayGrpcClient } from './client.js';
export { HealthClient } from './health.js';

// Re-export the whole generated theway.grpc.v1 package. Each generated file
// repeats the shared codec helper types and `protobufPackage`; pin those names
// to common.ts first so the star re-exports below remain unambiguous.
export { protobufPackage } from './generated/common.js';
export type { DeepPartial, Exact, MessageFns } from './generated/common.js';

export * from './generated/common.js';
export * from './generated/cron.js';
export * from './generated/events.js';
export * from './generated/feed.js';
export * from './generated/graph.js';
export * from './generated/graph_checkpoint.js';
export * from './generated/graph_control.js';
export * from './generated/graph_events.js';
export * from './generated/graph_output.js';
export * from './generated/session.js';
export * from './generated/session_status.js';
export * from './generated/sidebar.js';
export * from './generated/sidebar_tools.js';
export * from './generated/skills.js';
export * from './generated/state.js';
export * from './generated/subagent.js';
export * from './generated/subagent_events.js';
export * from './generated/theway_grpc.js';
export * from './generated/triggers.js';
export {
  HealthCheckResponse_ServingStatus,
  healthCheckResponse_ServingStatusFromJSON,
  healthCheckResponse_ServingStatusToJSON,
} from './generated/health.js';
export type { HealthCheckRequest, HealthCheckResponse } from './generated/health.js';
