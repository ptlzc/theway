//! The `/skill` turn envelope at admission: the command layer wraps the user's text with
//! `attach_skill_prompt`, and `PromptAdmission::admit` records the user's own text plus the
//! envelope preamble as an injected part while the model-facing prompt stays untouched.
//!
//! Lives in the `commands` suite because the envelope's producer is the `/skill` shortcut;
//! the attachment-admission suite under `tests/attachments/` owns the mention/image cases.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use theway_contract::attachments::AttachmentStore;
use theway_contract::user_input::{InputPart, InputSource};
use theway_storage::attachments::LocalAttachmentStore;
use theway_transport::commands::{attach_skill_prompt, skill_prompt_preamble, split_skill_prompt};

use crate::attachments::PromptAdmission;

/// One temp root holding a `work/` cwd and the content-addressed `store/` object tree.
struct Fixture {
    _temp: TempDir,
    cwd: PathBuf,
    admission: PromptAdmission,
}

/// Admission over a private temp cwd and attachment store.
fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let cwd = temp.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let store: Arc<dyn AttachmentStore> =
        Arc::new(LocalAttachmentStore::new(temp.path().join("store")));
    let admission = PromptAdmission::new(store, cwd.clone());
    Fixture {
        _temp: temp,
        cwd,
        admission,
    }
}

#[tokio::test]
async fn admit_records_a_skill_envelope_as_user_text_plus_the_injected_preamble() {
    // Arrange: the text the daemon receives for `/review-pr summarize the diff`.
    let fixture = fixture();
    let prompt = attach_skill_prompt("summarize the diff", Some("review-pr"));

    // Act
    let input = fixture
        .admission
        .admit(&prompt, &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert: the record carries the user's own text and the preamble as injected content.
    assert_eq!(input.text, "summarize the diff");
    assert_eq!(input.source, InputSource::User);
    assert!(input.source_ref.is_none());
    assert_eq!(input.parts.len(), 1, "{:?}", input.parts);
    match input.parts.first() {
        Some(InputPart::Injected(part)) => {
            assert_eq!(part.source, "skill");
            assert_eq!(part.name.as_deref(), Some("review-pr"));
            assert_eq!(part.text, skill_prompt_preamble("review-pr"));
        }
        other => panic!("injected part expected first: {other:?}"),
    }

    // The model-facing prompt is the one the caller already holds, unchanged.
    let preamble = skill_prompt_preamble("review-pr");
    assert_eq!(prompt, format!("{preamble}{}", input.text));
    assert!(prompt.starts_with(&preamble));
    let split = split_skill_prompt(&prompt);
    assert_eq!(split, Some(("review-pr".to_string(), input.text.clone())));
}

#[tokio::test]
async fn admit_keeps_mention_parts_before_the_injected_preamble_inside_an_envelope() {
    // Arrange: an envelope whose user text mentions a file.
    let fixture = fixture();
    std::fs::write(fixture.cwd.join("notes.txt"), "hello").unwrap();
    let prompt = attach_skill_prompt("see @notes.txt", Some("review-pr"));

    // Act
    let input = fixture
        .admission
        .admit(&prompt, &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert: the mention still resolves once, and injection appends after the attachment.
    assert_eq!(input.text, "see @notes.txt");
    assert_eq!(input.parts.len(), 2, "{:?}", input.parts);
    assert!(matches!(input.parts.first(), Some(InputPart::File(_))), "{:?}", input.parts);
    match input.parts.last() {
        Some(InputPart::Injected(part)) => {
            assert_eq!(part.name.as_deref(), Some("review-pr"));
            assert_eq!(part.text, skill_prompt_preamble("review-pr"));
        }
        other => panic!("injected part expected last: {other:?}"),
    }
}

#[tokio::test]
async fn admit_leaves_a_plain_prompt_as_the_record_text() {
    // Arrange: a submission that is not a skill envelope, mentioning an unresolvable path.
    let fixture = fixture();

    // Act
    let input = fixture
        .admission
        .admit("look at @notes.txt", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert: the text is recorded as submitted and nothing is injected (the skipped mention
    // leaves no part either).
    assert_eq!(input.text, "look at @notes.txt");
    assert!(input.parts.is_empty(), "{:?}", input.parts);
}
