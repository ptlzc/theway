use theway_contract::extension::RUNTIME_EXTENSION_ABI_MAJOR;

/// Generate the only host module available to package imports. Capability
/// brokers are added as explicit API methods; no ambient daemon authority is
/// copied into the JavaScript global object.
pub(super) fn generated_theway_module() -> String {
    format!(
        r#"
export const abiMajor = {abi};
export function defineExtension(setup) {{
  if (typeof setup !== "function") {{
    throw new TypeError("defineExtension requires a setup function");
  }}
  return Object.freeze({{ setup }});
}}
"#,
        abi = RUNTIME_EXTENSION_ABI_MAJOR
    )
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
