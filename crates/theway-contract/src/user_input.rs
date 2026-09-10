//! The canonical record of one round of user input.
//!
//! A round of input is `UserInput { text, parts, source, source_ref }`: `text`
//! is the text the user submitted (`@path` tokens kept in the shape they were
//! typed), `parts` records attachments and injected content as ordered
//! structured entries. Attachment bytes live in a content-addressed store keyed
//! by [`digest_bytes`] output, so only the digest travels in the record.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `sha256:<64 lowercase hex>`, the single attachment identifier form in the
/// whole workspace.
pub const DIGEST_PREFIX: &str = "sha256:";

/// Lowercase-hex sha256 of `bytes`, prefixed with [`DIGEST_PREFIX`].
pub fn digest_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let encoded = hex::encode(digest.as_slice());
    let mut out = String::with_capacity(DIGEST_PREFIX.len() + encoded.len());
    out.push_str(DIGEST_PREFIX);
    out.push_str(&encoded);
    out
}

/// Whether `digest` is exactly [`DIGEST_PREFIX`] followed by 64 lowercase hex
/// characters.
pub fn digest_is_valid(digest: &str) -> bool {
    let Some(body) = digest.strip_prefix(DIGEST_PREFIX) else {
        return false;
    };
    body.len() == 64 && body.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Who produced a round of input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    #[default]
    User,
    Trigger,
    Subagent,
    Host,
}

/// The canonical record of one round of user input. `text` is the original text
/// the user submitted, with `@path` tokens kept in the shape the user typed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInput {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<InputPart>,
    #[serde(default)]
    pub source: InputSource,
    /// Identifier of a non-`User` origin (trigger id / subagent job id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
}

impl UserInput {
    /// The `AgentMessage::Custom` role constant; the session log and the display
    /// projection share this one string.
    pub const CUSTOM_ROLE: &'static str = "user_input";

    /// A human-authored round of input with no parts attached.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            parts: Vec::new(),
            source: InputSource::User,
            source_ref: None,
        }
    }

    /// Whether at least one part is an attachment (`File` / `Image`).
    pub fn has_attachments(&self) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part, InputPart::File(_) | InputPart::Image(_)))
    }
}

/// One ordered entry of a [`UserInput`]: an attachment stored by digest, or
/// content injected before the turn reached the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputPart {
    File(InputFilePart),
    Image(InputImagePart),
    Injected(InputInjectedPart),
}

/// A file the user referenced by path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputFilePart {
    /// The cwd-relative path the user typed, used directly as chip text.
    pub path: String,
    /// Leaf file name, used for the `<file>` block and the chip.
    pub name: String,
    pub digest: String,
    pub bytes: u64,
    pub media_type: String,
    pub truncated: bool,
}

/// An image the user submitted with the turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputImagePart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub digest: String,
    pub bytes: u64,
    pub media_type: String,
}

/// Content injected before the model saw the turn: skill preamble, extension
/// injection, or a host patch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputInjectedPart {
    /// Producer: `"skill"` | `"trigger"` | `"extension"`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub text: String,
}
