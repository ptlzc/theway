//! `~/.theway/config.toml` readers used at startup (built-in skills, trigger poll interval).

use crate::{builtin_skills, config, triggers};

/// Read `<base_dir>/config.toml` and extract the `[builtin_skills] enabled = [...]` list.
/// Missing file → empty list. Parse error / missing section → empty list (the parser itself
/// returns empty per #32's soft fail-closed posture; see
/// [`builtin_skills::parse_builtin_skills_config`]).
pub async fn read_builtin_skills_config(base_dir: &std::path::Path) -> Vec<String> {
    let path = base_dir.join("config.toml");
    let Ok(text) = tokio::fs::read_to_string(&path).await else {
        return Vec::new();
    };
    builtin_skills::parse_builtin_skills_config(&text)
}

/// Resolve the local dynamic trigger poll interval. CLI overrides config; config overrides
/// the built-in default. A malformed config reports a diagnostic but does not block startup.
pub async fn read_trigger_poll_interval_secs(
    base_dir: &std::path::Path,
    cli_override: Option<u64>,
) -> (u64, Option<String>) {
    if let Some(secs) = cli_override {
        return (secs, None);
    }

    let default = triggers::dynamic::DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS;
    let path = base_dir.join("config.toml");
    let Ok(text) = tokio::fs::read_to_string(&path).await else {
        return (default, None);
    };
    match config::parse_trigger_poll_interval_secs(&text) {
        Ok(Some(secs)) => (secs, None),
        Ok(None) => (default, None),
        Err(err) => (
            default,
            Some(format!(
                "triggers: ignoring invalid poll interval in {}: {err}",
                path.display()
            )),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trigger_poll_interval_defaults_to_ten_minutes_and_allows_overrides() {
        let temp = tempfile::tempdir().unwrap();
        let base_dir = temp.path();

        let (default_secs, diagnostic) = read_trigger_poll_interval_secs(base_dir, None).await;
        assert_eq!(
            default_secs,
            crate::triggers::dynamic::DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS
        );
        assert_eq!(default_secs, 600);
        assert!(diagnostic.is_none());

        tokio::fs::write(
            base_dir.join("config.toml"),
            "[triggers]\npoll_interval_secs = 60\n",
        )
        .await
        .unwrap();
        let (config_secs, diagnostic) = read_trigger_poll_interval_secs(base_dir, None).await;
        assert_eq!(config_secs, 60);
        assert!(diagnostic.is_none());

        let (cli_secs, diagnostic) = read_trigger_poll_interval_secs(base_dir, Some(15)).await;
        assert_eq!(cli_secs, 15);
        assert!(diagnostic.is_none());
    }
}
