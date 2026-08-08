//! Unit tests for HTTP transport helpers: bind policy, feed-line projection, and prompt
//! image decoding.

use super::super::*;

#[test]
fn bind_addr_rejects_remote_by_default() {
    let err = bind_addr("0.0.0.0", 0).unwrap_err().to_string();
    assert!(err.contains("refusing non-loopback"));
}

#[test]
fn bind_addr_accepts_loopback_and_localhost() {
    let local = bind_addr("127.0.0.1", 0).unwrap();
    assert!(local.ip().is_loopback());

    let named = bind_addr("localhost", 0).unwrap();
    assert!(named.ip().is_loopback());
}

#[test]
fn web_feed_lines_keeps_all_rows() {
    let mut feed = feed::Feed::new();
    for i in 0..250 {
        feed.apply(feed::FeedUpdate::Plain {
            text: format!("line {i}"),
            level: feed::Level::Output,
        });
    }

    let lines = web_feed_lines(&feed);
    assert_eq!(lines.len(), 250);
    assert!(
        lines.first().is_some_and(|line| line.contains("line 0")),
        "{lines:?}"
    );
    assert!(
        lines.last().is_some_and(|line| line.contains("line 249")),
        "{lines:?}"
    );
    let first = lines.first().expect("first line");
    assert_eq!(first.chars().nth(4), Some('-'), "{lines:?}");
    assert_eq!(first.chars().nth(7), Some('-'), "{lines:?}");
    assert_eq!(first.chars().nth(10), Some(' '), "{lines:?}");
    assert_eq!(first.chars().nth(13), Some(':'), "{lines:?}");
}

#[test]
fn web_prompt_images_decode_to_image_content() {
    let data = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\npng");
    let images = load_web_prompt_images(&[WebPromptImage {
        data,
        name: Some("clip.png".into()),
    }])
    .unwrap();

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime_type, "image/png");
    assert!(!images[0].data.is_empty());
}

#[test]
fn web_prompt_images_enforce_count_limit() {
    let images = vec![
        WebPromptImage {
            data: String::new(),
            name: None,
        };
        crate::images::MAX_IMAGES_PER_MESSAGE + 1
    ];
    let err = load_web_prompt_images(&images).unwrap_err().to_string();
    assert!(err.contains("exceeds per-message cap"), "{err}");
}
