//! Auth-store helpers shared by command implementations (temporary SDK home —
//! daemon-kernel-layers: these move to the daemon together with `local::auth`).

use std::path::PathBuf;

/// Credential hint for a provider when neither the env vars nor the auth store hold one:
/// tells the user which env var to set or to run `/login <provider>`. `None` when a
/// credential is already available.
pub fn model_credential_hint(provider: &str) -> Option<String> {
    let vars = theway_llm_provider::env_api_keys::env_var_names(provider);
    let has_env = vars.iter().any(|var| {
        std::env::var(var)
            .ok()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    });
    if has_env {
        return None;
    }
    let has_stored = crate::local::auth::AuthStore::load()
        .ok()
        .and_then(|store| store.get(provider).cloned())
        .is_some();
    if has_stored {
        return None;
    }

    let env_hint = if vars.is_empty() {
        "set the provider API key env var".to_string()
    } else {
        format!("set {}", vars.join(" or "))
    };
    Some(format!("{env_hint} or run /login {provider}"))
}

/// Store an API key for `provider` in the local auth store (`~/.theway/auth.json`).
/// Returns the auth store path on success.
#[cfg_attr(test, allow(dead_code))]
pub fn save_api_key(provider: &str, token: &str) -> Result<PathBuf, String> {
    let mut store = match crate::local::auth::AuthStore::load() {
        Ok(s) => s,
        Err(e) => return Err(format!("load auth store: {e}")),
    };
    store.set(
        provider.to_string(),
        crate::local::auth::ProviderCredential::ApiKey {
            value: token.to_string(),
        },
    );
    if let Err(e) = store.save() {
        return Err(format!("save auth store: {e}"));
    }
    Ok(crate::local::auth::auth_path())
}

