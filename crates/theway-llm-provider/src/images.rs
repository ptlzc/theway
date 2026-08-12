//! Top-level images API. 1:1 stub of `packages/ai/src/images.ts`.

use crate::images_api_registry::get_images_api_provider;
use crate::types::{AssistantImages, ImagesContext, ImagesModel};

pub async fn images(
    model: &ImagesModel,
    context: &ImagesContext,
) -> Result<AssistantImages, String> {
    crate::providers::images::register_builtins::ensure();
    let entry = get_images_api_provider(&model.api)
        .ok_or_else(|| format!("No images API registered for: {}", model.api.0))?;
    entry.generate(model, context).await
}
