//! Tests for `attachments` — split out of src (see docs/rust-test-files.md).
//!
//! Admission is the write-before-record boundary: every mention and image byte is stored
//! under its content digest before the record names it, mentions that do not resolve are
//! skipped, and image validation keeps the wording the prompt path already returns.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use theway_contract::attachments::AttachmentStore;
use theway_storage::attachments::LocalAttachmentStore;

use super::PromptAdmission;

mod admission;
mod files;
mod images;

/// One temp root holding a `work/` cwd and the content-addressed `store/` object tree.
struct Fixture {
    _temp: TempDir,
    cwd: PathBuf,
    store: LocalAttachmentStore,
    admission: PromptAdmission,
}

/// Admission over a private temp cwd and attachment store.
fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let cwd = temp.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let store = LocalAttachmentStore::new(temp.path().join("store"));
    let attachments: Arc<dyn AttachmentStore> = Arc::new(store.clone());
    let admission = PromptAdmission::new(attachments, cwd.clone());
    Fixture {
        _temp: temp,
        cwd,
        store,
        admission,
    }
}

/// Write `content` into the fixture cwd (creating parents) and return the file path.
fn write_file(fixture: &Fixture, name: &str, content: &str) -> PathBuf {
    let path = fixture.cwd.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path
}

/// Every object file under the store root, at any shard depth.
fn stored_objects(store: &LocalAttachmentStore) -> Vec<PathBuf> {
    let mut objects = Vec::new();
    let mut pending = vec![store.root().to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                objects.push(path);
            }
        }
    }
    objects
}

#[test]
fn daemon_services_root_the_attachment_library_under_the_theway_base() {
    // Arrange: a base dir the caller resolved from `DaemonPaths`.
    let base = TempDir::new().unwrap();

    // Act
    let services = crate::orchestration::DaemonServices::new().with_attachments_base(base.path());

    // Assert: the process-wide library lives at `<base>/attachments/v1`.
    assert_eq!(services.attachments.root(), base.path().join("attachments").join("v1"));
}
