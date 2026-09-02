pub(super) const HOST_BOOTSTRAP_SOURCE: &str = r#"
globalThis.__thewayHandlers = Object.create(null);
globalThis.__thewayEffects = [];
globalThis.__thewayRegistrations = Object.create(null);
globalThis.__thewayRegistrationSequence = 0;
globalThis.__thewayMigrationRegistrationId = null;
globalThis.__thewayCurrentEnvelope = null;
globalThis.__thewayPendingDurableActions = [];
globalThis.__thewayPendingState = new Map();
globalThis.__thewayDisposers = [];

function __thewayDisposedRegistrationIds() {
  return Object.values(globalThis.__thewayRegistrations)
    .filter(({ disposed }) => disposed)
    .map(({ id }) => id);
}

// Dispose queue: `api.effect(fn)` runs `fn` immediately; the disposer it
// returns is executed in reverse registration order when the plugin unloads
// (before the QuickJS VM is destroyed). Each disposer runs isolated — one
// throwing disposer cannot stop the rest.
function __thewayDispose() {
  const report = { executed: 0, errors: [] };
  for (let index = globalThis.__thewayDisposers.length - 1; index >= 0; index--) {
    const entry = globalThis.__thewayDisposers[index];
    if (entry.disposed) continue;
    entry.disposed = true;
    // Count every queue entry as executed even when its disposer throws:
    // a throwing disposer still ran; the error is isolated per entry.
    report.executed++;
    try {
      const disposer = entry.disposer;
      if (typeof disposer === "function") {
        const result = disposer();
        if (result && typeof result.then === "function") {
          result.catch((error) => report.errors.push(String(error?.message ?? error)));
        }
      }
    } catch (error) {
      report.errors.push(String(error?.message ?? error));
    }
  }
  globalThis.__thewayDisposers = [];
  return report;
}

function __thewayHandle(registration) {
  return Object.freeze({
    id: `effect-${registration.id}`,
    dispose() { registration.disposed = true; },
    update(descriptor) {
      if (registration.disposed) {
        const error = new Error("registration handle is disposed");
        error.code = "effect_disposed";
        throw error;
      }
      if (descriptor === null || typeof descriptor !== "object") {
        throw new TypeError("registration descriptor must be an object");
      }
      registration.descriptor = descriptor;
    },
  });
}

function __thewayRegister(kind, descriptor, handler) {
  if (descriptor === null || typeof descriptor !== "object") {
    throw new TypeError(`api.register${kind} requires a descriptor`);
  }
  if ((kind === "tool" || kind === "command" || kind === "request_policy")
      && typeof handler !== "function") {
    throw new TypeError(`${kind} registration requires a handler`);
  }
  const registration = {
    id: globalThis.__thewayRegistrationSequence++,
    kind,
    descriptor,
    handler,
    sequence: globalThis.__thewayRegistrationSequence,
    disposed: false,
  };
  globalThis.__thewayEffects.push(registration);
  globalThis.__thewayRegistrations[registration.id] = registration;
  return __thewayHandle(registration);
}

function __thewayBrokerCall(operation, brokerArgs = {}) {
  const response = JSON.parse(globalThis.__thewayBroker(operation, JSON.stringify(brokerArgs)));
  if (!response.ok) {
    const error = new Error(response.error?.message ?? "capability broker failed");
    error.code = response.error?.code ?? "broker_failed";
    throw error;
  }
  return response.value;
}

function __thewayRequireInvocation() {
  const envelope = globalThis.__thewayCurrentEnvelope;
  if (envelope === null) {
    throw new Error("durable state writes require an active lifecycle invocation");
  }
  return envelope;
}

function __thewayQueueDurable(kind, entry) {
  const envelope = __thewayRequireInvocation();
  const stateSchemaVersion = __thewayBrokerCall("state.schema", {});
  globalThis.__thewayPendingDurableActions.push({
    kind,
    payload: {
      extensionId: envelope.context.extensionId,
      stateSchemaVersion,
      originSequence: Math.max(1, envelope.context.sequence),
      entry,
    },
  });
}

globalThis.__thewaySetup = async function () {
  try {
    const candidate = globalThis.__thewayExtension.default;
    const setup = typeof candidate === "function" ? candidate : candidate?.setup;
    if (typeof setup !== "function") {
      throw new TypeError("extension default export must be created by defineExtension");
    }
    const api = Object.freeze({
      capabilities: Object.freeze({
        has(permission) {
          return __thewayBrokerCall("capabilities.has", { permission });
        },
      }),
      workspace: Object.freeze({
        async readText(path) {
          return __thewayBrokerCall("workspace.readText", { path });
        },
        async writeText(path, content) {
          return __thewayBrokerCall("workspace.writeText", { path, content });
        },
      }),
      process: Object.freeze({
        async run(argv, options = {}) {
          return __thewayBrokerCall("process.run", {
            argv,
            timeoutMs: options.timeoutMs ?? null,
          });
        },
      }),
      network: Object.freeze({
        async fetch(url, options = {}) {
          return __thewayBrokerCall("network.fetch", {
            url,
            method: options.method ?? null,
            headers: options.headers ?? {},
            body: options.body ?? null,
          });
        },
      }),
      secrets: Object.freeze({
        async read(name) {
          return __thewayBrokerCall("secrets.read", { name });
        },
      }),
      providerRaw: Object.freeze({
        async read() {
          return __thewayBrokerCall("providerRaw.read", {});
        },
      }),
      state: Object.freeze({
        get(key) {
          if (globalThis.__thewayPendingState.has(key)) {
            return globalThis.__thewayPendingState.get(key);
          }
          return __thewayBrokerCall("state.get", { key });
        },
        set(key, value) {
          __thewayQueueDurable("set_state", {
            kind: "state_mutation",
            key,
            mutation: { operation: "set", value },
          });
          globalThis.__thewayPendingState.set(key, value);
        },
        delete(key) {
          __thewayQueueDurable("delete_state", {
            kind: "state_mutation",
            key,
            mutation: { operation: "delete" },
          });
          globalThis.__thewayPendingState.set(key, null);
        },
      }),
      events: Object.freeze({
        replay(customType = null) {
          return __thewayBrokerCall("events.replay", { customType });
        },
        append(eventId, type, payload) {
          __thewayQueueDurable("append_custom_event", {
            kind: "custom_event",
            eventId,
            customType: type,
            payload,
          });
        },
      }),
      modelContext: Object.freeze({
        append(contextId, placement, content) {
          __thewayQueueDurable("append_model_context", {
            kind: "model_context",
            contextId,
            placement,
            content,
          });
        },
      }),
      memory: Object.freeze({
        get(key) {
          return __thewayBrokerCall("memory.get", { key });
        },
        set(key, value) {
          return __thewayBrokerCall("memory.set", { key, value });
        },
        delete(key) {
          return __thewayBrokerCall("memory.delete", { key });
        },
        clear() {
          return __thewayBrokerCall("memory.clear", {});
        },
      }),
      effect(execute) {
        if (typeof execute !== "function") {
          throw new TypeError("api.effect requires a function");
        }
        const disposer = execute();
        globalThis.__thewayDisposers.push({
          disposer: typeof disposer === "function" ? disposer : null,
          disposed: false,
        });
        const registration = {
          id: globalThis.__thewayRegistrationSequence++,
          kind: "effect",
          disposed: false,
        };
        globalThis.__thewayRegistrations[registration.id] = registration;
        return __thewayHandle(registration);
      },
      migrateState(handler) {        if (typeof handler !== "function") {
          throw new TypeError("api.migrateState requires a handler");
        }
        if (globalThis.__thewayMigrationRegistrationId !== null) {
          throw new Error("only one state migration handler may be registered");
        }
        const registration = {
          id: globalThis.__thewayRegistrationSequence++,
          kind: "state_migration",
          handler,
          disposed: false,
        };
        globalThis.__thewayRegistrations[registration.id] = registration;
        globalThis.__thewayMigrationRegistrationId = registration.id;
        return __thewayHandle(registration);
      },
      registerTool(descriptor, handler) {
        return __thewayRegister("tool", descriptor, handler);
      },
      registerCommand(descriptor, handler) {
        return __thewayRegister("command", descriptor, handler);
      },
      registerProvider(descriptor) {
        return __thewayRegister("provider", descriptor, undefined);
      },
      registerPromptSection(descriptor) {
        return __thewayRegister("prompt_section", descriptor, undefined);
      },
      registerRequestPolicy(descriptor, handler) {
        return __thewayRegister("request_policy", descriptor, handler);
      },
      contribute(descriptor) {
        return __thewayRegister("contribution", descriptor, undefined);
      },
      on(event, descriptor, handler) {
        if (typeof descriptor === "function") {
          handler = descriptor;
          descriptor = {};
        }
        if (typeof event !== "string" || event.length === 0 || typeof handler !== "function") {
          throw new TypeError("api.on requires an event name and handler");
        }
        const registration = {
          id: globalThis.__thewayRegistrationSequence,
          event,
          descriptor: descriptor ?? {},
          handler,
          priority: Number.isFinite(descriptor?.priority) ? descriptor.priority : 0,
          sequence: globalThis.__thewayRegistrationSequence++,
          disposed: false,
        };
        (globalThis.__thewayHandlers[event] ??= []).push(registration);
        globalThis.__thewayRegistrations[registration.id] = registration;
        return __thewayHandle(registration);
      },
    });
    await setup(api);
    return JSON.stringify({
      ok: true,
      value: {
        registrations: Object.values(globalThis.__thewayHandlers)
          .flat()
          .filter(({ disposed }) => !disposed)
          .map(({ id, event, descriptor, sequence }) => ({
            registrationId: id,
            event,
            descriptor,
            sequence,
          })),
        effects: globalThis.__thewayEffects
          .filter(({ disposed }) => !disposed)
          .map(({ id, kind, descriptor, sequence }) => ({
            registrationId: id,
            kind,
            descriptor,
            sequence,
          })),
        migrationRegistrationId: globalThis.__thewayMigrationRegistrationId,
      },
    });
  } catch (error) {
    return JSON.stringify({ error: String(error?.message ?? error) });
  }
};

globalThis.__thewayInvoke = async function (serializedEnvelope, registrationId) {
  try {
    const envelope = JSON.parse(serializedEnvelope);
    globalThis.__thewayCurrentEnvelope = envelope;
    globalThis.__thewayPendingDurableActions = [];
    globalThis.__thewayPendingState = new Map();
    const registration = globalThis.__thewayRegistrations[registrationId];
    if (registration === undefined) {
      throw new Error("registration is unavailable");
    }
    if (registration.disposed) {
      const error = new Error("registration handle is disposed");
      error.code = "effect_disposed";
      throw error;
    }
    if (registration.event !== undefined && registration.event !== envelope.event) {
      throw new Error("hook registration does not accept this event");
    }
    const result = registration.event !== undefined
      ? await registration.handler(envelope, envelope.context)
      : await registration.handler(envelope.payload, envelope.context);
    globalThis.__thewayCurrentEnvelope = null;
    return JSON.stringify({
      ok: true,
      value: {
        result: result ?? null,
        disposedRegistrationIds: __thewayDisposedRegistrationIds(),
        queuedDurableActions: globalThis.__thewayPendingDurableActions,
      },
    });
  } catch (error) {
    globalThis.__thewayCurrentEnvelope = null;
    globalThis.__thewayPendingDurableActions = [];
    globalThis.__thewayPendingState = new Map();
    return JSON.stringify({ error: String(error?.message ?? error) });
  }
};
"#;
