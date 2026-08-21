import { defineExtension } from "@theway-ai/plugin-sdk";

const CONFIG_PATH = ".theway/extensions/deepseek-anchor/anchor-config.json";
const PROMOTION_KEY = "anchor.phase";
const PROMOTION_EVENT_ID = "anchor-promotion-v1";
const RESTORED_CONTEXT_ID = "anchor-restored-context-v1";
const EDITOR_NAME = "str_replace_editor";
const MAX_CONFIG_BYTES = 64 * 1024;

const EDITOR_DESCRIPTION = `Custom editing tool for viewing, creating and editing files.
* The view command displays a file with line numbers.
* The create command refuses to overwrite an existing file.
* str_replace requires old_str to occur exactly once.
* insert places new_str after insert_line.`;

const EDITOR_SCHEMA = {
  type: "object",
  properties: {
    command: {
      type: "string",
      description: "Allowed options: view, create, str_replace, insert.",
      enum: ["view", "create", "str_replace", "insert"],
    },
    path: { type: "string", description: "Absolute or workspace-relative file path." },
    file_text: { type: "string", description: "Required by create." },
    insert_line: { type: "integer", description: "Insert after this zero-or-one-based boundary." },
    new_str: { type: "string", description: "Replacement or inserted text." },
    old_str: { type: "string", description: "Unique literal text replaced by str_replace." },
    view_range: {
      type: "array",
      description: "Optional one-based inclusive [start, end], with -1 meaning EOF.",
      items: { type: "integer" },
    },
  },
  required: ["command", "path"],
};

function failConfig(message) {
  throw new Error(`deepseek-anchor configuration: ${message}`);
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function validateStringArray(value, name, allowEmpty = false) {
  if (!Array.isArray(value) || (!allowEmpty && value.length === 0)) {
    failConfig(`${name} must be ${allowEmpty ? "an" : "a non-empty"} array`);
  }
  if (value.some((item) => typeof item !== "string" || item.trim().length === 0)) {
    failConfig(`${name} entries must be non-empty strings`);
  }
  if (new Set(value).size !== value.length) failConfig(`${name} entries must be unique`);
}

function validateConfig(value) {
  if (!isObject(value)) failConfig("root must be an object");
  const allowed = new Set([
    "$schema", "providerPredicates", "modelPredicates", "bootstrapPrompt",
    "promotionCondition", "personaScope", "bootstrapTokenLimit", "restoredContext",
    "maxEditorOutputChars", "zeroAnchor",
  ]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) failConfig(`unknown property ${key}`);
  }
  validateStringArray(value.providerPredicates, "providerPredicates");
  validateStringArray(value.modelPredicates, "modelPredicates");
  if (typeof value.bootstrapPrompt !== "string" || value.bootstrapPrompt.trim().length === 0) {
    failConfig("bootstrapPrompt must be a non-empty string");
  }
  if (!isObject(value.promotionCondition)) failConfig("promotionCondition must be an object");
  const conditionAllowed = new Set(["kind", "textPattern", "toolNames"]);
  for (const key of Object.keys(value.promotionCondition)) {
    if (!conditionAllowed.has(key)) failConfig(`unknown promotionCondition property ${key}`);
  }
  if (!["first_assistant", "first_tool_call", "assistant_or_tool_call"]
    .includes(value.promotionCondition.kind)) {
    failConfig("promotionCondition.kind is invalid");
  }
  if (value.promotionCondition.textPattern !== undefined) {
    if (typeof value.promotionCondition.textPattern !== "string") {
      failConfig("promotionCondition.textPattern must be a string");
    }
    try { new RegExp(value.promotionCondition.textPattern); } catch (_) {
      failConfig("promotionCondition.textPattern must be a valid regular expression");
    }
  }
  validateStringArray(value.promotionCondition.toolNames ?? [], "promotionCondition.toolNames", true);
  if (!["bootstrap_only", "session"].includes(value.personaScope)) {
    failConfig("personaScope must be bootstrap_only or session");
  }
  if (value.bootstrapTokenLimit !== undefined
      && (!Number.isSafeInteger(value.bootstrapTokenLimit) || value.bootstrapTokenLimit <= 0)) {
    failConfig("bootstrapTokenLimit must be a positive safe integer");
  }
  if (typeof value.restoredContext !== "string" || value.restoredContext.trim().length === 0) {
    failConfig("restoredContext must be a non-empty string");
  }
  if (value.maxEditorOutputChars !== undefined
      && (!Number.isSafeInteger(value.maxEditorOutputChars) || value.maxEditorOutputChars <= 0)) {
    failConfig("maxEditorOutputChars must be a positive safe integer");
  }
  if (typeof value.zeroAnchor !== "boolean") failConfig("zeroAnchor must be a boolean");
  return Object.freeze({ ...value, maxEditorOutputChars: value.maxEditorOutputChars ?? 16000 });
}

async function loadConfig(api) {
  const source = await api.workspace.readText(CONFIG_PATH);
  if (source.length > MAX_CONFIG_BYTES) failConfig("file exceeds 64 KiB");
  let value;
  try { value = JSON.parse(source); } catch (_) { failConfig("file is not valid JSON"); }
  return validateConfig(value);
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function matchesGlob(value, pattern) {
  const source = pattern.split("*").map(escapeRegex).join(".*");
  return new RegExp(`^${source}$`).test(value);
}

function matchesModel(config, model) {
  return model !== undefined
    && config.providerPredicates.some((item) => matchesGlob(model.provider, item))
    && config.modelPredicates.some((item) => matchesGlob(model.model, item));
}

function requiredIncludes(schema, name) {
  return Array.isArray(schema?.required) && schema.required.includes(name);
}

function compatibleBash(tool) {
  const schema = tool?.parameters;
  return tool?.name === "bash" && schema?.type === "object"
    && schema.properties?.command?.type === "string" && requiredIncludes(schema, "command");
}

function compatibleEditor(tool) {
  const schema = tool?.parameters;
  const commands = schema?.properties?.command?.enum;
  return tool?.name === EDITOR_NAME && schema?.type === "object"
    && requiredIncludes(schema, "command") && requiredIncludes(schema, "path")
    && ["view", "create", "str_replace", "insert"].every((item) => commands?.includes(item));
}

function clip(value, limit) {
  return value.length <= limit ? value : `${value.slice(0, limit)}<response clipped>`;
}

function formatView(path, content, range, limit) {
  const all = content.split("\n");
  let start = 1;
  let end = all.length;
  if (range !== undefined) {
    if (!Array.isArray(range) || range.length !== 2 || !range.every(Number.isInteger)) {
      throw new Error("view_range must contain exactly two integers");
    }
    [start, end] = range;
    if (end === -1) end = all.length;
    if (start < 1 || start > all.length || end < start || end > all.length) {
      throw new Error(`view_range must be within [1, ${all.length}]`);
    }
  }
  const body = all.slice(start - 1, end)
    .map((line, index) => `${String(start + index).padStart(6, " ")}  ${line}`).join("\n");
  return clip(`Here's the content of ${path} with line numbers:\n${body}\n`, limit);
}

async function runEditor(api, config, args) {
  const { command, path } = args;
  if (typeof path !== "string" || path.trim().length === 0) throw new Error("path is required");
  if (command === "view") {
    return formatView(path, await api.workspace.readText(path), args.view_range,
      config.maxEditorOutputChars);
  }
  if (command === "create") {
    if (typeof args.file_text !== "string") throw new Error("file_text is required for create");
    try {
      await api.workspace.readText(path);
      throw new Error(`File already exists at: ${path}`);
    } catch (error) {
      if (error.code !== "not_found") throw error;
    }
    await api.workspace.writeText(path, args.file_text);
    return `New file created successfully at: ${path}`;
  }
  const before = await api.workspace.readText(path);
  if (command === "str_replace") {
    if (typeof args.old_str !== "string" || args.old_str.length === 0) {
      throw new Error("old_str is required for str_replace");
    }
    const first = before.indexOf(args.old_str);
    if (first < 0) throw new Error("old_str did not appear verbatim");
    if (before.indexOf(args.old_str, first + args.old_str.length) >= 0) {
      throw new Error("old_str occurs more than once and must be made unique");
    }
    const next = `${before.slice(0, first)}${args.new_str ?? ""}`
      + before.slice(first + args.old_str.length);
    await api.workspace.writeText(path, next);
    return `The file ${path} has been edited successfully.`;
  }
  if (command === "insert") {
    if (!Number.isInteger(args.insert_line)) throw new Error("insert_line is required for insert");
    if (typeof args.new_str !== "string") throw new Error("new_str is required for insert");
    const lines = before.split("\n");
    if (args.insert_line < 0 || args.insert_line > lines.length) {
      throw new Error(`insert_line must be within [0, ${lines.length}]`);
    }
    lines.splice(args.insert_line, 0, ...args.new_str.split("\n"));
    await api.workspace.writeText(path, lines.join("\n"));
    return `The file ${path} has been edited successfully.`;
  }
  throw new Error(`unsupported editor command ${command}`);
}

function diagnostic(phase, details = {}) {
  return {
    kind: "emit_diagnostic",
    payload: {
      code: "lifecycle_status",
      severity: "info",
      message: `DeepSeek Anchor phase: ${phase}`,
      details: { phase, ...details },
    },
  };
}

function decisionDiagnostic(api, phase, details = {}) {
  if (api.memory.get("last-diagnostic-phase") === phase) return [];
  api.memory.set("last-diagnostic-phase", phase);
  return [diagnostic(phase, details)];
}

function acceptedAssistant(message) {
  return message?.role === "assistant" && message.errorMessage == null
    && message.stopReason !== "error" && message.stopReason !== "aborted";
}

function matchesPromotion(config, message) {
  if (!acceptedAssistant(message)) return false;
  const text = (message.content ?? []).filter((item) => item.type === "text")
    .map((item) => item.text ?? "").join("\n");
  const calls = (message.content ?? []).filter((item) => item.type === "toolCall");
  const pattern = config.promotionCondition.textPattern;
  const textAccepted = pattern === undefined ? text.length > 0 : new RegExp(pattern).test(text);
  const names = config.promotionCondition.toolNames ?? [];
  const toolAccepted = calls.some((call) => names.length === 0 || names.includes(call.name));
  switch (config.promotionCondition.kind) {
    case "first_assistant": return textAccepted;
    case "first_tool_call": return toolAccepted;
    default: return textAccepted || toolAccepted;
  }
}

export default defineExtension(async (api) => {
  const config = await loadConfig(api);
  if (!config.zeroAnchor) {
    api.registerTool({
      name: EDITOR_NAME,
      label: "String replace editor",
      description: EDITOR_DESCRIPTION,
      inputSchema: EDITOR_SCHEMA,
      resultSchema: { type: "object", required: ["content", "details"] },
      permission: "prompt",
      override: api.capabilities.has("tools.override"),
    }, async ({ arguments: args }) => ({
      content: [{ type: "text", text: await runEditor(api, config, args) }],
      details: { command: args.command, path: args.path },
    }));
  }

  api.on("before_model_request", async ({ payload, context }) => {
    const request = payload.request;
    if (config.zeroAnchor) {
      return { abiMajor: 2, actions: decisionDiagnostic(api, "zero_anchor") };
    }
    if (!matchesModel(config, context.model)) {
      return { abiMajor: 2, actions: decisionDiagnostic(api, "inactive") };
    }
    if (api.state.get(PROMOTION_KEY) === "promoted") {
      const actions = decisionDiagnostic(api, "promoted");
      if (config.personaScope === "session") {
        const base = request.systemInstructions ?? "";
        actions.unshift({ kind: "replace_model_request", payload: {
          request: { ...request, systemInstructions: base.length === 0
            ? config.bootstrapPrompt : `${config.bootstrapPrompt}\n\n${base}` },
        } });
      }
      return { abiMajor: 2, actions };
    }
    const visible = request.visibleTools ?? [];
    const executable = new Set(request.executableToolNames ?? []);
    const bash = visible.find(compatibleBash);
    const editor = visible.find(compatibleEditor);
    if (bash === undefined || editor === undefined
        || !executable.has("bash") || !executable.has(EDITOR_NAME)) {
      throw new Error("anchor_configuration: bootstrap requires compatible bash and str_replace_editor tools");
    }
    const generationOptions = config.bootstrapTokenLimit === undefined
      ? request.generationOptions
      : { ...request.generationOptions, maxTokens: config.bootstrapTokenLimit };
    api.memory.set("bootstrap-armed", true);
    return { abiMajor: 2, actions: [
      { kind: "replace_model_request", payload: { request: {
        ...request,
        systemInstructions: config.bootstrapPrompt,
        messages: [],
        visibleTools: [bash, editor],
        executableToolNames: ["bash", EDITOR_NAME],
        generationOptions,
      } } },
      ...decisionDiagnostic(api, "bootstrap"),
    ] };
  });

  api.on("message_end", async ({ payload, context }) => {
    if (config.zeroAnchor || !matchesModel(config, context.model)
        || api.state.get(PROMOTION_KEY) === "promoted"
        || api.memory.get("bootstrap-armed") !== true
        || !matchesPromotion(config, payload.message)) {
      return null;
    }
    api.state.set(PROMOTION_KEY, "promoted");
    api.events.append(PROMOTION_EVENT_ID, "anchor.promotion", {
      provider: context.model.provider,
      model: context.model.model,
    });
    api.modelContext.append(RESTORED_CONTEXT_ID, "system_prompt_section",
      config.restoredContext);
    return { abiMajor: 2, actions: [diagnostic("promoted")] };
  });
});
