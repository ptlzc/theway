use theway_contract::extension::ExtensionDiagnosticCode;

use super::super::engine::EngineInvocationErrorKind;
use super::super::observation::diagnostic_code;

#[test]
fn diagnostic_code_maps_engine_errors() {
    assert_eq!(diagnostic_code(EngineInvocationErrorKind::Timeout), ExtensionDiagnosticCode::HookTimedOut);
    assert_eq!(diagnostic_code(EngineInvocationErrorKind::Cancelled), ExtensionDiagnosticCode::Cancelled);
    assert_eq!(diagnostic_code(EngineInvocationErrorKind::ResourceLimit), ExtensionDiagnosticCode::ResourceLimit);
    assert_eq!(diagnostic_code(EngineInvocationErrorKind::Runtime), ExtensionDiagnosticCode::HookFailed);
}

#[test]
fn queue_new_caps_capacity_at_one() {
    use std::sync::Arc;
    use super::super::observation::{ObservationJob, ObservationQueue};
    let queue = Arc::new(ObservationQueue::new(0));
    let job = ObservationJob {
        envelope: super::super::dispatcher::envelope(
            "ext",
            "sess",
            "/cwd",
            1,
            theway_contract::extension::ExtensionLifecycleEvent::Input,
            serde_json::json!({}),
        ),
        cancellation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let first = queue.enqueue(job).expect("first job starts drain");
    assert_eq!(first.envelope.event, theway_contract::extension::ExtensionLifecycleEvent::Input);
}
