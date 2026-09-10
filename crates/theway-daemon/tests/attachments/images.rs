//! Image admission: decoded bytes land in the store, and every rejected payload keeps the
//! message the existing prompt path returns.

use theway_contract::attachments::AttachmentStore;
use theway_contract::user_input::{InputPart, InputSource, digest_bytes};
use theway_transport::images::MAX_IMAGES_PER_MESSAGE;
use theway_transport::wire::WirePromptImage;

use super::{fixture, stored_objects, write_file};

/// Base64 of the 8 PNG magic bytes — enough for mime inference.
const PNG_B64: &str = "iVBORw0KGgo=";

/// PNG magic bytes as stored by admission.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n";

fn image(data: &str, name: Option<&str>) -> WirePromptImage {
    WirePromptImage {
        data: data.to_string(),
        name: name.map(str::to_string),
    }
}

#[tokio::test]
async fn admit_stores_decoded_image_bytes_and_labels_the_part() {
    // Arrange: a data-URL payload, the shape the web client sends.
    let fixture = fixture();
    let data_url = format!("data:image/png;base64,{PNG_B64}");

    // Act
    let input = fixture
        .admission
        .admit("look", &[image(&data_url, Some("pic.png"))], InputSource::User, None)
        .await
        .unwrap();

    // Assert: the store holds the decoded bytes, named by their digest.
    let [InputPart::Image(part)] = input.parts.as_slice() else {
        panic!("expected exactly one image part: {:?}", input.parts);
    };
    assert_eq!(part.name.as_deref(), Some("pic.png"));
    assert_eq!(part.media_type, "image/png");
    assert_eq!(part.bytes, PNG_BYTES.len() as u64);
    assert_eq!(part.digest, digest_bytes(PNG_BYTES));
    assert_eq!(fixture.store.get(&part.digest).unwrap(), PNG_BYTES);
}

#[tokio::test]
async fn admit_rejects_an_unsupported_image_before_any_write() {
    // Arrange: one valid mention alongside a payload that is not an image.
    let fixture = fixture();
    write_file(&fixture, "notes.txt", "hello");

    // Act
    let error = fixture
        .admission
        .admit(
            "look at @notes.txt",
            &[image("aGVsbG8=", Some("pic.png"))],
            InputSource::User,
            None,
        )
        .await
        .unwrap_err();

    // Assert: the message is the one the prompt path already returns, and nothing was stored.
    assert_eq!(
        error,
        "unsupported image format for clipboard image `pic.png`; expected PNG/JPEG/WebP/GIF"
    );
    assert!(stored_objects(&fixture.store).is_empty());
}

#[tokio::test]
async fn admit_labels_a_nameless_image_by_its_index_in_failures() {
    // Arrange: a blank name falls back to the 1-based submission index.
    let fixture = fixture();

    // Act
    let error = fixture
        .admission
        .admit(
            "look",
            &[image(PNG_B64, Some("   ")), image("not base64!!!", None)],
            InputSource::User,
            None,
        )
        .await
        .unwrap_err();

    // Assert
    assert!(error.contains("clipboard image #2"), "{error}");
}

#[tokio::test]
async fn admit_reports_invalid_base64_with_the_decode_context() {
    // Arrange
    let fixture = fixture();

    // Act
    let error = fixture
        .admission
        .admit("look", &[image("not base64!!!", None)], InputSource::User, None)
        .await
        .unwrap_err();

    // Assert: the admission surfaces the same top-level context the pre-record intake
    // path rendered, so the client-visible `pasted image: ...` line is unchanged.
    assert!(error.starts_with("decode clipboard image #1"), "{error}");
    assert!(!error.contains("clipboard image #2"), "{error}");
}

#[tokio::test]
async fn admit_rejects_more_images_than_the_per_message_cap() {
    // Arrange: one image past the shared per-message cap.
    let fixture = fixture();
    let images: Vec<WirePromptImage> = (0..=MAX_IMAGES_PER_MESSAGE)
        .map(|_| image(PNG_B64, None))
        .collect();

    // Act
    let error = fixture
        .admission
        .admit("look", &images, InputSource::User, None)
        .await
        .unwrap_err();

    // Assert
    assert_eq!(
        error,
        format!(
            "{} images exceeds per-message cap of {}",
            MAX_IMAGES_PER_MESSAGE + 1,
            MAX_IMAGES_PER_MESSAGE
        )
    );
    assert!(stored_objects(&fixture.store).is_empty());
}
