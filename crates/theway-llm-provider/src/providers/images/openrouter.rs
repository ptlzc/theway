//! TODO: 1:1 port of `packages/ai/src/providers/images/openrouter.ts`.

use crate::types::ImagesModel;

#[derive(Copy, Clone)]
pub struct ImagesEntry;

impl ImagesEntry {
    pub async fn generate(&self, _model: &ImagesModel) -> Result<(), String> {
        Err("openrouter-images not yet implemented".into())
    }
}
