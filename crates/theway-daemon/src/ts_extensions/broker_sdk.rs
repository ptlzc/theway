//! Virtual plugin SDK module injected into QuickJS. It exports only
//! `defineExtension` and the side-effect `register` entry form; every other
//! author-facing capability is installed by the host bootstrap through the
//! capability broker.

/// Package name of the only host module available to extension imports.
pub(super) const PLUGIN_SDK_MODULE: &str = "@theway-ai/plugin-sdk";

/// Generate the plugin SDK host module. Capability brokers are added as
/// explicit API methods; no ambient daemon authority is copied into the
/// JavaScript global object.
pub(super) fn generated_theway_module() -> String {
    r#"
export function defineExtension(setup) {
  if (typeof setup !== "function") {
    throw new TypeError("defineExtension requires a setup function");
  }
  return Object.freeze({ setup });
}

export function register(setup, options) {
  if (typeof setup !== "function") {
    throw new TypeError("register requires a setup function");
  }
  if (options !== undefined && (options === null || typeof options !== "object")) {
    throw new TypeError("register options must be an object");
  }
  globalThis.__thewayRegisteredExtension = Object.freeze({
    setup,
    inject: Array.isArray(options?.inject) ? options.inject : [],
  });
  return globalThis.__thewayRegisteredExtension;
}
"#
    .to_string()
}

/// Names intentionally absent from the direct package environment. Tests use
/// this list to keep future host additions capability-brokered.
pub(super) const FORBIDDEN_DIRECT_GLOBALS: &[&str] = &[
    "process",
    "Deno",
    "Bun",
    "require",
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "thewayFilesystem",
    "thewayNetwork",
    "thewayEnvironment",
    "thewaySecrets",
    "thewayProvider",
    "thewayPersistence",
];
