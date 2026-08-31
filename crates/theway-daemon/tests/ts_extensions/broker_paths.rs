use std::path::Path;

use super::super::broker_paths::{audit_path, resolve_existing_path, resolve_write_path};

#[test]
fn resolve_existing_path_resolves_relative_files_within_root() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x.txt");
    std::fs::write(&file, "hello").unwrap();

    let resolved = resolve_existing_path(dir.path(), "x.txt").unwrap();
    assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());
}

#[test]
fn resolve_existing_path_rejects_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_existing_path(dir.path(), "missing.txt").unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[test]
fn resolve_existing_path_rejects_parent_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_existing_path(dir.path(), "../outside").unwrap_err();
    assert_eq!(err.code, "path_escape");
}

#[test]
fn resolve_existing_path_rejects_absolute_path_outside_root() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("outside.txt");
    std::fs::write(&file, "x").unwrap();
    let err = resolve_existing_path(dir.path(), file.to_str().unwrap()).unwrap_err();
    assert_eq!(err.code, "path_escape");
}

#[test]
fn resolve_write_path_uses_existing_path_when_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("existing.txt");
    std::fs::write(&file, "x").unwrap();
    let resolved = resolve_write_path(dir.path(), "existing.txt").unwrap();
    assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());
}

#[test]
fn resolve_write_path_constructs_new_file_below_existing_parent() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let resolved = resolve_write_path(dir.path(), "sub/new.txt").unwrap();
    assert_eq!(resolved, std::fs::canonicalize(&sub).unwrap().join("new.txt"));
}

#[test]
fn resolve_write_path_rejects_parent_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_write_path(dir.path(), "../new.txt").unwrap_err();
    assert_eq!(err.code, "path_escape");
}

#[test]
fn resolve_write_path_rejects_missing_parent() {
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_write_path(dir.path(), "no/such/dir/file.txt").unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[test]
fn resolve_write_path_rejects_parent_that_resolves_outside_root() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("existing.txt");
    std::fs::write(&file, "x").unwrap();
    // A non-existing child of a file outside root has no canonical parent either,
    // but the path itself is absolute and outside root.
    let err = resolve_write_path(dir.path(), file.join("new.txt").to_str().unwrap()).unwrap_err();
    assert_eq!(err.code, "path_escape");
}

#[test]
fn audit_path_uses_relative_path_and_truncates_to_160_chars() {
    let root = Path::new("/tmp/root");
    let relative = Path::new("/tmp/root/a/b/c.txt");
    assert_eq!(audit_path(root, relative), "a/b/c.txt");
    let outside = Path::new("/tmp/outside");
    assert_eq!(audit_path(root, outside), "/tmp/outside");

    let long = Path::new("/tmp/root").join("a".repeat(200));
    let audited = audit_path(root, &long);
    assert_eq!(audited.chars().count(), 160);
}
