// @theway-ai/sdk — typed TypeScript SDK for the theway gRPC daemon.
//
// Generated messages/services (ts-proto, client-only) plus the high-level
// typed client wrappers. The proto sources are shipped under proto/ and the
// generation script lives in scripts/generate.sh.

export { ThewayGrpcClient } from './client.js';
export { HealthClient } from './health.js';

// Re-export the whole generated theway.grpc.v1 package. Each generated file
// repeats the shared codec helper types and `protobufPackage`; pin those names
// to commands.ts first so the star re-exports below remain unambiguous.
export { protobufPackage } from './generated/commands.js';
export type { DeepPartial, Exact, MessageFns } from './generated/commands.js';

export * from './generated/commands.js';
export * from './generated/events.js';
export * from './generated/extensions.js';
export * from './generated/graph_engine.js';
export * from './generated/session.js';
export * from './generated/settings.js';
export * from './generated/tools.js';
export * from './generated/state.js';
export {
  HealthCheckResponse_ServingStatus,
  healthCheckResponse_ServingStatusFromJSON,
  healthCheckResponse_ServingStatusToJSON,
} from './generated/health.js';
export type { HealthCheckRequest, HealthCheckResponse } from './generated/health.js';
