//! Static image-model catalog. 1:1 stub of `packages/ai/src/image-models.generated.ts`.
//! **Do not edit by hand.**

use once_cell::sync::Lazy;

use crate::types::ImagesModel;

pub static BUILTIN_IMAGE_MODELS: Lazy<Vec<ImagesModel>> = Lazy::new(Vec::new);
