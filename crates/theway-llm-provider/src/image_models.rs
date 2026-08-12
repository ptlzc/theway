//! Image-model registry. 1:1 stub of `packages/ai/src/image-models.ts`.

use crate::image_models_generated::BUILTIN_IMAGE_MODELS;
use crate::types::{ImagesModel, ImagesProvider};

pub fn get_image_model(provider: &ImagesProvider, id: &str) -> Option<ImagesModel> {
    BUILTIN_IMAGE_MODELS
        .iter()
        .find(|m| m.provider == *provider && m.id == id)
        .cloned()
}

pub fn list_image_models() -> Vec<ImagesModel> {
    BUILTIN_IMAGE_MODELS.iter().cloned().collect()
}
