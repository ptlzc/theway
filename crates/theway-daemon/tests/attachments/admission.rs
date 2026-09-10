//! Record shape: source and source_ref, the empty-input record, part ordering, and the
//! `with_injected` append rule.

use theway_contract::user_input::{InputInjectedPart, InputPart, InputSource};
use theway_transport::wire::WirePromptImage;

use super::{fixture, write_file};
use crate::attachments::PromptAdmission;

#[tokio::test]
async fn admit_returns_an_empty_record_for_empty_text_and_no_images() {
    // Arrange: a submission with nothing to attach.
    let fixture = fixture();

    // Act
    let input = fixture
        .admission
        .admit("", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert: the record is shape-only, and empty parts stay off the wire.
    assert_eq!(input.text, "");
    assert!(input.parts.is_empty());
    assert_eq!(input.source, InputSource::User);
    assert!(input.source_ref.is_none());
    assert!(!input.has_attachments());
    let json = serde_json::to_value(&input).unwrap();
    assert!(json.get("parts").is_none(), "{json}");
}

#[tokio::test]
async fn admit_records_the_source_and_source_ref() {
    // Arrange: a non-user origin carrying its trigger id.
    let fixture = fixture();

    // Act
    let input = fixture
        .admission
        .admit("cron tick", &[], InputSource::Trigger, Some("trigger-7".into()))
        .await
        .unwrap();

    // Assert
    assert_eq!(input.source, InputSource::Trigger);
    assert_eq!(input.source_ref.as_deref(), Some("trigger-7"));
    assert!(input.parts.is_empty());
}

#[tokio::test]
async fn admit_orders_file_parts_before_image_parts() {
    // Arrange: one mention and one image in the same submission.
    let fixture = fixture();
    write_file(&fixture, "notes.txt", "hello");
    let image = WirePromptImage { data: "iVBORw0KGgo=".to_string(), name: None };

    // Act
    let input = fixture
        .admission
        .admit("see @notes.txt", &[image], InputSource::User, None)
        .await
        .unwrap();

    // Assert
    assert_eq!(input.parts.len(), 2, "{:?}", input.parts);
    assert!(matches!(input.parts.first(), Some(InputPart::File(_))), "{:?}", input.parts);
    assert!(matches!(input.parts.last(), Some(InputPart::Image(_))), "{:?}", input.parts);
}

#[tokio::test]
async fn with_injected_appends_the_part_after_every_attachment() {
    // Arrange: an admitted record that already carries a file part.
    let fixture = fixture();
    write_file(&fixture, "notes.txt", "hello");
    let input = fixture
        .admission
        .admit("see @notes.txt", &[], InputSource::User, None)
        .await
        .unwrap();
    let attached = input.parts.len();

    // Act
    let injected = PromptAdmission::with_injected(
        input,
        InputInjectedPart {
            source: "skill".to_string(),
            name: Some("review".to_string()),
            text: "skill preamble".to_string(),
        },
    );

    // Assert: injection appends, and the submitted text stays as typed.
    assert_eq!(injected.parts.len(), attached + 1);
    assert!(matches!(injected.parts.first(), Some(InputPart::File(_))));
    match injected.parts.last() {
        Some(InputPart::Injected(part)) => {
            assert_eq!(part.source, "skill");
            assert_eq!(part.name.as_deref(), Some("review"));
            assert_eq!(part.text, "skill preamble");
        }
        other => panic!("injected part expected last: {other:?}"),
    }
    assert_eq!(injected.text, "see @notes.txt");
}
