// @theway-ai/sdk — typed TypeScript SDK for the theway gRPC daemon.
//
// Generated messages/services (ts-proto, client-only) plus the high-level
// typed client wrappers. The proto sources are shipped under proto/ and the
// generation script lives in scripts/generate.sh.

export { ThewayGrpcClient } from './client.js';
export { HealthClient } from './health.js';

export * from './generated/theway_grpc.js';
export {
  HealthCheckResponse_ServingStatus,
  healthCheckResponse_ServingStatusFromJSON,
  healthCheckResponse_ServingStatusToJSON,
} from './generated/health.js';
export type { HealthCheckRequest, HealthCheckResponse } from './generated/health.js';
