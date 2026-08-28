import * as grpc from '@grpc/grpc-js';
import {
  CommandServiceClient as CommandServiceClientCtor,
  type CommandServiceClient as CommandServiceClientApi,
  MessageMode,
  type ApproveRequest,
  type CancelRequest,
  type CommandResult,
  type Empty,
  type SendMessageRequest,
  type SetModelRequest,
} from './generated/commands.js';
import {
  EventServiceClient as EventServiceClientCtor,
  type EventServiceClient as EventServiceClientApi,
  type StreamEventsRequest,
  type StreamFrame,
} from './generated/events.js';
import {
  ExtensionServiceClient as ExtensionServiceClientCtor,
  type ExtensionServiceClient as ExtensionServiceClientApi,
  type DecideExtensionTrustRequest,
  type DecideExtensionTrustResponse,
  type ExtensionCommandOutcome,
  type ExtensionSnapshot,
  type InvokeExtensionCommandRequest,
  type ReloadExtensionsRequest,
  type ReloadExtensionsResponse,
} from './generated/extensions.js';
import {
  GraphEngineServiceClient as GraphEngineServiceClientCtor,
  type GraphEngineServiceClient as GraphEngineServiceClientApi,
  type GetNodeOutputRequest,
  type GetNodeOutputResponse,
  type GraphCancelRequest,
  type GraphCheckpointRequest,
  type GraphCheckpointResponse,
  type GraphListRequest,
  type GraphListResponse,
  type GraphNodeInterruptRequest,
  type GraphNodeSteerRequest,
  type GraphRestoreRequest,
  type GraphRestoreResponse,
  type GraphRetryRequest,
  type GraphRetryResponse,
  type GraphSkipRequest,
  type GraphSkipResponse,
} from './generated/graph_engine.js';
import {
  SessionServiceClient as SessionServiceClientCtor,
  type SessionServiceClient as SessionServiceClientApi,
  type ActivateSessionRequest,
  type ActivateSessionResponse,
  type ClearCredentialRequest,
  type CreateSessionRequest,
  type CreateSessionResponse,
  type DeleteSessionRequest,
  type DeleteSessionResponse,
  type ListSessionsResponse,
  type RenameSessionRequest,
  type SessionState,
  type SessionStateRequest,
  type SetCredentialRequest,
  type UpdateSessionMetadataRequest,
} from './generated/session.js';
import {
  SettingsServiceClient as SettingsServiceClientCtor,
  type SettingsServiceClient as SettingsServiceClientApi,
  type DaemonConfig,
} from './generated/settings.js';
import {
  ToolServiceClient as ToolServiceClientCtor,
  type ToolServiceClient as ToolServiceClientApi,
  type EditFileRequest,
  type EditFileResponse,
  type ExecCommandRequest,
  type FindRequest,
  type FindResponse,
  type GrepRequest,
  type GrepResponse,
  type ListDirRequest,
  type ListDirResponse,
  type MemoryForgetRequest,
  type MemoryForgetResponse,
  type MemoryListRequest,
  type MemoryListResponse,
  type MemoryReadRequest,
  type MemoryReadResponse,
  type MemorySaveRequest,
  type MemorySaveResponse,
  type ReadFileRequest,
  type ReadFileResponse,
  type SkillInstallRequest,
  type SkillInstallResponse,
  type WriteFileRequest,
  type WriteFileResponse,
} from './generated/tools.js';
import {
  StorageServiceClient as StorageServiceClientCtor,
  type StorageServiceClient as StorageServiceClientApi,
  type LoadCronJobsRequest,
  type LoadCronJobsResponse,
  type LoadDagRunsRequest,
  type LoadDagRunsResponse,
  type LoadTriggerRulesRequest,
  type LoadTriggerRulesResponse,
  type SaveCronJobsRequest,
  type SaveCronJobsResponse,
  type SaveDagRunRequest,
  type SaveDagRunResponse,
  type SaveTriggerRulesRequest,
  type SaveTriggerRulesResponse,
} from './generated/state.js';

// ── authority cleaning ──

const SCHEME_PREFIX = /^https?:\/\//;
const TRAILING_SLASHES = /\/+$/;

/**
 * Typed gRPC client for the `theway --grpc` loopback server
 * (theway.grpc.v1.CommandService / SessionService / SettingsService /
 * ExtensionService / ToolService / StorageService / GraphEngineService /
 * EventService).
 * Generated from the domain proto files
 * and health.proto (ts-proto + @grpc/grpc-js) — no runtime proto loading.
 *
 * Command RPCs (SendMessage / SetModel / …) resolve the raw CommandResult;
 * the convenience wrappers (`prompt`, `setModelSpec`, `abort`, …) throw when
 * the server replies `accepted: false`, mirroring the caller-side contract of
 * the JSON channels.
 */
export class ThewayGrpcClient {
  readonly #session: SessionServiceClientApi;
  readonly #command: CommandServiceClientApi;
  readonly #extensions: ExtensionServiceClientApi;
  readonly #graph: GraphEngineServiceClientApi;
  readonly #events: EventServiceClientApi;
  readonly #settings: SettingsServiceClientApi;
  readonly #tools: ToolServiceClientApi;
  readonly #storage: StorageServiceClientApi;

  constructor(
    baseUrl: string,
    credentials: grpc.ChannelCredentials = grpc.credentials.createInsecure(),
  ) {
    const authority = baseUrl.replace(SCHEME_PREFIX, '').replace(TRAILING_SLASHES, '');
    this.#session = new SessionServiceClientCtor(authority, credentials);
    this.#command = new CommandServiceClientCtor(authority, credentials);
    this.#extensions = new ExtensionServiceClientCtor(authority, credentials);
    this.#graph = new GraphEngineServiceClientCtor(authority, credentials);
    this.#events = new EventServiceClientCtor(authority, credentials);
    this.#settings = new SettingsServiceClientCtor(authority, credentials);
    this.#tools = new ToolServiceClientCtor(authority, credentials);
    this.#storage = new StorageServiceClientCtor(authority, credentials);
  }

  // ── session state ──

  /** `GetState` — the latest full session state. Pass a session id or omit for the daemon's current session. */
  getState(sessionId = ''): Promise<SessionState> {
    const request: SessionStateRequest = { sessionId };
    return this.#call<SessionStateRequest, SessionState>(this.#session.getState.bind(this.#session), 'GetState', request);
  }

  /** `GetNodeOutput` — a DAG node's output fragment from an offset. */
  getNodeOutput(request: GetNodeOutputRequest): Promise<GetNodeOutputResponse> {
    return this.#call<GetNodeOutputRequest, GetNodeOutputResponse>(this.#graph.getNodeOutput.bind(this.#graph), 'GetNodeOutput', request);
  }

  // ── commands ──

  /** `SendMessage` — submit a user prompt (mode=QUEUE). */
  async prompt(text: string, images: SendMessageRequest['images'] = [], sessionId?: string): Promise<void> {
    const request: SendMessageRequest = { text, images, mode: MessageMode.QUEUE, sessionId };
    const result = await this.sendMessage(request);
    if (!result.accepted) {
      throw new Error('theway grpc: SendMessage was not accepted by the server');
    }
  }

  /** `SendMessage` — raw request (mode=INTERRUPT supported). */
  sendMessage(request: SendMessageRequest): Promise<CommandResult> {
    return this.#call<SendMessageRequest, CommandResult>(this.#command.sendMessage.bind(this.#command), 'SendMessage', request);
  }

  /** `SetModel` — switch model with a composed `provider:model` spec. */
  async setModelSpec(spec: string, sessionId = ''): Promise<void> {
    const request: SetModelRequest = { sessionId, spec };
    const result = await this.#call<SetModelRequest, CommandResult>(
      this.#command.setModel.bind(this.#command),
      'SetModel',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: SetModel was not accepted by the server');
    }
  }

  /** `SetModel` with `provider` + `model` (composed into `provider:model`). */
  setModel(provider: string, model: string, sessionId = ''): Promise<void> {
    return this.setModelSpec(`${provider}:${model}`, sessionId);
  }

  /** `Cancel` — stop the in-flight turn (same as local Ctrl-C). */
  async abort(sessionId = ''): Promise<void> {
    const request: CancelRequest = { sessionId };
    const result = await this.#call<CancelRequest, CommandResult>(this.#command.cancel.bind(this.#command), 'Cancel', request);
    if (!result.accepted) {
      throw new Error('theway grpc: Cancel was not accepted by the server');
    }
  }

  /** `Approve` — resolve the pending control-plane approval (approve / reject). */
  async resolveControlPlane(approve: boolean, sessionId = ''): Promise<void> {
    const request: ApproveRequest = { sessionId, approve };
    const result = await this.#call<ApproveRequest, CommandResult>(
      this.#command.approve.bind(this.#command),
      'Approve',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: Approve was not accepted by the server');
    }
  }

  // ── runtime extensions ──

  /** Structured extension catalog, redacted diagnostics and contributions. */
  getExtensions(): Promise<ExtensionSnapshot> {
    return this.#call<Empty, ExtensionSnapshot>(
      this.#extensions.getExtensions.bind(this.#extensions),
      'GetExtensions',
      {},
    );
  }

  /** Invoke a registered extension command in either interactive or headless mode. */
  invokeExtensionCommand(request: InvokeExtensionCommandRequest): Promise<ExtensionCommandOutcome> {
    return this.#call<InvokeExtensionCommandRequest, ExtensionCommandOutcome>(
      this.#extensions.invokeCommand.bind(this.#extensions),
      'InvokeExtensionCommand',
      request,
    );
  }

  /** Re-discover extensions; active work may return `status=pending`. */
  reloadExtensions(request: ReloadExtensionsRequest = { cancelActive: false }): Promise<ReloadExtensionsResponse> {
    return this.#call<ReloadExtensionsRequest, ReloadExtensionsResponse>(
      this.#extensions.reload.bind(this.#extensions),
      'ReloadExtensions',
      request,
    );
  }

  /** Persist a project or exact-package trust decision and request reload. */
  decideExtensionTrust(request: DecideExtensionTrustRequest): Promise<DecideExtensionTrustResponse> {
    return this.#call<DecideExtensionTrustRequest, DecideExtensionTrustResponse>(
      this.#extensions.decideTrust.bind(this.#extensions),
      'DecideExtensionTrust',
      request,
    );
  }

  // ── graph control (DAG + goal runs) ──

  /** `GraphCancel` — cancel a run. */
  async graphCancel(sessionId: string, runId: string): Promise<void> {
    const request: GraphCancelRequest = { sessionId, runId };
    const result = await this.#call<GraphCancelRequest, CommandResult>(
      this.#graph.graphCancel.bind(this.#graph),
      'GraphCancel',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: GraphCancel was not accepted by the server');
    }
  }

  /** `GraphRetry` — re-run failed/cancelled nodes (omitted nodeId = all). */
  graphRetry(sessionId: string, runId: string, nodeId?: string): Promise<GraphRetryResponse> {
    const request: GraphRetryRequest = { sessionId, runId, nodeId };
    return this.#call<GraphRetryRequest, GraphRetryResponse>(this.#graph.graphRetry.bind(this.#graph), 'GraphRetry', request);
  }

  /** `GraphSkip` — skip a node (counts as success for downstream). */
  graphSkip(sessionId: string, runId: string, nodeId: string): Promise<GraphSkipResponse> {
    const request: GraphSkipRequest = { sessionId, runId, nodeId };
    return this.#call<GraphSkipRequest, GraphSkipResponse>(this.#graph.graphSkip.bind(this.#graph), 'GraphSkip', request);
  }

  /** `GraphNodeInterrupt` — stop the node's in-flight turn. */
  async graphNodeInterrupt(sessionId: string, runId: string, nodeId: string): Promise<void> {
    const request: GraphNodeInterruptRequest = { sessionId, runId, nodeId };
    const result = await this.#call<GraphNodeInterruptRequest, CommandResult>(
      this.#graph.graphNodeInterrupt.bind(this.#graph),
      'GraphNodeInterrupt',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: GraphNodeInterrupt was not accepted by the server');
    }
  }

  /** `GraphNodeSteer` — queue a steering message at the node's next turn boundary. */
  async graphNodeSteer(sessionId: string, runId: string, nodeId: string, text: string): Promise<void> {
    const request: GraphNodeSteerRequest = { sessionId, runId, nodeId, text };
    const result = await this.#call<GraphNodeSteerRequest, CommandResult>(
      this.#graph.graphNodeSteer.bind(this.#graph),
      'GraphNodeSteer',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: GraphNodeSteer was not accepted by the server');
    }
  }

  /** `GraphCheckpoint` — export run snapshots as portable JSON. */
  graphCheckpoint(request: GraphCheckpointRequest = {}): Promise<GraphCheckpointResponse> {
    return this.#call<GraphCheckpointRequest, GraphCheckpointResponse>(this.#graph.graphCheckpoint.bind(this.#graph), 'GraphCheckpoint', request);
  }

  /** `GraphRestore` — revive a run from a GraphCheckpoint snapshot. */
  graphRestore(sessionId: string, snapshot: string): Promise<GraphRestoreResponse> {
    const request: GraphRestoreRequest = { sessionId, snapshot };
    return this.#call<GraphRestoreRequest, GraphRestoreResponse>(this.#graph.graphRestore.bind(this.#graph), 'GraphRestore', request);
  }

  /** `GraphList` — enumerate one session's graph runs. */
  graphList(sessionId: string): Promise<GraphListResponse> {
    const request: GraphListRequest = { sessionId };
    return this.#call<GraphListRequest, GraphListResponse>(this.#graph.graphList.bind(this.#graph), 'GraphList', request);
  }

  // ── session resources ──

  /** `ListSessions` — all sessions (live + archived) with the current one marked. */
  listSessions(): Promise<ListSessionsResponse> {
    return this.#call<Empty, ListSessionsResponse>(this.#session.listSessions.bind(this.#session), 'ListSessions', {});
  }

  /** `CreateSession` — create a session. */
  createSession(name?: string, sessionId?: string, metadata: CreateSessionRequest['metadata'] = {}): Promise<CreateSessionResponse> {
    const request: CreateSessionRequest = { name, sessionId, metadata };
    return this.#call<CreateSessionRequest, CreateSessionResponse>(this.#session.createSession.bind(this.#session), 'CreateSession', request);
  }

  /** `UpdateSessionMetadata` — update opaque session metadata KV. */
  async updateSessionMetadata(sessionId: string, metadata: UpdateSessionMetadataRequest['metadata']): Promise<void> {
    const request: UpdateSessionMetadataRequest = { sessionId, metadata };
    const result = await this.#call<UpdateSessionMetadataRequest, CommandResult>(
      this.#session.updateSessionMetadata.bind(this.#session),
      'UpdateSessionMetadata',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: UpdateSessionMetadata was not accepted by the server');
    }
  }

  /** `RenameSession` — rename a session. */
  async renameSession(sessionId: string, name: string): Promise<void> {
    const request: RenameSessionRequest = { sessionId, name };
    const result = await this.#call<RenameSessionRequest, CommandResult>(
      this.#session.renameSession.bind(this.#session),
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
    return this.#call<DeleteSessionRequest, DeleteSessionResponse>(this.#session.deleteSession.bind(this.#session), 'DeleteSession', request);
  }

  // ── session activation and credentials (issue #26) ──

  /**
   * `ActivateSession` — atomically create or resume a client-bound session.
   * The server applies the runtime before replying, so a successful response
   * means the daemon is ready to operate in `runtime.workDir`.
   */
  activateSession(request: ActivateSessionRequest): Promise<ActivateSessionResponse> {
    return this.#call<ActivateSessionRequest, ActivateSessionResponse>(this.#session.activateSession.bind(this.#session), 'ActivateSession', request);
  }

  /**
   * `SetCredential` — install a memory-only provider secret for a session.
   * Secrets are never persisted and never appear in responses or events.
   */
  async setCredential(sessionId: string, provider: string, secret: Uint8Array | string): Promise<void> {
    const request: SetCredentialRequest = {
      sessionId,
      provider,
      secret: typeof secret === 'string' ? new TextEncoder().encode(secret) : secret,
    };
    const result = await this.#call<SetCredentialRequest, CommandResult>(
      this.#session.setCredential.bind(this.#session),
      'SetCredential',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: SetCredential was not accepted by the server');
    }
  }

  /**
   * `ClearCredential` — remove a session credential. With no provider, clears
   * every provider secret held for the session.
   */
  async clearCredential(sessionId: string, provider?: string): Promise<void> {
    const request: ClearCredentialRequest = { sessionId, provider };
    const result = await this.#call<ClearCredentialRequest, CommandResult>(
      this.#session.clearCredential.bind(this.#session),
      'ClearCredential',
      request,
    );
    if (!result.accepted) {
      throw new Error('theway grpc: ClearCredential was not accepted by the server');
    }
  }

  // ── settings / config ──

  /** `GetConfig` — the daemon's current configuration view. */
  getConfig(): Promise<DaemonConfig> {
    return this.#call<Empty, DaemonConfig>(this.#settings.getConfig.bind(this.#settings), 'GetConfig', {});
  }

  /**
   * `SetConfig` — push a partial configuration update. Absent fields keep
   * the daemon's current value (repeated fields default to empty = no-op).
   */
  async setConfig(config: Partial<DaemonConfig>): Promise<void> {
    const result = await this.#call<DaemonConfig, CommandResult>(
      this.#settings.setConfig.bind(this.#settings),
      'SetConfig',
      fillDaemonConfig(config),
    );
    if (!result.accepted) {
      throw new Error('theway grpc: SetConfig was not accepted by the server');
    }
  }

  /** `Configure` — alias of `setConfig` (JSON-RPC / WS / MCP verb alignment). */
  async configure(config: Partial<DaemonConfig>): Promise<void> {
    const result = await this.#call<DaemonConfig, CommandResult>(
      this.#settings.configure.bind(this.#settings),
      'Configure',
      fillDaemonConfig(config),
    );
    if (!result.accepted) {
      throw new Error('theway grpc: Configure was not accepted by the server');
    }
  }

  // ── tool operations ──

  /** `ReadFile` — read a file with line pagination. */
  toolRead(request: ReadFileRequest): Promise<ReadFileResponse> {
    return this.#call<ReadFileRequest, ReadFileResponse>(this.#tools.readFile.bind(this.#tools), 'ReadFile', request);
  }

  /** `WriteFile` — write a file. */
  toolWrite(request: WriteFileRequest): Promise<WriteFileResponse> {
    return this.#call<WriteFileRequest, WriteFileResponse>(this.#tools.writeFile.bind(this.#tools), 'WriteFile', request);
  }

  /** `EditFile` — search-and-replace edit. */
  toolEdit(request: EditFileRequest): Promise<EditFileResponse> {
    return this.#call<EditFileRequest, EditFileResponse>(this.#tools.editFile.bind(this.#tools), 'EditFile', request);
  }

  /** `ExecCommand` — streaming shell execution. */
  toolExec(request: ExecCommandRequest) {
    return this.#tools.execCommand(request);
  }

  /** `ListDir` — list one directory level. */
  toolListDir(request: ListDirRequest): Promise<ListDirResponse> {
    return this.#call<ListDirRequest, ListDirResponse>(this.#tools.listDir.bind(this.#tools), 'ListDir', request);
  }

  /** `Grep` — regex content search. */
  toolGrep(request: GrepRequest): Promise<GrepResponse> {
    return this.#call<GrepRequest, GrepResponse>(this.#tools.grep.bind(this.#tools), 'Grep', request);
  }

  /** `Find` — filename-glob search. */
  toolFind(request: FindRequest): Promise<FindResponse> {
    return this.#call<FindRequest, FindResponse>(this.#tools.find.bind(this.#tools), 'Find', request);
  }

  /** `MemorySave` — save a memory entry. */
  toolMemorySave(request: MemorySaveRequest): Promise<MemorySaveResponse> {
    return this.#call<MemorySaveRequest, MemorySaveResponse>(this.#tools.memorySave.bind(this.#tools), 'MemorySave', request);
  }

  /** `MemoryList` — list memory entries. */
  toolMemoryList(request: MemoryListRequest): Promise<MemoryListResponse> {
    return this.#call<MemoryListRequest, MemoryListResponse>(this.#tools.memoryList.bind(this.#tools), 'MemoryList', request);
  }

  /** `MemoryRead` — read one memory entry. */
  toolMemoryRead(request: MemoryReadRequest): Promise<MemoryReadResponse> {
    return this.#call<MemoryReadRequest, MemoryReadResponse>(this.#tools.memoryRead.bind(this.#tools), 'MemoryRead', request);
  }

  /** `MemoryForget` — forget a memory entry. */
  toolMemoryForget(request: MemoryForgetRequest): Promise<MemoryForgetResponse> {
    return this.#call<MemoryForgetRequest, MemoryForgetResponse>(this.#tools.memoryForget.bind(this.#tools), 'MemoryForget', request);
  }

  /** `SkillInstall` — two-phase skill install. */
  toolSkillInstall(request: SkillInstallRequest): Promise<SkillInstallResponse> {
    return this.#call<SkillInstallRequest, SkillInstallResponse>(this.#tools.skillInstall.bind(this.#tools), 'SkillInstall', request);
  }

  // ── runtime state storage ──

  /** `StorageService.ListSessions` — session list from external storage. */
  stateListSessions(request: Empty): Promise<import('./generated/session.js').ListSessionsResponse> {
    return this.#call<Empty, import('./generated/session.js').ListSessionsResponse>(this.#storage.listSessions.bind(this.#storage), 'ListSessions', request);
  }

  /** `StorageService.CreateSession` — create a session in external storage. */
  stateCreateSession(request: CreateSessionRequest): Promise<CreateSessionResponse> {
    return this.#call<CreateSessionRequest, CreateSessionResponse>(this.#storage.createSession.bind(this.#storage), 'CreateSession', request);
  }

  /** `StorageService.RenameSession` — rename a session in external storage. */
  stateRenameSession(request: RenameSessionRequest): Promise<CommandResult> {
    return this.#call<RenameSessionRequest, CommandResult>(this.#storage.renameSession.bind(this.#storage), 'RenameSession', request);
  }

  /** `StorageService.DeleteSession` — delete a session in external storage. */
  stateDeleteSession(request: DeleteSessionRequest): Promise<DeleteSessionResponse> {
    return this.#call<DeleteSessionRequest, DeleteSessionResponse>(this.#storage.deleteSession.bind(this.#storage), 'DeleteSession', request);
  }

  /** `StorageService.SaveDagRun` — persist a DAG run. */
  stateSaveDagRun(request: SaveDagRunRequest): Promise<SaveDagRunResponse> {
    return this.#call<SaveDagRunRequest, SaveDagRunResponse>(this.#storage.saveDagRun.bind(this.#storage), 'SaveDagRun', request);
  }

  /** `StorageService.LoadDagRuns` — load persisted DAG runs. */
  stateLoadDagRuns(request: LoadDagRunsRequest): Promise<LoadDagRunsResponse> {
    return this.#call<LoadDagRunsRequest, LoadDagRunsResponse>(this.#storage.loadDagRuns.bind(this.#storage), 'LoadDagRuns', request);
  }

  /** `StorageService.SaveTriggerRules` — persist trigger rules. */
  stateSaveTriggerRules(request: SaveTriggerRulesRequest): Promise<SaveTriggerRulesResponse> {
    return this.#call<SaveTriggerRulesRequest, SaveTriggerRulesResponse>(this.#storage.saveTriggerRules.bind(this.#storage), 'SaveTriggerRules', request);
  }

  /** `StorageService.LoadTriggerRules` — load persisted trigger rules. */
  stateLoadTriggerRules(request: LoadTriggerRulesRequest): Promise<LoadTriggerRulesResponse> {
    return this.#call<LoadTriggerRulesRequest, LoadTriggerRulesResponse>(this.#storage.loadTriggerRules.bind(this.#storage), 'LoadTriggerRules', request);
  }

  /** `StorageService.SaveCronJobs` — persist cron jobs. */
  stateSaveCronJobs(request: SaveCronJobsRequest): Promise<SaveCronJobsResponse> {
    return this.#call<SaveCronJobsRequest, SaveCronJobsResponse>(this.#storage.saveCronJobs.bind(this.#storage), 'SaveCronJobs', request);
  }

  /** `StorageService.LoadCronJobs` — load persisted cron jobs. */
  stateLoadCronJobs(request: LoadCronJobsRequest): Promise<LoadCronJobsResponse> {
    return this.#call<LoadCronJobsRequest, LoadCronJobsResponse>(this.#storage.loadCronJobs.bind(this.#storage), 'LoadCronJobs', request);
  }

  // ── event stream ──

  /**
   * `StreamEvents` — server-streaming snapshot/event frames. Every frame
   * (snapshot and event) is delivered to `onFrame`. Resolves when the stream
   * ends normally; rejects on transport errors or when the client is closed.
   */
  streamEvents(onFrame: (frame: StreamFrame) => void, signal?: AbortSignal, sessionId?: string): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const forwardAbort = (): void => {
        this.#closed = true;
        // Cancel the stream by ending the call
      };
      signal?.addEventListener('abort', forwardAbort, { once: true });

      const request: StreamEventsRequest = { sessionId };
      const stream = this.#events.streamEvents.bind(this.#events)(request);

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
    this.#session.close();
    this.#command.close();
    this.#extensions.close();
    this.#graph.close();
    this.#events.close();
    this.#settings.close();
    this.#tools.close();
    this.#storage.close();
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
      invoke(request, (err, response) => {
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

/**
 * Complete a partial `DaemonConfig` for the wire: ts-proto models proto3
 * repeated fields as required arrays. Empty value arrays mean "keep" while
 * `clearFields` carries explicit clear intent.
 */
function fillDaemonConfig(config: Partial<DaemonConfig>): DaemonConfig {
  return {
    ...config,
    builtinSkills: config.builtinSkills ?? [],
    skillsDirs: config.skillsDirs ?? [],
    clearFields: config.clearFields ?? [],
  };
}
