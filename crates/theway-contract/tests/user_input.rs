use serde_json::json;
use theway_contract::user_input::{
    DIGEST_PREFIX, InputFilePart, InputImagePart, InputInjectedPart, InputPart, InputSource,
    UserInput, digest_bytes, digest_is_valid,
};

/// sha256 of the empty input; a digest helper that mishandles empty bytes or the
/// prefix would change this constant.
const EMPTY_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// sha256 of `abc`, the reference vector for a non-empty input.
const ABC_SHA256: &str = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn digest_bytes_is_prefixed_lowercase_sha256() {
    assert_eq!(DIGEST_PREFIX, "sha256:");
    assert_eq!(digest_bytes(b""), EMPTY_SHA256);
    assert_eq!(digest_bytes(b"abc"), ABC_SHA256);
    assert!(digest_is_valid(&digest_bytes(b"theway attachment")));
    assert_ne!(digest_bytes(b"a"), digest_bytes(b"b"));
    assert!(digest_bytes(b"a").starts_with(DIGEST_PREFIX));
}

#[test]
fn digest_is_valid_rejects_malformed_digests() {
    let valid = digest_bytes(b"payload");
    let body = valid.strip_prefix(DIGEST_PREFIX).unwrap();

    assert!(digest_is_valid(&valid));
    assert!(!digest_is_valid(body));
    assert!(!digest_is_valid(""));
    assert!(!digest_is_valid("sha256"));
    assert!(!digest_is_valid(DIGEST_PREFIX));
    assert!(!digest_is_valid(&format!("{DIGEST_PREFIX}{}", &body[..63])));
    assert!(!digest_is_valid(&format!("{DIGEST_PREFIX}{body}0")));
    assert!(!digest_is_valid(&format!(
        "{DIGEST_PREFIX}{}",
        body.to_uppercase()
    )));
    assert!(!digest_is_valid(&format!(
        "{DIGEST_PREFIX}{}",
        "g".repeat(64)
    )));
    assert!(!digest_is_valid(&format!(
        "{DIGEST_PREFIX}{}x",
        "a".repeat(63)
    )));

    let uppercase_prefix = format!("{}:{body}", DIGEST_PREFIX.to_uppercase());
    assert!(!digest_is_valid(&uppercase_prefix));
}

#[test]
fn user_input_omits_empty_parts_and_keeps_the_original_text() {
    let input = UserInput::user("see @src/foo.rs");

    assert_eq!(UserInput::CUSTOM_ROLE, "user_input");
    assert_eq!(input.text, "see @src/foo.rs");
    assert_eq!(input.source, InputSource::User);
    assert_eq!(input.source_ref, None);
    assert!(input.parts.is_empty());
    assert!(!input.has_attachments());

    let encoded = serde_json::to_value(&input).unwrap();
    assert_eq!(
        encoded,
        json!({ "text": "see @src/foo.rs", "source": "user" })
    );
    assert!(encoded.get("parts").is_none());
    assert!(encoded.get("sourceRef").is_none());

    let decoded: UserInput = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, input);
    assert_eq!(serde_json::to_value(&decoded).unwrap(), encoded);
}

#[test]
fn user_input_keeps_submitted_text_verbatim() {
    let input = UserInput::user("看下 @src/foo.rs");
    let encoded = serde_json::to_value(&input).unwrap();

    assert_eq!(input.text, "看下 @src/foo.rs");
    assert_eq!(encoded["text"], "看下 @src/foo.rs");
}

#[test]
fn user_input_defaults_parts_source_and_source_ref() {
    let decoded: UserInput = serde_json::from_value(json!({ "text": "hi" })).unwrap();

    assert_eq!(decoded, UserInput::user("hi"));
    assert_eq!(decoded.source, InputSource::User);
    assert_eq!(decoded.source_ref, None);
    assert!(decoded.parts.is_empty());
}

#[test]
fn user_input_round_trips_parts_and_origin() {
    let input = UserInput {
        text: "@src/foo.rs please".into(),
        parts: vec![
            InputPart::File(InputFilePart {
                path: "src/foo.rs".into(),
                name: "foo.rs".into(),
                digest: digest_bytes(b"file body"),
                bytes: 9,
                media_type: "text/plain".into(),
                truncated: false,
            }),
            InputPart::Injected(InputInjectedPart {
                source: "trigger".into(),
                name: None,
                text: "scheduled prompt".into(),
            }),
        ],
        source: InputSource::Trigger,
        source_ref: Some("trigger-1".into()),
    };

    assert!(input.has_attachments());
    let encoded = serde_json::to_value(&input).unwrap();

    assert_eq!(encoded["source"], "trigger");
    assert_eq!(encoded["sourceRef"], "trigger-1");
    assert_eq!(encoded["parts"][0]["kind"], "file");
    assert_eq!(encoded["parts"][0]["mediaType"], "text/plain");
    assert_eq!(encoded["parts"][1]["kind"], "injected");
    assert!(encoded["parts"][1].get("name").is_none());

    let decoded: UserInput = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, input);
    assert_eq!(serde_json::to_value(&decoded).unwrap(), encoded);
}

#[test]
fn input_part_json_shapes_use_kind_tags_and_camel_case_fields() {
    let digest = digest_bytes(b"image bytes");
    let file = InputPart::File(InputFilePart {
        path: "src/foo.rs".into(),
        name: "foo.rs".into(),
        digest: digest.clone(),
        bytes: 12,
        media_type: "text/plain".into(),
        truncated: true,
    });
    let image = InputPart::Image(InputImagePart {
        name: None,
        digest: digest.clone(),
        bytes: 4096,
        media_type: "image/png".into(),
    });
    let named_image = InputPart::Image(InputImagePart {
        name: Some("shot.png".into()),
        digest: digest.clone(),
        bytes: 4096,
        media_type: "image/png".into(),
    });
    let injected = InputPart::Injected(InputInjectedPart {
        source: "skill".into(),
        name: Some("review-pr".into()),
        text: "review the diff".into(),
    });

    let file_json = json!({
        "kind": "file",
        "path": "src/foo.rs",
        "name": "foo.rs",
        "digest": digest.clone(),
        "bytes": 12,
        "mediaType": "text/plain",
        "truncated": true
    });
    assert_eq!(serde_json::to_value(&file).unwrap(), file_json);

    let image_json = json!({
        "kind": "image",
        "digest": digest.clone(),
        "bytes": 4096,
        "mediaType": "image/png"
    });
    assert_eq!(serde_json::to_value(&image).unwrap(), image_json);

    let named_image_json = json!({
        "kind": "image",
        "name": "shot.png",
        "digest": digest.clone(),
        "bytes": 4096,
        "mediaType": "image/png"
    });
    let encoded = serde_json::to_value(&named_image).unwrap();
    assert_eq!(encoded, named_image_json);

    let injected_json = json!({
        "kind": "injected",
        "source": "skill",
        "name": "review-pr",
        "text": "review the diff"
    });
    assert_eq!(serde_json::to_value(&injected).unwrap(), injected_json);

    for (part, expected) in [
        (file, file_json),
        (image, image_json),
        (named_image, named_image_json),
        (injected, injected_json),
    ] {
        let decoded: InputPart = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(decoded, part);
    }

    let missing_digest = json!({ "text": "x", "parts": [{ "kind": "file", "path": "a" }] });
    assert!(serde_json::from_value::<UserInput>(missing_digest).is_err());
}

#[test]
fn input_source_uses_snake_case_names_and_defaults_to_user() {
    for (source, wire) in [
        (InputSource::User, "user"),
        (InputSource::Trigger, "trigger"),
        (InputSource::Subagent, "subagent"),
        (InputSource::Host, "host"),
    ] {
        assert_eq!(serde_json::to_value(source).unwrap(), json!(wire));
        let decoded: InputSource = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(decoded, source);
    }

    assert_eq!(InputSource::default(), InputSource::User);
}

#[test]
fn has_attachments_counts_only_file_and_image_parts() {
    let mut input = UserInput::user("run this");
    input.parts.push(InputPart::Injected(InputInjectedPart {
        source: "extension".into(),
        name: Some("host-patch".into()),
        text: "context".into(),
    }));
    assert!(!input.has_attachments());

    input.parts.push(InputPart::File(InputFilePart {
        path: "src/foo.rs".into(),
        name: "foo.rs".into(),
        digest: digest_bytes(b"body"),
        bytes: 4,
        media_type: "text/plain".into(),
        truncated: false,
    }));
    assert!(input.has_attachments());

    let mut image_only = UserInput::user("look");
    image_only.parts.push(InputPart::Image(InputImagePart {
        name: None,
        digest: digest_bytes(b"png"),
        bytes: 3,
        media_type: "image/png".into(),
    }));
    assert!(image_only.has_attachments());
}
