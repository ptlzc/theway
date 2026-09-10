//! Admission for one round of user input: resolve `@path` mentions, store their text and
//! every submitted image in the content-addressed attachment library, and produce the
//! canonical [`UserInput`] record that names them.
//!
//! Every byte is written before `admit` returns, so a persisted record never names content
//! that was not stored; the record itself carries digests only. A skill envelope submitted by
//! a `/skill` turn splits into the user's own text plus an injected preamble part.

mod files;
mod images;

use std::path::PathBuf;
use std::sync::Arc;

use theway_contract::attachments::{AttachmentError, AttachmentStore};
use theway_contract::user_input::{
    InputImagePart, InputInjectedPart, InputPart, InputSource, UserInput,
};
use theway_transport::commands::{skill_prompt_preamble, split_skill_prompt};
use theway_transport::wire::WirePromptImage;

pub use images::decode_prompt_images;

/// Admission for one round of user input: resolve mentions, store attachment bytes, and
/// produce the canonical record. Every write happens before the record is persisted.
pub struct PromptAdmission {
    store: Arc<dyn AttachmentStore>,
    cwd: PathBuf,
}

impl PromptAdmission {
    /// Admit input whose mentions resolve against `cwd` and whose bytes land in `store`.
    pub fn new(store: Arc<dyn AttachmentStore>, cwd: PathBuf) -> Self {
        Self { store, cwd }
    }

    /// Resolve `text`'s mentions and validate `images`, store the bytes each one carries, and
    /// return the record naming them: file parts first, then images, in submission order.
    ///
    /// Images are validated before the first write, so a rejected submission stores nothing.
    /// Mentions that do not resolve to a readable UTF-8 file are skipped silently, and a path
    /// mentioned twice yields one part.
    ///
    /// A skill envelope (`attach_skill_prompt(text, Some(name))`, the shape `/skill` turns
    /// arrive in) is split back: the record's `text` is the user's own text and the envelope
    /// preamble becomes an `Injected { source: "skill" }` part. The model-facing prompt the
    /// caller already holds is not rewritten.
    pub async fn admit(
        &self,
        text: &str,
        images: &[WirePromptImage],
        source: InputSource,
        source_ref: Option<String>,
    ) -> Result<UserInput, String> {
        let decoded = decode_prompt_images(images)?;
        let mut parts = files::parts(self.store.as_ref(), &self.cwd, text).await?;
        for (name, bytes, media_type) in decoded {
            let digest = self.store.put(&bytes).map_err(store_error)?;
            parts.push(InputPart::Image(InputImagePart {
                name,
                digest,
                bytes: bytes.len() as u64,
                media_type,
            }));
        }
        let (record_text, skill_name) = match split_skill_prompt(text) {
            Some((skill_name, user_text)) => (user_text, Some(skill_name)),
            None => (text.to_string(), None),
        };
        let input = UserInput {
            text: record_text,
            parts,
            source,
            source_ref,
        };
        let Some(name) = skill_name else {
            return Ok(input);
        };
        let preamble = skill_prompt_preamble(&name);
        let part = InputInjectedPart {
            source: "skill".to_string(),
            name: Some(name),
            text: preamble,
        };
        Ok(Self::with_injected(input, part))
    }

    /// Append one injected part (skill preamble, trigger, extension) after every attachment
    /// `input` already carries.
    pub fn with_injected(mut input: UserInput, part: InputInjectedPart) -> UserInput {
        input.parts.push(InputPart::Injected(part));
        input
    }
}

/// Admission reports store failures as the message the error already renders, so a caller can
/// surface it without knowing the storage type.
fn store_error(error: AttachmentError) -> String {
    error.to_string()
}

#[cfg(test)]
// Test files live in `tests/attachments/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("attachments");
