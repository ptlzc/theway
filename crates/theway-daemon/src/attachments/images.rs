//! Submitted-image admission: decode the data-URL/base64 payload, then apply the shared
//! `theway_transport::images` format and size validation.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use theway_transport::images::{MAX_IMAGES_PER_MESSAGE, load_bytes};
use theway_transport::wire::WirePromptImage;

/// Decoded, validated image bytes for one submitted image, in submission order.
///
/// Each entry is `(name, raw bytes, media type)`. Validation and its failure wording belong
/// to [`theway_transport::images::load_bytes`], so the store path and the model path reject
/// identical payloads with identical messages.
pub fn decode_prompt_images(
    images: &[WirePromptImage],
) -> Result<Vec<(Option<String>, Vec<u8>, String)>, String> {
    decode(images).map_err(|error| format!("{error}"))
}

fn decode(images: &[WirePromptImage]) -> Result<Vec<(Option<String>, Vec<u8>, String)>> {
    if images.len() > MAX_IMAGES_PER_MESSAGE {
        bail!(
            "{} images exceeds per-message cap of {}",
            images.len(),
            MAX_IMAGES_PER_MESSAGE
        );
    }
    let mut out = Vec::with_capacity(images.len());
    for (index, image) in images.iter().enumerate() {
        let name = image
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string);
        let label = match name.as_deref() {
            Some(name) => format!("clipboard image `{name}`"),
            None => format!("clipboard image #{}", index + 1),
        };
        let data = image
            .data
            .rsplit_once(',')
            .map(|(_, data)| data)
            .unwrap_or(image.data.as_str());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .with_context(|| format!("decode {label}"))?;
        // `load_bytes` owns the magic-byte and size checks; its re-encoded payload serves the
        // model path, so this store path drops it and keeps the decoded bytes.
        let decoded = load_bytes(&label, &bytes)?;
        out.push((name, bytes, decoded.mime_type));
    }
    Ok(out)
}
