// ── transport endpoints, unavailable seams, completer, mode label ───

use crate::transport::{SlashCompleter, STORAGE_OPS_UNAVAILABLE, TOOL_OPS_UNAVAILABLE};
use crate::wire::{ToolError, WireToolSkillInstallRequest, WireToolSkillSource};
use crate::{
    GraphOps, JobOps, StorageOps, ToolOps, TransportMode, UnavailableGraphOps, UnavailableJobOps,
    UnavailableStorageOps, UnavailableToolOps,
};

#[test]
fn transport_mode_label_matches_carrier() {
    assert_eq!(TransportMode::Web.label(), "web");
    assert_eq!(TransportMode::Grpc.label(), "grpc");
}

#[test]
fn transport_slash_completer_sorts_dedups_and_matches() {
    let completer = SlashCompleter::from_commands(vec![
        "/model".into(),
        "/help".into(),
        "/help".into(),
    ]);
    assert_eq!(completer.matches("/"), vec!["/help", "/model"]);
    assert_eq!(completer.matches("/mo"), vec!["/model"]);
    assert!(completer.matches("/model").is_empty());
    assert!(completer.matches("hello").is_empty());
    assert!(completer.matches("/model x").is_empty());
}

#[test]
fn transport_unavailable_job_ops_are_noops() {
    let ops = UnavailableJobOps;
    assert_eq!(ops.node_output("run", "node"), crate::wire::WireNodeOutput::default());
    assert!(!ops.interrupt_node("run", "node"));
    assert!(!ops.steer_node("run", "node", "text".into()));
}

#[test]
fn transport_unavailable_graph_ops_are_empty_or_error() {
    let ops = UnavailableGraphOps;
    ops.cancel_run("run", Some("reason"));
    assert!(ops.retry("run", Some(&["node".to_string()])).is_empty());
    assert!(!ops.skip("run", "node"));
    assert!(ops.checkpoints("session", Some("run")).unwrap().is_empty());
    assert!(ops.restore("session", "snapshot").is_err());
    assert!(ops.list("session").is_empty());
}

#[tokio::test]
async fn transport_unavailable_storage_ops_all_fail_with_same_message() {
    let ops = UnavailableStorageOps;
    let requests = [
        ops.save_dag_run(&Default::default()).await.unwrap_err().to_string(),
        ops.load_dag_runs(&Default::default()).await.unwrap_err().to_string(),
        ops.save_trigger_rules(&Default::default()).await.unwrap_err().to_string(),
        ops.load_trigger_rules(&Default::default()).await.unwrap_err().to_string(),
        ops.save_cron_jobs(&Default::default()).await.unwrap_err().to_string(),
        ops.load_cron_jobs(&Default::default()).await.unwrap_err().to_string(),
    ];
    for message in requests {
        assert!(message.contains(STORAGE_OPS_UNAVAILABLE), "{message}");
    }
}

#[tokio::test]
async fn transport_unavailable_tool_ops_all_fail_with_same_message() {
    let ops = UnavailableToolOps;
    let errs = vec![
        ops.read_file(&Default::default()).await.unwrap_err(),
        ops.write_file(&Default::default()).await.unwrap_err(),
        ops.edit_file(&Default::default()).await.unwrap_err(),
        {
            match ops.exec_command(&Default::default()).await {
                Err(e) => e,
                Ok(_) => panic!("expected exec_command to fail"),
            }
        },
        ops.list_dir(&Default::default()).await.unwrap_err(),
        ops.grep(&Default::default()).await.unwrap_err(),
        ops.find(&Default::default()).await.unwrap_err(),
        ops.memory_save(&Default::default()).await.unwrap_err(),
        ops.memory_list(&Default::default()).await.unwrap_err(),
        ops.memory_read(&Default::default()).await.unwrap_err(),
        ops.memory_forget(&Default::default()).await.unwrap_err(),
        ops.skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Url("https://example.invalid/skill.md".into()),
            confirm: false,
            overwrite: false,
        })
        .await
        .unwrap_err(),
    ];
    for err in errs {
        match err {
            ToolError::Other(e) => assert!(e.to_string().contains(TOOL_OPS_UNAVAILABLE), "{e}"),
            other => panic!("expected ToolError::Other, got {other:?}"),
        }
    }
}
