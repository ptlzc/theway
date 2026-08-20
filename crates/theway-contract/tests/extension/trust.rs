use theway_contract::extension::{
    ExtensionPermission, ExtensionTrustDecision, ExtensionTrustError, ExtensionTrustRecord,
    ExtensionTrustSubject,
};

fn package_record() -> ExtensionTrustRecord {
    ExtensionTrustRecord {
        subject: ExtensionTrustSubject::Package {
            extension_id: "deepseek-anchor".into(),
            canonical_path: "/work/.theway/extensions/deepseek-anchor".into(),
            content_sha256: "a".repeat(64),
        },
        permissions: vec![ExtensionPermission::SessionWrite],
        decision: ExtensionTrustDecision::Trusted,
        decided_at: "2026-08-20T00:00:00Z".into(),
    }
}

#[test]
fn trust_record_valid_package_round_trips() {
    let record = package_record();

    record.validate().unwrap();
    let encoded = serde_json::to_value(&record).unwrap();
    let decoded: ExtensionTrustRecord = serde_json::from_value(encoded).unwrap();

    assert_eq!(decoded, record);
}

#[test]
fn trust_record_invalid_digest_timestamp_and_duplicate_permissions_are_rejected() {
    let mut digest = package_record();
    let ExtensionTrustSubject::Package { content_sha256, .. } = &mut digest.subject else {
        unreachable!()
    };
    *content_sha256 = "NOT-A-DIGEST".into();
    assert_eq!(digest.validate(), Err(ExtensionTrustError::InvalidSubject));

    let mut timestamp = package_record();
    timestamp.decided_at = "today".into();
    assert_eq!(
        timestamp.validate(),
        Err(ExtensionTrustError::InvalidDecisionTimestamp)
    );

    let mut duplicate = package_record();
    duplicate
        .permissions
        .push(ExtensionPermission::SessionWrite);
    assert_eq!(
        duplicate.validate(),
        Err(ExtensionTrustError::DuplicatePermission(
            "session.write".into()
        ))
    );
}
