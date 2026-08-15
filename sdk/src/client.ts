import * as grpc from '@grpc/grpc-js';
import {
  ThewayGrpcClient as ThewayGrpcClientCtor,
  type ThewayGrpcClient as ThewayGrpcClientApi,
} from './generated/theway_grpc.js';
import {
  MessageMode,
  type ApproveRequest,
  type CommandResult,
  type Empty,
  type SendMessageRequest,
  type SetModelRequest,
} from './generated/common.js';
import type { SessionState } from './generated/state.js';
import type { StreamFrame } from './generated/events.js';
import type {
  GraphCancelRequest,
  GraphNodeInterruptRequest,
  GraphNodeSteerRequest,
  GraphRetryRequest,
  GraphRetryResponse,
  GraphSkipRequest,
  GraphSkipResponse,
} from './generated/graph_control.js';
import type {
  GraphCheckpointRequest,
  GraphCheckpointResponse,
  GraphRestoreRequest,
  GraphRestoreResponse,
} from './generated/graph_checkpoint.js';
import type {
  GetNodeOutputRequest,
  GetNodeOutputResponse,
  GraphListRequest,
  GraphListResponse,
} from './generated/graph_output.js';
import type {
  CreateSessionRequest,
  CreateSessionResponse,
  DeleteSessionRequest,
  DeleteSessionResponse,
  ListSessionsResponse,
  RenameSessionRequest,
  SwitchSessionRequest,
} from './generated/session.js';

// ── authority cleaning ──

const SCHEME_PREFIX = /^https?:\/\//;
const TRAILING_SLASHES = /\/+$/;

/**
 * Typed gRPC client for the `theway --grpc` loopback server
 * (theway.grpc.v1.ThewayGrpc). Generated from proto/theway_grpc.proto and its
 * domain imports (ts-proto + @grpc/grpc-js) — no runtime proto loading.
 *
 * Command RPCs (SendMessage / SetModel / …) resolve the raw CommandResult;
 * the convenience wrappers (`prompt`, `setModelSpec`, `abort`, …) throw when
 * the server replies `accepted: false`, mirroring the caller-side contract of
 * the JSON channels.
 */
export class ThewayGrpcClient {
  readonly #client: ThewayGrpcClientApi;

  constructor(
    baseUrl: string,
    credentials: grpc.ChannelCredentials = grpc.credentials.createInsecure(),
  ) {
    const authority = baseUrl.replace(SCHEME_PREFIX, '').replace(TRAILING_SLASHES, '');
    this.#client = new ThewayGrpcClientCtor(authority, credentials);
  }

  // ── session state ──

  /** `GetState` — the latest full session state. */
  getState(): Promise<SessionState> {
    return this.#call<Empty, SessionState>(this.#client.getState, 'GetState', {});
  }

  /** `GetNodeOutput` — a DAG node's output fragment from an offset. */
  getNodeOutput(request: GetNodeOutputRequest): Promise<GetNodeOutputResponse> {
    return this.#call<GetNodeOutputRequest, GetNodeOutputResponse>(this.#client.getNodeOutput, 'GetNodeOutput', request);
  }

  // ── commands ──

  /** `SendMessage` — submit a user prompt (mode=GUIDE). */
  async prompt(text: string, images: SendMessageRequest['images'] = []): Promise<void> {
    const request: SendMessageRequest = { text, images, mode: MessageMode.GUIDE };
    const result = await this.sendMessage(request);
    if (!result.accepted) {
      throw new Error('theway grpc: SendMessage was not accepted by the server');
    }
  }

  /** `SendMessage` — raw request (mode=INTERRUPT supported). */
  sendMessage(request: SendMessageRequest): Promise<CommandResult> {
    return this.#call<SendMessageRequest, CommandResult>(this.#client.sendMessage, 'SendMessage', request);
  }

  /** `SetModel` — switch model with a composed `provider:model` spec. */
  async setModelSpec(spec: string): Promise<void> {
    const request: SetModelRequest = { spec };
    const result = await this.#call<SetModelRequest, CommandResult>(
      this.#client.setModel,
      'SetModel',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: SetModel was not accepted by the server');
    }
  }

  /** `SetModel` with `provider` + `model` (composed into `provider:model`). */
  setModel(provider: string, model: string): Promise<void> {
    return this.setModelSpec(`${provider}:${model}`);
  }

  /** `Cancel` — stop the in-flight turn (same as local Ctrl-C). */
  async abort(): Promise<void> {
    const result = await this.#call<Empty, CommandResult>(this.#client.cancel, 'Cancel', {});
    if (!result.accepted) {
      throw new Error('theway grpc: Cancel was not accepted by the server');
    }
  }

  /** `Approve` — resolve the pending control-plane approval (approve / reject). */
  async resolveControlPlane(approve: boolean): Promise<void> {
    const request: ApproveRequest = { approve };
    const result = await this.#call<ApproveRequest, CommandResult>(
      this.#client.approve,
      'Approve',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: Approve was not accepted by the server');
    }
  }

  // ── graph control (DAG + goal runs) ──

  /** `GraphCancel` — cancel a run. */
  async graphCancel(runId: string): Promise<void> {
    const request: GraphCancelRequest = { runId };
    const result = await this.#call<GraphCancelRequest, CommandResult>(
      this.#client.graphCancel,
      'GraphCancel',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: GraphCancel was not accepted by the server');
    }
  }

  /** `GraphRetry` — re-run failed/cancelled nodes (omitted nodeId = all). */
  graphRetry(runId: string, nodeId?: string): Promise<GraphRetryResponse> {
    const request: GraphRetryRequest = { runId, nodeId };
    return this.#call<GraphRetryRequest, GraphRetryResponse>(this.#client.graphRetry, 'GraphRetry', request);
  }

  /** `GraphSkip` — skip a node (counts as success for downstream). */
  graphSkip(runId: string, nodeId: string): Promise<GraphSkipResponse> {
    const request: GraphSkipRequest = { runId, nodeId };
    return this.#call<GraphSkipRequest, GraphSkipResponse>(this.#client.graphSkip, 'GraphSkip', request);
  }

  /** `GraphNodeInterrupt` — stop the node's in-flight turn. */
  async graphNodeInterrupt(runId: string, nodeId: string): Promise<void> {
    const request: GraphNodeInterruptRequest = { runId, nodeId };
    const result = await this.#call<GraphNodeInterruptRequest, CommandResult>(
      this.#client.graphNodeInterrupt,
      'GraphNodeInterrupt',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: GraphNodeInterrupt was not accepted by the server');
    }
  }

  /** `GraphNodeSteer` — queue a steering message at the node's next turn boundary. */
  async graphNodeSteer(runId: string, nodeId: string, text: string): Promise<void> {
    const request: GraphNodeSteerRequest = { runId, nodeId, text };
    const result = await this.#call<GraphNodeSteerRequest, CommandResult>(
      this.#client.graphNodeSteer,
      'GraphNodeSteer',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: GraphNodeSteer was not accepted by the server');
    }
  }

  /** `GraphCheckpoint` — export run snapshots as portable JSON. */
  graphCheckpoint(request: GraphCheckpointRequest = {}): Promise<GraphCheckpointResponse> {
    return this.#call<GraphCheckpointRequest, GraphCheckpointResponse>(this.#client.graphCheckpoint, 'GraphCheckpoint', request);
  }

  /** `GraphRestore` — revive a run from a GraphCheckpoint snapshot. */
  graphRestore(sessionId: string, snapshot: string): Promise<GraphRestoreResponse> {
    const request: GraphRestoreRequest = { sessionId, snapshot };
    return this.#call<GraphRestoreRequest, GraphRestoreResponse>(this.#client.graphRestore, 'GraphRestore', request);
  }

  /** `GraphList` — enumerate one session's graph runs. */
  graphList(sessionId: string): Promise<GraphListResponse> {
    const request: GraphListRequest = { sessionId };
    return this.#call<GraphListRequest, GraphListResponse>(this.#client.graphList, 'GraphList', request);
  }

  // ── session resources ──

  /** `ListSessions` — all sessions (live + archived) with the current one marked. */
  listSessions(): Promise<ListSessionsResponse> {
    return this.#call<Empty, ListSessionsResponse>(this.#client.listSessions, 'ListSessions', {});
  }

  /** `CreateSession` — create a session. */
  createSession(name?: string): Promise<CreateSessionResponse> {
    const request: CreateSessionRequest = { name };
    return this.#call<CreateSessionRequest, CreateSessionResponse>(this.#client.createSession, 'CreateSession', request);
  }

  /** `SwitchSession` — switch the live session. */
  async switchSession(sessionId: string): Promise<void> {
    const request: SwitchSessionRequest = { sessionId };
    const result = await this.#call<SwitchSessionRequest, CommandResult>(
      this.#client.switchSession,
      'SwitchSession',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: SwitchSession was not accepted by the server');
    }
  }

  /** `RenameSession` — rename a session. */
  async renameSession(sessionId: string, name: string): Promise<void> {
    const request: RenameSessionRequest = { sessionId, name };
    const result = await this.#call<RenameSessionRequest, CommandResult>(
      this.#client.renameSession,
      'RenameSession',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: RenameSession was not accepted by the server');
    }
  }

  /** `DeleteSession` — delete a session (refused while graphs are running). */
  deleteSession(sessionId: string): Promise<DeleteSessionResponse> {
    const request: DeleteSessionRequest = { sessionId };
    return this.#call<DeleteSessionRequest, DeleteSessionResponse>(this.#client.deleteSession, 'DeleteSession', request);
  }

  // ── event stream ──

  /**
   * `StreamEvents` — server-streaming snapshot/event frames. Every frame
   * (snapshot and event) is delivered to `onFrame`. Resolves when the stream
   * ends normally; rejects on transport errors or when the client is closed.
   */
  streamEvents(onFrame: (frame: StreamFrame) => void, signal?: AbortSignal): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const forwardAbort = (): void => {
        this.#closed = true;
        // Cancel the stream by ending the call
      };
      signal?.addEventListener('abort', forwardAbort, { once: true });

      const stream = this.#client.streamEvents({});

      stream.on('data', (frame: StreamFrame) => {
        try {
          onFrame(frame);
        } catch (err) {
          signal?.removeEventListener('abort', forwardAbort);
          stream.destroy();
          reject(
            new Error(
              `theway grpc: StreamEvents handler threw: ${extractErrorMessage(err)}`,
              { cause: err },
            ),
          );
        }
      });

      stream.on('end', () => {
        signal?.removeEventListener('abort', forwardAbort);
        resolve();
      });

      stream.on('error', (err: grpc.ServiceError) => {
        signal?.removeEventListener('abort', forwardAbort);
        if (this.#closed || signal?.aborted) {
          reject(new Error('theway grpc: StreamEvents aborted by close()'));
        } else {
          reject(this.#mapError('StreamEvents', err));
        }
      });
    });
  }

  /** Close the underlying channel; in-flight streams fail fast. */
  close(): void {
    this.#closed = true;
    this.#client.close();
  }

  // ── private helpers ──

  #closed = false;

  #call<TReq, TResp>(
    invoke: (
      req: TReq,
      cb: (err: grpc.ServiceError | null, resp: TResp) => void,
    ) => grpc.ClientUnaryCall,
    method: string,
    request: TReq,
  ): Promise<TResp> {
    return new Promise<TResp>((resolve, reject) => {
      invoke.call(this.#client, request, (err, response) => {
        if (err) {
          reject(this.#mapError(method, err));
          return;
        }
        resolve(response);
      });
    });
  }

  #mapError(method: string, err: grpc.ServiceError): Error {
    return new Error(`theway grpc: ${method} failed: ${err.message}`, { cause: err });
  }
}

function extractErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}
