//! `@path` mention admission: resolve each mention against the session cwd, keep the
//! truncation window the model would see, store those bytes, and describe them as an
//! [`InputFilePart`].

use std::collections::HashSet;
use std::path::Path;

use theway_contract::attachments::AttachmentStore;
use theway_contract::user_input::{InputFilePart, InputPart};
use theway_transport::mentions::{mentions, truncate_mention};

use super::store_error;

/// One part per distinct mention that resolves to a readable UTF-8 file, in the order the
/// text mentions them.
pub(super) async fn parts(
    store: &dyn AttachmentStore,
    cwd: &Path,
    text: &str,
) -> Result<Vec<InputPart>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for relative in mentions(text) {
        if !seen.insert(relative.clone()) {
            continue;
        }
        if let Some(part) = file_part(store, cwd, &relative).await? {
            out.push(part);
        }
    }
    Ok(out)
}

/// The part for one mention, or `None` when the path is missing, unreadable, or not UTF-8 —
/// the same silent skip the prompt-expansion path applies.
async fn file_part(
    store: &dyn AttachmentStore,
    cwd: &Path,
    relative: &str,
) -> Result<Option<InputPart>, String> {
    let Ok(text) = tokio::fs::read_to_string(cwd.join(relative)).await else {
        return Ok(None);
    };
    let (body, truncated) = truncate_mention(&text);
    let digest = store.put(body.as_bytes()).map_err(store_error)?;
    Ok(Some(InputPart::File(InputFilePart {
        path: relative.to_string(),
        name: leaf_name(relative),
        digest,
        bytes: body.len() as u64,
        media_type: media_type(relative).to_string(),
        truncated,
    })))
}

/// Leaf file name of a mention, used for the `<file>` block and the chip. A mention with no
/// final component (a trailing separator) keeps its typed text.
fn leaf_name(relative: &str) -> String {
    match Path::new(relative).file_name() {
        Some(name) if !name.is_empty() => name.to_string_lossy().into_owned(),
        _ => relative.to_string(),
    }
}

/// Extension-based media type for a stored mention. Every admitted file is UTF-8 text, so
/// `text/plain` is the fallback; these extensions name the type they carry instead.
fn media_type(relative: &str) -> &'static str {
    let Some(extension) = Path::new(relative).extension() else {
        return "text/plain";
    };
    match extension.to_string_lossy().to_ascii_lowercase().as_str() {
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "xml" => "text/xml",
        _ => "text/plain",
    }
}
