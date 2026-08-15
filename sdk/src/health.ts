import * as grpc from '@grpc/grpc-js';
import {
  HealthClient as HealthClientCtor,
  HealthCheckResponse_ServingStatus,
} from './generated/health.js';
import type {
  HealthCheckRequest,
  HealthCheckResponse,
  HealthClient as HealthClientApi,
} from './generated/health.js';

/**
 * Typed client for the standard gRPC health protocol (grpc.health.v1.Health)
 * served alongside theway.grpc.v1 — usable by generic probes and by the
 * ThewayDaemon liveness check alike.
 */
export class HealthClient {
  readonly #client: HealthClientApi;

  constructor(
    authority: string,
    credentials: grpc.ChannelCredentials = grpc.credentials.createInsecure(),
  ) {
    this.#client = new HealthClientCtor(authority, credentials);
  }

  /**
   * `Check` — one-shot health probe. Resolves `true` only when the service
   * reports SERVING (empty service name = the server as a whole).
   */
  check(service = ''): Promise<boolean> {
    const request: HealthCheckRequest = { service };
    return new Promise<boolean>((resolve, reject) => {
      this.#client.check(
        request,
        (err: grpc.ServiceError | null, response: HealthCheckResponse) => {
          if (err) {
            reject(
              new Error(`theway grpc: Health/Check failed: ${err.message}`, { cause: err }),
            );
            return;
          }
          resolve(response.status === HealthCheckResponse_ServingStatus.SERVING);
        },
      );
    });
  }

  /** `Watch` — server-streaming status updates. Resolves when the stream ends. */
  watch(
    service: string,
    onStatus: (serving: boolean) => void,
    signal?: AbortSignal,
  ): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      let stream: grpc.ClientReadableStream<HealthCheckResponse>;
      const forwardAbort = (): void => {
        stream.destroy();
      };
      signal?.addEventListener('abort', forwardAbort, { once: true });

      const request: HealthCheckRequest = { service };
      stream = this.#client.watch(request);

      stream.on('data', (response: HealthCheckResponse) => {
        onStatus(response.status === HealthCheckResponse_ServingStatus.SERVING);
      });
      stream.on('end', () => {
        signal?.removeEventListener('abort', forwardAbort);
        resolve();
      });
      stream.on('error', (err: grpc.ServiceError) => {
        signal?.removeEventListener('abort', forwardAbort);
        if (signal?.aborted) {
          resolve();
        } else {
          reject(new Error(`theway grpc: Health/Watch failed: ${err.message}`, { cause: err }));
        }
      });
    });
  }

  /** Close the underlying channel. */
  close(): void {
    this.#client.close();
  }
}
