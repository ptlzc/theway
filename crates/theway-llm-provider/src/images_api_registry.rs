//! Registry for image-generation providers. 1:1 stub of
//! `packages/ai/src/images-api-registry.ts`.

use crate::types::{AssistantImages, ImagesApi, ImagesContext, ImagesModel};

#[cfg(feature = "openrouter-images")]
pub fn register_images_api_provider(
    api: String,
    _entry: crate::providers::images::openrouter::ImagesEntry,
) {
    // TODO: wire concrete provider trait once the openrouter impl materializes.
    let _ = api;
}

pub fn get_images_api_provider(_api: &ImagesApi) -> Option<ImagesEntryHandle> {
    // TODO: when providers are registered, look them up via the global registry.
    None
}

pub struct ImagesEntryHandle;

impl ImagesEntryHandle {
    pub async fn generate(
        &self,
        _model: &ImagesModel,
        _context: &ImagesContext,
    ) -> Result<AssistantImages, String> {
        Err("images registry handle is a stub".into())
    }
}
