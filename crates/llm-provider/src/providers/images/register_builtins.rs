//! 1:1 stub of `packages/ai/src/providers/images/register-builtins.ts`. Registers all enabled
//! image providers in the global images registry on first use.

use std::sync::OnceLock;

static ENSURED: OnceLock<()> = OnceLock::new();

pub fn ensure() {
    ENSURED.get_or_init(|| {
        #[cfg(feature = "openrouter-images")]
        crate::images_api_registry::register_images_api_provider(
            "openrouter-images".into(),
            crate::providers::images::openrouter::ImagesEntry,
        );
    });
}
