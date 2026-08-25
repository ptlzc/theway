use super::*;

#[tokio::test]
async fn find_bound_session_returns_matching_store_and_binding() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = std::fs::canonicalize(&work).unwrap();
    let binding = binding(&work, "client-1");
    let store: Arc<dyn SessionStore> =
        Arc::new(FakeSessionStore::with_binding("s1", binding.clone()));
    let repo = Arc::new(FakeSessionRepository::new(
        vec![FakeSessionRepository::record("s1")],
        vec![("s1".to_string(), store.clone())],
    )) as Arc<dyn SessionRepository>;

    // Act
    let found = find_bound_session(&repo, &work, "client-1").await.unwrap();

    // Assert
    let (found_store, found_binding) = found.unwrap();
    assert_eq!(found_binding, binding);
    assert_eq!(session_id_of(&found_store).await.unwrap(), "s1");
}

#[tokio::test]
async fn find_bound_session_returns_none_when_client_key_differs() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = std::fs::canonicalize(&work).unwrap();
    let binding = binding(&work, "client-1");
    let store: Arc<dyn SessionStore> =
        Arc::new(FakeSessionStore::with_binding("s1", binding));
    let repo = Arc::new(FakeSessionRepository::new(
        vec![FakeSessionRepository::record("s1")],
        vec![("s1".to_string(), store)],
    )) as Arc<dyn SessionRepository>;

    // Act
    let found = find_bound_session(&repo, &work, "client-2").await.unwrap();

    // Assert
    assert!(found.is_none());
}

#[tokio::test]
async fn find_bound_session_returns_none_when_work_dir_differs() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let other = dir.path().join("other");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let work = std::fs::canonicalize(&work).unwrap();
    let other = std::fs::canonicalize(&other).unwrap();
    let binding = binding(&work, "client-1");
    let store: Arc<dyn SessionStore> =
        Arc::new(FakeSessionStore::with_binding("s1", binding));
    let repo = Arc::new(FakeSessionRepository::new(
        vec![FakeSessionRepository::record("s1")],
        vec![("s1".to_string(), store)],
    )) as Arc<dyn SessionRepository>;

    // Act
    let found = find_bound_session(&repo, &other, "client-1").await.unwrap();

    // Assert
    assert!(found.is_none());
}

#[tokio::test]
async fn find_bound_session_skips_records_without_openable_store() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = std::fs::canonicalize(&work).unwrap();
    let repo = Arc::new(FakeSessionRepository::new(
        vec![FakeSessionRepository::record("missing")],
        Vec::new(),
    )) as Arc<dyn SessionRepository>;

    // Act
    let found = find_bound_session(&repo, &work, "client-1").await.unwrap();

    // Assert
    assert!(found.is_none());
}

#[tokio::test]
async fn find_bound_session_maps_list_error_to_internal() {
    // Arrange
    let repo = Arc::new(FakeSessionRepository::new(Vec::new(), Vec::new()).with_list_error("list boom"))
        as Arc<dyn SessionRepository>;

    // Act
    let err = find_bound_session(&repo, Path::new("/tmp"), "client-1")
        .await
        .err()
        .unwrap();

    // Assert
    assert_eq!(err.code, "internal");
    assert!(err.message.contains("list sessions"));
}

#[tokio::test]
async fn find_bound_session_maps_open_error_to_internal() {
    // Arrange
    let repo = Arc::new(
        FakeSessionRepository::new(
            vec![FakeSessionRepository::record("s1")],
            Vec::new(),
        )
        .with_open_error("open boom"),
    ) as Arc<dyn SessionRepository>;

    // Act
    let err = find_bound_session(&repo, Path::new("/tmp"), "client-1")
        .await
        .err()
        .unwrap();

    // Assert
    assert_eq!(err.code, "internal");
    assert!(err.message.contains("open session"));
}
