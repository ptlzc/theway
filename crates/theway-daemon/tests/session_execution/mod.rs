use super::*;
use tempfile::tempdir;
use theway_contract::session::{SessionBinding, SessionRuntimeContext};

fn binding(work_dir: &std::path::Path, client_key: &str) -> SessionBinding {
    SessionBinding {
        client_key: client_key.into(),
        runtime: SessionRuntimeContext {
            work_dir: work_dir.to_string_lossy().into_owned(),
            provider: Some("provider".into()),
            model: Some("model".into()),
            base_url: None,
            thinking: None,
        },
    }
}

fn registered_work_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    (dir, work)
}

fn registered_registry() -> (tempfile::TempDir, SessionExecutionRegistry) {
    let (dir, work) = registered_work_dir();
    let registry = SessionExecutionRegistry::new();
    registry
        .set("s1", binding(&work, "client-1"))
        .unwrap();
    (dir, registry)
}

#[test]
fn set_rejects_missing_work_dir() {
    let registry = SessionExecutionRegistry::new();
    let missing = std::path::Path::new("/definitely/missing/theway-work");

    let err = registry
        .set("s1", binding(missing, "client-1"))
        .unwrap_err();

    assert!(matches!(err, RegistryError::WorkDirMissing(_)));
}

#[test]
fn set_rejects_non_directory_work_dir() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, b"not a directory").unwrap();
    let registry = SessionExecutionRegistry::new();

    let err = registry.set("s1", binding(&file, "client-1")).unwrap_err();

    assert!(matches!(err, RegistryError::WorkDirNotDirectory(_)));
}

#[test]
fn set_rejects_empty_client_key() {
    let (_dir, work) = registered_work_dir();
    let registry = SessionExecutionRegistry::new();

    let err = registry
        .set("s1", binding(&work, "   "))
        .unwrap_err();

    assert!(matches!(err, RegistryError::EmptyClientKey));
}

#[test]
fn set_allows_same_session_same_client_and_work_dir_rebind() {
    let (_dir, work) = registered_work_dir();
    let registry = SessionExecutionRegistry::new();

    registry
        .set("s1", binding(&work, "client-1"))
        .unwrap();
    registry
        .set("s1", binding(&work, "client-1"))
        .unwrap();

    assert!(registry.get("s1").is_some());
}

#[test]
fn get_returns_canonicalized_existing_work_dir() {
    let (_dir, work) = registered_work_dir();
    let registry = SessionExecutionRegistry::new();
    let non_canonical = work.join(".");

    registry
        .set("s1", binding(&non_canonical, "client-1"))
        .unwrap();

    let bound = registry.get("s1").unwrap();
    let canonical = std::fs::canonicalize(&work).unwrap();
    assert_eq!(bound.runtime.work_dir, canonical.to_string_lossy());
}

#[test]
fn set_rejects_same_session_client_key_rebind() {
    let (dir, registry) = registered_registry();
    let work = dir.path().join("work");

    let err = registry
        .set("s1", binding(&work, "client-2"))
        .unwrap_err();

    assert!(matches!(err, RegistryError::SessionClientKeyConflict { .. }));
}

#[test]
fn set_rejects_same_session_work_dir_rebind() {
    let dir = tempdir().unwrap();
    let work_a = dir.path().join("a");
    let work_b = dir.path().join("b");
    std::fs::create_dir_all(&work_a).unwrap();
    std::fs::create_dir_all(&work_b).unwrap();
    let registry = SessionExecutionRegistry::new();
    registry
        .set("s1", binding(&work_a, "client-1"))
        .unwrap();

    let err = registry
        .set("s1", binding(&work_b, "client-1"))
        .unwrap_err();

    assert!(matches!(err, RegistryError::SessionWorkDirConflict { .. }));
}

#[test]
fn set_rejects_client_key_reused_by_another_session_in_same_cwd() {
    let (_dir, work) = registered_work_dir();
    let registry = SessionExecutionRegistry::new();
    registry
        .set("s1", binding(&work, "client-1"))
        .unwrap();

    let err = registry
        .set("s2", binding(&work, "client-1"))
        .unwrap_err();

    assert!(matches!(err, RegistryError::ClientKeyCwdConflict { .. }));
}

#[test]
fn set_credential_requires_registered_session() {
    let registry = SessionExecutionRegistry::new();

    let err = registry
        .set_credential("missing", "provider", b"secret".to_vec())
        .unwrap_err();

    assert_eq!(err, RegistryError::SessionNotRegistered("missing".into()));
}

#[test]
fn set_get_credential_round_trips_provider_scoped_bytes() {
    let (_dir, registry) = registered_registry();

    registry
        .set_credential("s1", "provider-a", b"alpha".to_vec())
        .unwrap();
    registry
        .set_credential("s1", "provider-b", b"beta".to_vec())
        .unwrap();

    let alpha = registry.get_credential("s1", "provider-a").unwrap();
    let beta = registry.get_credential("s1", "provider-b").unwrap();
    assert_eq!(&*alpha.into_zeroizing(), b"alpha");
    assert_eq!(&*beta.into_zeroizing(), b"beta");
}

#[test]
fn clear_credential_removes_only_requested_provider() {
    let (_dir, registry) = registered_registry();
    registry
        .set_credential("s1", "provider-a", b"alpha".to_vec())
        .unwrap();
    registry
        .set_credential("s1", "provider-b", b"beta".to_vec())
        .unwrap();

    assert!(registry.clear_credential("s1", "provider-a"));
    assert!(!registry.clear_credential("s1", "provider-a"));
    assert!(registry.get_credential("s1", "provider-a").is_none());
    let beta = registry.get_credential("s1", "provider-b").unwrap();
    assert_eq!(&*beta.into_zeroizing(), b"beta");
}

#[test]
fn remove_session_zeroizes_credentials_and_removes_binding() {
    let (_dir, registry) = registered_registry();
    registry
        .set_credential("s1", "provider", b"secret".to_vec())
        .unwrap();

    assert!(registry.remove("s1"));
    assert!(!registry.remove("s1"));
    assert!(registry.get("s1").is_none());
    assert!(registry.get_credential("s1", "provider").is_none());
}

#[test]
fn clear_credentials_removes_all_provider_secrets() {
    let (_dir, registry) = registered_registry();
    registry
        .set_credential("s1", "provider-a", b"alpha".to_vec())
        .unwrap();
    registry
        .set_credential("s1", "provider-b", b"beta".to_vec())
        .unwrap();

    assert!(registry.clear_credentials("s1"));
    assert!(!registry.clear_credentials("s1"));
    assert!(registry.get_credential("s1", "provider-a").is_none());
    assert!(registry.get_credential("s1", "provider-b").is_none());
    assert!(registry.get("s1").is_some(), "clearing credentials keeps binding");
}

#[test]
fn clear_all_credentials_zeroizes_every_registered_session() {
    let (_dir, registry) = registered_registry();
    registry
        .set_credential("s1", "provider-a", b"alpha".to_vec())
        .unwrap();
    registry
        .set_credential("s1", "provider-b", b"beta".to_vec())
        .unwrap();
    let (dir2, work2) = registered_work_dir();
    let _ = dir2;
    registry
        .set("s2", binding(&work2, "client-2"))
        .unwrap();
    registry
        .set_credential("s2", "provider-a", b"gamma".to_vec())
        .unwrap();

    registry.clear_all_credentials();

    assert!(registry.get_credential("s1", "provider-a").is_none());
    assert!(registry.get_credential("s1", "provider-b").is_none());
    assert!(registry.get_credential("s2", "provider-a").is_none());
    assert!(registry.get("s1").is_some());
    assert!(registry.get("s2").is_some());
}
