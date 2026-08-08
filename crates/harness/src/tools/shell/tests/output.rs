use super::*;

#[test]
fn output_buffer_keeps_tail_and_reports_drop() {
    // A single chunk larger than the whole cap: only its tail survives.
    let mut buf = OutputBuffer::new();
    buf.append("x".repeat(MAX_OUTPUT_BYTES + 1000));
    let (snap, dropped) = buf.snapshot();
    assert!(dropped > 0);
    assert!(snap.len() <= MAX_OUTPUT_BYTES);
    assert!(snap.ends_with('x'));

    // Many chunks: the oldest are dropped, the newest survive.
    let mut buf = OutputBuffer::new();
    for i in 0..10 {
        buf.append(format!("chunk-{i}-{}", "y".repeat(MAX_OUTPUT_BYTES / 8)));
    }
    let (snap, dropped) = buf.snapshot();
    assert!(dropped > 0);
    assert!(snap.len() <= MAX_OUTPUT_BYTES);
    assert!(snap.contains("chunk-9-"), "newest chunk must survive");
}

#[test]
fn render_reports_truncation_marker() {
    let snap = OutputSnapshot {
        version: 1,
        stdout: "tail".into(),
        stdout_dropped: 42,
        stderr: String::new(),
        stderr_dropped: 0,
        exited: false,
        exit_code: None,
    };
    let text = render_snapshot("shell-1", &snap);
    assert!(text.contains("[shell-1] running"), "got: {text}");
    assert!(text.contains("stdout:\ntail"), "got: {text}");
    assert!(text.contains("…(42 字符, 截断)"), "got: {text}");
}
