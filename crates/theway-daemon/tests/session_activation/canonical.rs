use super::*;

#[test]
fn canonical_work_dir_accepts_existing_directory() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();

    // Act
    let canonical = canonical_work_dir(&dir.path().to_string_lossy()).unwrap();

    // Assert
    assert_eq!(canonical, std::fs::canonicalize(dir.path()).unwrap());
}

#[test]
fn canonical_work_dir_rejects_missing_directory() {
    // Arrange
    let missing = "/definitely/missing/theway-work";

    // Act
    let err = canonical_work_dir(missing).unwrap_err();

    // Assert
    assert_eq!(err.code, "invalid_argument");
    assert!(err.message.contains("does not exist"));
}

#[test]
fn canonical_work_dir_rejects_file_path() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, b"not a directory").unwrap();

    // Act
    let err = canonical_work_dir(&file.to_string_lossy()).unwrap_err();

    // Assert
    assert_eq!(err.code, "invalid_argument");
    assert!(err.message.contains("not a directory"));
}

#[test]
fn same_canonical_work_dir_matches_equivalent_paths() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    let stored = canonical.join(".").to_string_lossy().into_owned();

    // Act
    let matches = same_canonical_work_dir(&stored, &canonical);

    // Assert
    assert!(matches);
}

#[test]
fn same_canonical_work_dir_rejects_different_existing_paths() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let a = std::fs::canonicalize(&a).unwrap();

    // Act
    let matches = same_canonical_work_dir(&b.to_string_lossy(), &a);

    // Assert
    assert!(!matches);
}

#[test]
fn same_canonical_work_dir_returns_false_for_missing_stored_path() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    let missing = canonical.join("missing").to_string_lossy().into_owned();

    // Act
    let matches = same_canonical_work_dir(&missing, &canonical);

    // Assert
    assert!(!matches);
}
