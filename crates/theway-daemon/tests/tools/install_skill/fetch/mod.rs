//! Tests for `install_skill::fetch` — split out of src (see docs/rust-test-files.md).

use super::*;

#[test]
fn is_private_or_local_host_rejects_loopback_private_linklocal_and_special_names() {
    for host in [
        "localhost",
        "ip6-localhost",
        "ip6-loopback",
        "broadcasthost",
        "api.localhost",
        "api.local",
        "127.0.0.1",
        "10.0.0.1",
        "192.168.1.1",
        "172.16.0.1",
        "169.254.1.1",
        "0.0.0.0",
        "255.255.255.255",
        "::1",
        "fc00::1",
    ] {
        assert!(
            is_private_or_local_host(host),
            "host {host} should be rejected"
        );
    }
}

#[test]
fn is_private_or_local_host_allows_public_hostnames_and_ips() {
    for host in ["example.com", "1.2.3.4", "8.8.8.8"] {
        assert!(
            !is_private_or_local_host(host),
            "host {host} should be allowed"
        );
    }
}

#[tokio::test]
async fn fetch_inline_returns_content() {
    let source = Source::Content {
        content: "hello\n".into(),
    };
    let fetched = fetch_source(&source, &CancellationToken::new())
        .await
        .expect("inline fetch should succeed");
    assert_eq!(fetched.content, "hello\n");
}

#[tokio::test]
async fn fetch_source_url_and_path_delegate_to_specific_fetchers() {
    let cancel = CancellationToken::new();
    let bad_scheme = fetch_source(
        &Source::Url {
            url: "http://example.com/skill.md".into(),
        },
        &cancel,
    )
    .await
    .err()
    .expect("http must be refused");
    assert!(bad_scheme.to_string().contains("https"), "got: {bad_scheme}");

    let relative = fetch_source(
        &Source::Path {
            path: "relative.md".into(),
        },
        &cancel,
    )
    .await
    .err()
    .expect("relative path must be refused");
    assert!(
        relative.to_string().contains("path must be absolute"),
        "got: {relative}"
    );
}

#[tokio::test]
async fn fetch_path_reads_utf8_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("SKILL.md");
    tokio::fs::write(&path, "---\nname: alpha\ndescription: d\n---\nbody\n")
        .await
        .unwrap();

    let fetched = fetch_source(
        &Source::Path {
            path: path.to_string_lossy().into_owned(),
        },
        &CancellationToken::new(),
    )
    .await
    .expect("path fetch should succeed");

    assert_eq!(fetched.content, "---\nname: alpha\ndescription: d\n---\nbody\n");
}

#[tokio::test]
async fn fetch_path_rejects_missing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.md");
    let err = fetch_source(
        &Source::Path {
            path: missing.to_string_lossy().into_owned(),
        },
        &CancellationToken::new(),
    )
    .await
    .err()
    .expect("missing file must fail");
    assert!(err.to_string().contains("stat"), "got: {err}");
}

#[tokio::test]
async fn fetch_path_rejects_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = fetch_source(
        &Source::Path {
            path: dir.path().to_string_lossy().into_owned(),
        },
        &CancellationToken::new(),
    )
    .await
    .err()
    .expect("directory must fail");
    assert!(
        err.to_string().contains("not a regular file"),
        "got: {err}"
    );
}

#[tokio::test]
async fn fetch_path_rejects_oversized_file_before_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.md");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(SKILL_FETCH_OOM_GUARD_BYTES as u64 + 1)
        .unwrap();
    let err = fetch_source(
        &Source::Path {
            path: path.to_string_lossy().into_owned(),
        },
        &CancellationToken::new(),
    )
    .await
    .err()
    .expect("oversized file must fail");
    assert!(
        err.to_string().contains("exceeds")
            && err.to_string().contains("in-memory guard"),
        "got: {err}"
    );
}

#[tokio::test]
async fn fetch_url_rejects_invalid_url() {
    let err = fetch_url("not a url", &CancellationToken::new())
        .await
        .err()
        .expect("invalid url must fail");
    assert!(err.to_string().contains("invalid url"), "got: {err}");
}

#[tokio::test]
async fn fetch_url_rejects_non_https_scheme() {
    let err = fetch_url("http://example.com/skill.md", &CancellationToken::new())
        .await
        .err()
        .expect("http must fail");
    assert!(err.to_string().contains("https"), "got: {err}");
}

#[tokio::test]
async fn fetch_url_rejects_private_host() {
    let err = fetch_url("https://127.0.0.1/skill.md", &CancellationToken::new())
        .await
        .err()
        .expect("loopback must fail");
    assert!(err.to_string().contains("SSRF guard"), "got: {err}");
}
