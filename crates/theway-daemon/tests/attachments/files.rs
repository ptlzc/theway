//! Mention admission: hit and miss, duplicate suppression, truncation, media-type guess,
//! content-addressed reuse of one mention's bytes, and store-failure reporting.

use std::sync::Arc;

use tempfile::TempDir;
use theway_contract::attachments::AttachmentStore;
use theway_contract::user_input::{InputPart, InputSource, digest_bytes};
use theway_storage::attachments::LocalAttachmentStore;
use theway_transport::mentions::MAX_MENTION_BYTES;

use super::{fixture, stored_objects, write_file};
use crate::attachments::PromptAdmission;

#[tokio::test]
async fn admit_mention_stores_the_file_and_labels_the_part() {
    // Arrange
    let fixture = fixture();
    write_file(&fixture, "notes.txt", "hello mentions");

    // Act
    let input = fixture
        .admission
        .admit("look at @notes.txt", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert: the typed text stays verbatim and the part describes the stored bytes.
    assert_eq!(input.text, "look at @notes.txt");
    let [InputPart::File(part)] = input.parts.as_slice() else {
        panic!("expected exactly one file part: {:?}", input.parts);
    };
    assert_eq!(part.path, "notes.txt");
    assert_eq!(part.name, "notes.txt");
    assert_eq!(part.media_type, "text/plain");
    assert_eq!(part.bytes, "hello mentions".len() as u64);
    assert!(!part.truncated);
    assert_eq!(part.digest, digest_bytes(b"hello mentions"));
    assert_eq!(fixture.store.get(&part.digest).unwrap(), b"hello mentions");
    assert!(input.has_attachments());
}

#[tokio::test]
async fn admit_labels_a_nested_mention_with_its_typed_path_and_leaf_name() {
    // Arrange: the mention is the cwd-relative path, the name is its final component.
    let fixture = fixture();
    write_file(&fixture, "src/foo.rs", "fn main() {}");

    // Act
    let input = fixture
        .admission
        .admit("@src/foo.rs", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert
    let [InputPart::File(part)] = input.parts.as_slice() else {
        panic!("expected exactly one file part: {:?}", input.parts);
    };
    assert_eq!(part.path, "src/foo.rs");
    assert_eq!(part.name, "foo.rs");
    assert_eq!(part.media_type, "text/plain");
    assert!(!part.truncated);
}

#[tokio::test]
async fn admit_skips_mentions_that_do_not_resolve() {
    // Arrange: one mention resolves, one names a file that is not there.
    let fixture = fixture();
    write_file(&fixture, "present.txt", "here");

    // Act
    let input = fixture
        .admission
        .admit(
            "see @missing.txt and @present.txt",
            &[],
            InputSource::User,
            None,
        )
        .await
        .unwrap();

    // Assert: the miss adds no part, an error, or an object.
    let [InputPart::File(part)] = input.parts.as_slice() else {
        panic!("only the resolvable mention yields a part: {:?}", input.parts);
    };
    assert_eq!(part.path, "present.txt");
    assert_eq!(stored_objects(&fixture.store).len(), 1);
}

#[tokio::test]
async fn admit_skips_a_mention_that_is_not_utf8_text() {
    // Arrange: mention expansion only admits files it can read as UTF-8.
    let fixture = fixture();
    std::fs::write(fixture.cwd.join("raw.bin"), [0xff, 0xfe, 0x00]).unwrap();

    // Act
    let input = fixture
        .admission
        .admit("@raw.bin", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert
    assert!(input.parts.is_empty(), "{:?}", input.parts);
    assert!(stored_objects(&fixture.store).is_empty());
}

#[tokio::test]
async fn admit_reads_a_duplicate_mention_once() {
    // Arrange: the same path twice in one text.
    let fixture = fixture();
    write_file(&fixture, "dup.txt", "once");

    // Act
    let input = fixture
        .admission
        .admit("@dup.txt and again @dup.txt", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert
    assert_eq!(input.parts.len(), 1, "{:?}", input.parts);
    assert_eq!(stored_objects(&fixture.store).len(), 1);
}

#[tokio::test]
async fn admit_reuses_one_object_for_repeated_mentions_of_one_file() {
    // Arrange: two rounds naming the same file.
    let fixture = fixture();
    write_file(&fixture, "shared.txt", "same bytes");

    // Act
    let first = fixture
        .admission
        .admit("@shared.txt", &[], InputSource::User, None)
        .await
        .unwrap();
    let second = fixture
        .admission
        .admit("again @shared.txt @shared.txt", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert: identical content addresses the same digest and a single object.
    let [InputPart::File(first_part)] = first.parts.as_slice() else {
        panic!("expected one file part: {:?}", first.parts);
    };
    let [InputPart::File(second_part)] = second.parts.as_slice() else {
        panic!("expected one file part: {:?}", second.parts);
    };
    assert_eq!(first_part.digest, second_part.digest);
    assert_eq!(stored_objects(&fixture.store).len(), 1);
}

#[tokio::test]
async fn admit_stores_the_truncated_bytes_and_flags_the_part() {
    // Arrange: a file larger than the mention window.
    let fixture = fixture();
    let content = "a".repeat(MAX_MENTION_BYTES + 512);
    write_file(&fixture, "big.txt", &content);

    // Act
    let input = fixture
        .admission
        .admit("@big.txt", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert: the stored object is the truncated text the model would have seen.
    let [InputPart::File(part)] = input.parts.as_slice() else {
        panic!("expected exactly one file part: {:?}", input.parts);
    };
    let window = "a".repeat(MAX_MENTION_BYTES);
    assert!(part.truncated);
    assert_eq!(part.bytes, MAX_MENTION_BYTES as u64);
    assert_eq!(part.digest, digest_bytes(window.as_bytes()));
    assert_eq!(fixture.store.get(&part.digest).unwrap(), window.as_bytes());
}

#[tokio::test]
async fn admit_reports_a_store_failure_as_the_attachment_error() {
    // Arrange: the object tree hangs off a regular file, so no object can be created.
    let temp = TempDir::new().unwrap();
    let blocker = temp.path().join("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();
    let object_root = LocalAttachmentStore::new(blocker.join("store"));
    let store: Arc<dyn AttachmentStore> = Arc::new(object_root);
    let admission = PromptAdmission::new(store, temp.path().to_path_buf());
    std::fs::write(temp.path().join("notes.txt"), "hello").unwrap();

    // Act
    let error = admission
        .admit("@notes.txt", &[], InputSource::User, None)
        .await
        .unwrap_err();

    // Assert
    assert!(error.starts_with("attachment io:"), "{error}");
}

#[tokio::test]
async fn admit_guesses_media_type_from_the_file_extension() {
    // Arrange: one file per guess rule, plus an extension the rule does not name.
    let fixture = fixture();
    write_file(&fixture, "doc.md", "# title");
    write_file(&fixture, "data.json", "{}");
    write_file(&fixture, "notes.unknownext", "text");

    // Act
    let input = fixture
        .admission
        .admit("@doc.md @data.json @notes.unknownext", &[], InputSource::User, None)
        .await
        .unwrap();

    // Assert
    let kinds: Vec<&str> = input
        .parts
        .iter()
        .map(|part| match part {
            InputPart::File(file) => file.media_type.as_str(),
            other => panic!("file part expected: {other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["text/markdown", "application/json", "text/plain"]);
}
