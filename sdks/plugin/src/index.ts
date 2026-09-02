import type {
  ExtensionActionBatch,
  ExtensionActionKind,
  ExtensionClientContribution,
  ExtensionCommandOutcome,
  ExtensionDeliveryPolicy,
  ExtensionEventContext,
  ExtensionEventEnvelope,
  ExtensionHookClass,
  ExtensionHookDeadline,
  ExtensionHookFailurePolicy,
  ExtensionLifecycleEvent,
  ExtensionModelContextPlacement,
  ExtensionPermission,
  ExtensionScope,
  JsonValue,
} from '../abi/theway-extension.js';

export type * from '../abi/theway-extension.js';

export type MaybePromise<T> = T | Promise<T>;
export type JsonObject = { [key: string]: JsonValue };
export type JsonSchema = boolean | JsonObject;

export type ThinkingBudgets = JsonObject & {
  minimal?: number;
  low?: number;
  medium?: number;
  high?: number;
};

export type NormalizedGenerationOptions = JsonObject & {
  temperature?: number;
  maxTokens?: number;
  reasoning?: string;
  thinkingBudgets?: ThinkingBudgets;
};

export type ModelToolDefinition = JsonObject & {
  name: string;
  description: string;
  parameters: JsonSchema;
};

export type NormalizedModelRequest = JsonObject & {
  provider: string;
  model: string;
  systemInstructions?: string;
  messages: JsonValue[];
  visibleTools: ModelToolDefinition[];
  executableToolNames: string[];
  generationOptions: NormalizedGenerationOptions;
};

export type ExtensionEventName = ExtensionLifecycleEvent | (string & {});

export interface KnownLifecyclePayloads {
  custom: { event: string; payload: JsonValue };
  input: { message: JsonValue };
  context: { messages: JsonValue[] };
  before_model_request: { request: NormalizedModelRequest };
  before_provider_request_headers: { request: JsonObject };
  before_provider_request_raw: { request: JsonObject };
  provider_response: { response: JsonObject };
  provider_request_failed: { failure: JsonObject };
  message_start: { message: JsonValue };
  message_update: { message: JsonValue; updateKind: string };
  message_end: { message: JsonValue };
  tool_call: { assistantMessage: JsonValue; toolCall: JsonValue; args: JsonValue };
  tool_execution_start: { toolName: string; args: JsonValue };
  tool_execution_update: { toolName: string; update: JsonValue };
  tool_execution_end: { toolName: string; result: JsonValue; isError: boolean };
  tool_result: { toolCall: JsonValue; args: JsonValue; result: JsonValue; isError: boolean };
}

export type LifecyclePayload<E extends ExtensionEventName> =
  E extends keyof KnownLifecyclePayloads ? KnownLifecyclePayloads[E] : JsonValue;

export type LifecycleEnvelope<E extends ExtensionEventName = ExtensionLifecycleEvent> = Omit<
  ExtensionEventEnvelope,
  'event' | 'payload'
> & {
  event: E;
  payload: LifecyclePayload<E>;
};

export interface HookDescriptor {
  class?: ExtensionHookClass;
  payloadSchema?: JsonSchema;
  allowedActions?: ExtensionActionKind[];
  priority?: number;
  deadline?: ExtensionHookDeadline;
  delivery?: ExtensionDeliveryPolicy;
  failure?: ExtensionHookFailurePolicy;
}

export type HookResult = MaybePromise<ExtensionActionBatch | JsonValue | null | undefined | void>;
export type HookHandler<E extends ExtensionEventName> = (
  event: LifecycleEnvelope<E>,
  context: ExtensionEventContext,
) => HookResult;

export interface RegistrationHandle<Descriptor = JsonObject> {
  readonly id: string;
  dispose(): void;
  update(descriptor: Descriptor): void;
}

export type ToolPermission = 'allow' | 'prompt' | 'block';

export interface ToolRegistrationDescriptor {
  name: string;
  label: string;
  description: string;
  inputSchema: JsonSchema;
  resultSchema?: JsonSchema;
  permission?: ToolPermission;
  scope?: ExtensionScope;
  override?: boolean;
}

export interface ToolInvocation<Arguments = JsonValue> {
  toolCallId: string;
  arguments: Arguments;
}

export interface ToolResult {
  content: JsonValue[];
  details?: JsonValue;
  terminate?: boolean;
}

export type ToolHandler<Arguments = JsonValue> = (
  invocation: ToolInvocation<Arguments>,
  context: ExtensionEventContext,
) => MaybePromise<ToolResult>;

export interface RegistrationPredicate {
  providers?: string[];
  models?: string[];
  requiresInteractiveClient?: boolean;
}

export interface CommandRegistrationDescriptor {
  name: string;
  label: string;
  description: string;
  argumentSchema: JsonSchema;
  availability?: RegistrationPredicate;
  scope?: ExtensionScope;
}

export type CommandHandler<Arguments = JsonValue> = (
  invocation: { arguments: Arguments },
  context: ExtensionEventContext,
) => MaybePromise<ExtensionCommandOutcome>;

export type ProviderWireFormat =
  | 'openai_chat_completions'
  | 'openai_responses'
  | 'anthropic_messages';

export type ProviderInputModality = 'text' | 'image';

export interface ProviderModelDescriptor {
  id: string;
  name: string;
  reasoning?: boolean;
  input?: ProviderInputModality[];
  contextWindow: number;
  maxTokens: number;
}

export interface ProviderRegistrationDescriptor {
  providerId: string;
  baseUrl: string;
  format: ProviderWireFormat;
  credentialRef?: string;
  models: ProviderModelDescriptor[];
  scope?: ExtensionScope;
}

export interface PromptSectionDescriptor {
  sectionId: string;
  text: string;
  priority?: number;
  predicate?: RegistrationPredicate;
  scope?: ExtensionScope;
}

export interface RequestPolicyDescriptor {
  policyId: string;
  priority?: number;
  predicate?: RegistrationPredicate;
  scope?: ExtensionScope;
}

export type RequestPolicyHandler = (
  payload: { request: NormalizedModelRequest },
  context: ExtensionEventContext,
) => HookResult;

export interface ActionRegistrationDescriptor {
  name: string;
  description?: string;
  inputSchema: JsonSchema;
  scope?: ExtensionScope;
}

export type ActionHandler<Arguments = JsonValue> = (
  invocation: { arguments: Arguments },
  context: ExtensionEventContext,
) => MaybePromise<JsonValue>;

export interface PromptVariableDescriptor {
  sectionId: string;
  text: string;
  priority?: number;
  predicate?: RegistrationPredicate;
  scope?: ExtensionScope;
}

export interface RuntimeIdentity {
  version: string;
  pluginId: string;
  sessionId: string;
}

export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

export type DispatchMode = 'emit' | 'parallel' | 'serial' | 'bail' | 'waterfall';

export type NativeName = 'notify' | 'httpRequest' | 'log';

export interface ServiceHandle {
  dispose(): void;
}

export interface CustomEvent {
  eventId: string;
  type: string;
  payload: JsonValue;
  stateSchemaVersion: number;
  originSequence: number;
}

export interface StateMigrationInput {
  fromSchemaVersion: number;
  toSchemaVersion: number;
  state: Record<string, JsonValue>;
}

export type StateMigrationHandler = (
  input: StateMigrationInput,
  context: ExtensionEventContext,
) => MaybePromise<{ state: Record<string, JsonValue> }>;

export interface ProcessResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export interface NetworkRequestOptions {
  method?: 'GET' | 'POST';
  headers?: Record<string, string>;
  body?: string;
}

export interface NetworkResponse {
  status: number;
  body: string;
}

export interface ExtensionApi {
  readonly capabilities: {
    has(permission: ExtensionPermission): boolean;
  };
  readonly workspace: {
    readText(path: string): Promise<string>;
    writeText(path: string, content: string): Promise<void>;
  };
  readonly process: {
    run(argv: string[], options?: { timeoutMs?: number }): Promise<ProcessResult>;
  };
  readonly network: {
    fetch(url: string, options?: NetworkRequestOptions): Promise<NetworkResponse>;
  };
  readonly secrets: {
    read(name: string): Promise<string>;
  };
  readonly providerRaw: {
    read(): Promise<JsonValue>;
  };
  readonly state: {
    get(key: string): JsonValue;
    set(key: string, value: JsonValue): void;
    delete(key: string): void;
  };
  readonly events: {
    replay(customType?: string | null): CustomEvent[];
    append(eventId: string, type: string, payload: JsonValue): void;
  };
  readonly modelContext: {
    append(contextId: string, placement: ExtensionModelContextPlacement, content: JsonValue): void;
  };
  readonly memory: {
    get(key: string): JsonValue;
    set(key: string, value: JsonValue): void;
    delete(key: string): void;
    clear(): void;
  };
  readonly runtime: RuntimeIdentity;
  effect(execute: () => MaybePromise<void | (() => MaybePromise<void>)>): RegistrationHandle;
  getConfig(): JsonValue;
  provide(name: string, value: JsonValue): ServiceHandle;
  get(name: string): JsonValue | undefined;
  native(name: NativeName, args?: JsonObject): JsonValue;
  log(level: LogLevel, message: string): void;
  emit(event: string, payload?: JsonValue, mode?: DispatchMode): JsonValue;
  migrateState(handler: StateMigrationHandler): RegistrationHandle;
  registerTool<Arguments = JsonValue>(
    descriptor: ToolRegistrationDescriptor,
    handler: ToolHandler<Arguments>,
  ): RegistrationHandle<ToolRegistrationDescriptor>;
  registerTool<Arguments = JsonValue>(
    name: string,
    description: string,
    inputSchema: JsonSchema,
    handler: ToolHandler<Arguments>,
  ): RegistrationHandle<ToolRegistrationDescriptor>;
  registerCommand<Arguments = JsonValue>(
    descriptor: CommandRegistrationDescriptor,
    handler: CommandHandler<Arguments>,
  ): RegistrationHandle<CommandRegistrationDescriptor>;
  registerProvider(
    descriptor: ProviderRegistrationDescriptor,
  ): RegistrationHandle<ProviderRegistrationDescriptor>;
  registerPromptSection(
    descriptor: PromptSectionDescriptor,
  ): RegistrationHandle<PromptSectionDescriptor>;
  registerPromptVariable(
    descriptor: PromptVariableDescriptor,
  ): RegistrationHandle<PromptVariableDescriptor>;
  registerRequestPolicy(
    descriptor: RequestPolicyDescriptor,
    handler: RequestPolicyHandler,
  ): RegistrationHandle<RequestPolicyDescriptor>;
  registerAction<Arguments = JsonValue>(
    descriptor: ActionRegistrationDescriptor,
    handler: ActionHandler<Arguments>,
  ): RegistrationHandle<ActionRegistrationDescriptor>;
  registerAction<Arguments = JsonValue>(
    name: string,
    handler: ActionHandler<Arguments>,
  ): RegistrationHandle<ActionRegistrationDescriptor>;
  contribute(
    descriptor: ExtensionClientContribution,
  ): RegistrationHandle<ExtensionClientContribution>;
  on<E extends ExtensionEventName>(event: E, handler: HookHandler<E>): RegistrationHandle;
  on<E extends ExtensionEventName>(
    event: E,
    descriptor: HookDescriptor,
    handler: HookHandler<E>,
  ): RegistrationHandle<HookDescriptor>;
  once<E extends ExtensionEventName>(event: E, handler: HookHandler<E>): RegistrationHandle;
  once<E extends ExtensionEventName>(
    event: E,
    descriptor: HookDescriptor,
    handler: HookHandler<E>,
  ): RegistrationHandle<HookDescriptor>;
}

export type ExtensionSetup = (api: ExtensionApi) => MaybePromise<void>;

export interface ExtensionDefinition {
  readonly setup: ExtensionSetup;
  readonly inject?: readonly string[];
  readonly kind?: string;
}

export function defineExtension(setup: ExtensionSetup): Readonly<ExtensionDefinition> {
  if (typeof setup !== 'function') {
    throw new TypeError('defineExtension requires a setup function');
  }
  return Object.freeze({ setup });
}

export interface RegisterOptions {
  inject?: readonly string[];
}

export function register(
  setup: ExtensionSetup,
  options: RegisterOptions = {},
): Readonly<ExtensionDefinition & { inject: readonly string[] }> {
  if (typeof setup !== 'function') {
    throw new TypeError('register requires a setup function');
  }
  return Object.freeze({ setup, inject: options.inject ?? [] });
}
