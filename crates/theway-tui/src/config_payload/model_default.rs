use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail};
use theway_transport::config::ModelDefault;
use toml_edit::{DocumentMut, Item, Table, value};

static CONFIG_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Persist a model pair into the controller-owned `config.toml` without
/// disturbing unrelated sections or comments. Syntax-invalid files are left
/// byte-for-byte unchanged, and readers observe either the old complete file
/// or the new complete file through a same-directory temporary rename.
pub(crate) fn persist_model_default(path: &Path, default: &ModelDefault) -> Result<()> {
    if default.provider.trim().is_empty() || default.model.trim().is_empty() {
        bail!("provider and model must both be non-empty");
    }

    let original = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", path.display()));
        }
    };
    let mut document = original
        .parse::<DocumentMut>()
        .with_context(|| format!("parse {}", path.display()))?;
    let model = document
        .as_table_mut()
        .entry("model")
        .or_insert(Item::Table(Table::new()));
    let model = model
        .as_table_like_mut()
        .with_context(|| format!("`model` in {} must be a table", path.display()))?;
    model.insert("provider", value(&default.provider));
    model.insert("model", value(&default.model));

    atomic_write(path, document.to_string().as_bytes())
        .with_context(|| format!("write {}", path.display()))
}

fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    fs::create_dir_all(parent.unwrap_or_else(|| Path::new(".")))?;

    let target = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(path)?,
        Ok(_) => path.to_path_buf(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error),
    };
    let file_name = target.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", target.display()),
        )
    })?;
    let tmp_path = target.with_file_name(format!(
        ".{}.theway-tmp-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        CONFIG_TMP_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&tmp_path)?;
        if let Ok(metadata) = fs::metadata(&target) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&tmp_path, &target)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_result
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("config_payload/model_default");
