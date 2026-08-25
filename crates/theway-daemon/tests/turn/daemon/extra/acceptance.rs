use super::*;

const SENTINEL_A: &[u8] = b"sentinel-credential-alpha-9f2c";
const SENTINEL_B: &[u8] = b"sentinel-credential-beta-7d41";

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn tree_contains_bytes(root: &std::path::Path, needle: &[u8]) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if tree_contains_bytes(&path, needle) {
                return true;
            }
        } else if let Ok(bytes) = std::fs::read(&path) {
            if contains_bytes(&bytes, needle) {
                return true;
            }
        }
    }
    false
}

#[tokio::test]
async fn one_daemon_two_cwd_two_credentials_acceptance_scans_no_sentinel_leaks() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work_a = fixture._scratch.path().join("work").canonicalize().unwrap();
    let work_b = fixture._scratch.path().join("work-b");
    std::fs::create_dir_all(&work_b).unwrap();
    let work_b = work_b.canonicalize().unwrap();
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let scratch_path = fixture._scratch.path().to_path_buf();
    let host = fixture.host();

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: activation_request(
                "client-a",
                &work_a,
                Some(&model.provider.0),
                Some(&model.id),
            ),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let session_a = rx.await.unwrap().unwrap();
    let id_a = session_a.session.unwrap().session_id;

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::SetCredential {
            request: WireSetCredentialRequest {
                session_id: id_a.clone(),
                provider: "faux".into(),
                secret: SENTINEL_A.to_vec(),
            },
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    rx.await.unwrap().unwrap();

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: activation_request(
                "client-b",
                &work_b,
                Some(&model.provider.0),
                Some(&model.id),
            ),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let session_b = rx.await.unwrap().unwrap();
    let id_b = session_b.session.unwrap().session_id;

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::SetCredential {
            request: WireSetCredentialRequest {
                session_id: id_b.clone(),
                provider: "faux".into(),
                secret: SENTINEL_B.to_vec(),
            },
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    rx.await.unwrap().unwrap();

    // Both sessions are live in the same daemon with distinct memory-only credentials.
    let credential_a = host
        .automation
        .services
        .session_execution
        .get_credential(&id_a, "faux")
        .expect("credential A installed");
    let credential_b = host
        .automation
        .services
        .session_execution
        .get_credential(&id_b, "faux")
        .expect("credential B installed");
    assert_eq!(credential_a.as_bytes(), SENTINEL_A);
    assert_eq!(credential_b.as_bytes(), SENTINEL_B);
    assert_ne!(id_a, id_b);

    // Persisted artifacts under either cwd must never contain the sentinels.
    assert!(
        !tree_contains_bytes(&scratch_path, SENTINEL_A),
        "sentinel A leaked into persisted artifacts under {}",
        scratch_path.display()
    );
    assert!(
        !tree_contains_bytes(&scratch_path, SENTINEL_B),
        "sentinel B leaked into persisted artifacts under {}",
        scratch_path.display()
    );

    // Streamed artifacts: the authoritative snapshot and any published snapshot
    // must not contain the sentinels.
    let endpoints = host.transport_endpoints();
    let latest_json = serde_json::to_string(&*endpoints.latest.lock()).unwrap();
    assert!(!latest_json.contains(std::str::from_utf8(SENTINEL_A).unwrap()));
    assert!(!latest_json.contains(std::str::from_utf8(SENTINEL_B).unwrap()));

    let mut snapshots = endpoints.snapshot_tx.subscribe();
    host.publish_snapshot(&endpoints.latest, &endpoints.snapshot_tx, true)
        .await;
    if let Ok(update) = snapshots.try_recv() {
        let update_debug = format!("{update:?}");
        assert!(!update_debug.contains(std::str::from_utf8(SENTINEL_A).unwrap()));
        assert!(!update_debug.contains(std::str::from_utf8(SENTINEL_B).unwrap()));
    }
}
