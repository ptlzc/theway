//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! Persistent credential store — the auth store file format is shared contract:
//! the TUI's /login flow writes it, the daemon reads it. Stores per-provider
//! credentials at
//! `~/.theway/auth.json` with mode 0600; `resolve_for_provider` plumbs into
//! model auto-detection and the stream wrapper.
//!
//! Resolution precedence (in `resolve_for_provider`):
//!   1. The provider's environment variable, if set and non-empty.
//!   2. A matching entry in `auth.json`.
//!   3. None.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::client::base_dir;

pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn auth_path() -> PathBuf {
    base_dir().join("auth.json")
}

/// Human-readable guidance for when `/login` cannot run (non-TTY stdin).
pub fn login_requires_tty_message(provider: &str, recovery_command: Option<&str>) -> String {
    let command = recovery_command
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("/login {provider}"));
    format!(
        "/login requires an interactive terminal so the API key is not echoed; run theway in a TTY and use `{command}`"
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderCredential {
    ApiKey {
        value: String,
    },
    Oauth {
        access_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        /// Unix epoch seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<i64>,
        #[serde(default)]
        scopes: Vec<String>,
    },
}

impl ProviderCredential {
    /// True if this is an OAuth credential whose expires_at is in the past or within `slack`
    /// seconds from now. Wired in by the OAuth refresher (lands in a follow-up).
    #[allow(dead_code)]
    pub fn needs_refresh(&self, slack_seconds: i64) -> bool {
        match self {
            Self::ApiKey { .. } => false,
            Self::Oauth { expires_at, .. } => match expires_at {
                Some(exp) => {
                    let now = chrono::Utc::now().timestamp();
                    now + slack_seconds >= *exp
                }
                None => false,
            },
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AuthStore {
    /// Schema version — incremented on breaking changes; the loader tolerates older versions.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Provider id → credential.
    #[serde(default)]
    pub providers: HashMap<String, ProviderCredential>,
}

fn default_version() -> u32 {
    1
}

impl AuthStore {
    pub fn load() -> Result<Self> {
        Self::load_from(&auth_path())
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let store: AuthStore =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        Ok(store)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&auth_path())
    }

    /// Atomic write: rename-temp + chmod 600. Best-effort on platforms without unix perms.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&tmp, perms).ok();
        }
        std::fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))?;
        Ok(())
    }

    pub fn set(&mut self, provider: impl Into<String>, cred: ProviderCredential) {
        self.providers.insert(provider.into(), cred);
    }

    pub fn remove(&mut self, provider: &str) -> Option<ProviderCredential> {
        self.providers.remove(provider)
    }

    pub fn get(&self, provider: &str) -> Option<&ProviderCredential> {
        self.providers.get(provider)
    }

    /// Resolve a credential for `provider`. Env var wins; auth.json is the fallback. Returns
    /// the bare API-key string for `api_key` and the access token for `oauth`.
    pub fn resolve_for_provider(&self, provider: &str) -> Option<String> {
        for env_var in theway_llm_provider::env_api_keys::env_var_names(provider) {
            if let Ok(v) = std::env::var(env_var) {
                if !v.trim().is_empty() {
                    return Some(v);
                }
            }
        }
        match self.providers.get(provider)? {
            ProviderCredential::ApiKey { value } => Some(value.clone()),
            ProviderCredential::Oauth { access_token, .. } => Some(access_token.clone()),
        }
    }
}

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
    let has_stored = AuthStore::load()
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
pub fn save_api_key(provider: &str, token: &str) -> Result<PathBuf, String> {
    let mut store = match AuthStore::load() {
        Ok(s) => s,
        Err(e) => return Err(format!("load auth store: {e}")),
    };
    store.set(
        provider.to_string(),
        ProviderCredential::ApiKey {
            value: token.to_string(),
        },
    );
    if let Err(e) = store.save() {
        return Err(format!("save auth store: {e}"));
    }
    Ok(auth_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let original = std::env::var_os(key);
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }

        fn remove(key: &'static str) -> Self {
            let original = std::env::var_os(key);
            unsafe { std::env::remove_var(key) };
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn round_trip_api_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        let mut store = AuthStore::default();
        store.set(
            "anthropic",
            ProviderCredential::ApiKey {
                value: "sk-test".into(),
            },
        );
        store.save_to(&path).unwrap();
        let reloaded = AuthStore::load_from(&path).unwrap();
        assert_eq!(reloaded.providers.len(), 1);
        match reloaded.get("anthropic").unwrap() {
            ProviderCredential::ApiKey { value } => assert_eq!(value, "sk-test"),
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn round_trip_oauth() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        let mut store = AuthStore::default();
        store.set(
            "anthropic",
            ProviderCredential::Oauth {
                access_token: "tok".into(),
                refresh_token: Some("rtok".into()),
                expires_at: Some(1_900_000_000),
                scopes: vec!["chat".into()],
            },
        );
        store.save_to(&path).unwrap();
        let reloaded = AuthStore::load_from(&path).unwrap();
        match reloaded.get("anthropic").unwrap() {
            ProviderCredential::Oauth {
                access_token,
                refresh_token,
                expires_at,
                scopes,
            } => {
                assert_eq!(access_token, "tok");
                assert_eq!(refresh_token.as_deref(), Some("rtok"));
                assert_eq!(expires_at, &Some(1_900_000_000));
                assert_eq!(scopes, &vec!["chat".to_string()]);
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn needs_refresh_evaluates_expiry_slack() {
        let now = chrono::Utc::now().timestamp();
        let expires_soon = ProviderCredential::Oauth {
            access_token: "x".into(),
            refresh_token: None,
            expires_at: Some(now + 30),
            scopes: vec![],
        };
        assert!(expires_soon.needs_refresh(60));
        assert!(!expires_soon.needs_refresh(10));
        let api_key = ProviderCredential::ApiKey { value: "x".into() };
        assert!(!api_key.needs_refresh(0));
    }

    #[test]
    fn needs_refresh_oauth_without_expiry_returns_false() {
        let cred = ProviderCredential::Oauth {
            access_token: "x".into(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
        };
        assert!(!cred.needs_refresh(0));
    }

    #[test]
    fn model_credential_hint_unknown_provider_uses_generic_env_hint() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _way = EnvGuard::set("THEWAY_DIR", dir.path());
        let hint = model_credential_hint("totally-unknown-provider").unwrap();
        assert!(hint.contains("set the provider API key env var"), "{hint}");
        assert!(hint.contains("/login totally-unknown-provider"), "{hint}");
    }

    #[test]
    fn missing_file_loads_empty_store() {
        let dir = TempDir::new().unwrap();
        let store = AuthStore::load_from(&dir.path().join("nope.json")).unwrap();
        assert!(store.providers.is_empty());
    }

    #[test]
    fn resolve_for_provider_uses_shared_provider_env_map() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _deepseek = EnvGuard::set("DEEPSEEK_API_KEY", "sk-deepseek-env");
        let _openai = EnvGuard::set("OPENAI_API_KEY", "sk-openai-should-not-count");

        let store = AuthStore::default();
        assert_eq!(
            store.resolve_for_provider("deepseek").as_deref(),
            Some("sk-deepseek-env")
        );

        drop(_deepseek);
        let _deepseek_removed = EnvGuard::remove("DEEPSEEK_API_KEY");
        assert_eq!(store.resolve_for_provider("deepseek"), None);
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_mode_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        let mut store = AuthStore::default();
        store.set("p", ProviderCredential::ApiKey { value: "k".into() });
        store.save_to(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got {:o}", mode);
    }
    #[test]
    fn auth_path_and_login_guidance_are_contract_stable() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _way = EnvGuard::set("THEWAY_DIR", dir.path());
        assert_eq!(auth_path(), dir.path().join("auth.json"));
        assert_eq!(
            login_requires_tty_message("anthropic", None),
            "/login requires an interactive terminal so the API key is not echoed; run theway in a TTY and use `/login anthropic`"
        );
        assert_eq!(
            login_requires_tty_message("anthropic", Some("theway /login anthropic")),
            "/login requires an interactive terminal so the API key is not echoed; run theway in a TTY and use `theway /login anthropic`"
        );
    }

    #[test]
    fn empty_file_loads_default_and_bad_json_is_an_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, "   \n").unwrap();
        assert!(AuthStore::load_from(&path).unwrap().providers.is_empty());

        std::fs::write(&path, "not-json").unwrap();
        assert!(AuthStore::load_from(&path).is_err());
    }

    #[test]
    fn default_version_and_store_mutations_round_trip() {
        assert_eq!(default_version(), 1);
        let mut store = AuthStore::default();
        assert!(store.get("p").is_none());
        store.set("p", ProviderCredential::ApiKey { value: "v".into() });
        assert_eq!(
            store
                .get("p")
                .map(|c| match c {
                    ProviderCredential::ApiKey { value } => value.clone(),
                    _ => String::new(),
                })
                .as_deref(),
            Some("v")
        );
        let removed = store.remove("p");
        assert!(matches!(removed, Some(ProviderCredential::ApiKey { value }) if value == "v"));
        assert!(store.get("p").is_none());
    }

    #[test]
    fn resolve_for_provider_falls_back_to_stored_api_key_and_oauth() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _openai = EnvGuard::remove("OPENAI_API_KEY");
        let mut store = AuthStore::default();
        store.set(
            "openai",
            ProviderCredential::ApiKey {
                value: "stored".into(),
            },
        );
        assert_eq!(
            store.resolve_for_provider("openai").as_deref(),
            Some("stored")
        );

        let mut store = AuthStore::default();
        store.set(
            "openai",
            ProviderCredential::Oauth {
                access_token: "oauth-token".into(),
                refresh_token: None,
                expires_at: None,
                scopes: vec![],
            },
        );
        assert_eq!(
            store.resolve_for_provider("openai").as_deref(),
            Some("oauth-token")
        );
    }

    #[test]
    fn model_credential_hint_respects_env_store_and_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _way = EnvGuard::set("THEWAY_DIR", dir.path());
        let _openai = EnvGuard::remove("OPENAI_API_KEY");

        assert!(
            model_credential_hint("openai")
                .unwrap()
                .contains("OPENAI_API_KEY"),
            "missing hint should mention env var"
        );

        let _env = EnvGuard::set("OPENAI_API_KEY", "sk-env");
        assert!(model_credential_hint("openai").is_none());

        drop(_env);
        let mut store = AuthStore::default();
        store.set(
            "openai",
            ProviderCredential::ApiKey {
                value: "sk-store".into(),
            },
        );
        store.save_to(&dir.path().join("auth.json")).unwrap();
        eprintln!(
            "auth_path={:?} loaded={:?} hint={:?}",
            auth_path(),
            AuthStore::load().map(|s| s.get("openai").cloned()),
            model_credential_hint("openai")
        );
        assert!(model_credential_hint("openai").is_none());
    }

    #[test]
    fn save_api_key_uses_configured_auth_path_and_reports_errors() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _way = EnvGuard::set("THEWAY_DIR", dir.path());
        let path = save_api_key("openai", "sk-saved").unwrap();
        assert_eq!(path, dir.path().join("auth.json"));
        let loaded = AuthStore::load().unwrap();
        assert_eq!(
            loaded.resolve_for_provider("openai").as_deref(),
            Some("sk-saved")
        );

        let file = dir.path().join("blocked");
        std::fs::write(&file, "x").unwrap();
        let _bad_way = EnvGuard::set("THEWAY_DIR", &file);
        assert!(save_api_key("openai", "sk-fail").is_err());
    }
}
