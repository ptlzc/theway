use super::*;

#[test]
fn decode_bytes_input_maps_markers() {
    assert_eq!(decode_bytes_input("<CR>"), "\r");
    assert_eq!(decode_bytes_input("<LF>"), "\n");
    assert_eq!(decode_bytes_input("<ESC>"), "\x1b");
    assert_eq!(decode_bytes_input("<BS>"), "\x7f");
    assert_eq!(decode_bytes_input("<C-c>"), "\x03");
    assert_eq!(decode_bytes_input("<C-d>"), "\x04");
    assert_eq!(decode_bytes_input("<C-z>"), "\x1a");
    assert_eq!(decode_bytes_input("<C-a>"), "\x01");
    assert_eq!(decode_bytes_input("<C-A>"), "\x01");
    assert_eq!(decode_bytes_input("a<C-b>c"), "a\x02c");
    // Unknown / malformed markers stay literal.
    assert_eq!(decode_bytes_input("<C-?>"), "<C-?>");
    assert_eq!(decode_bytes_input("<FOO>"), "<FOO>");
    assert_eq!(decode_bytes_input("x < y"), "x < y");
    assert_eq!(decode_bytes_input(""), "");
    assert_eq!(decode_bytes_input("plain"), "plain");
}

/// A control byte written via `bytes_input` reaches the process as data, not as a
/// signal: pipes carry bytes, and only a terminal driver turns a Ctrl-C keystroke into
/// SIGINT. `cat` echoing the byte back proves the decode (`<C-c>` → ETX) end to end.
#[cfg(unix)]
#[tokio::test]
async fn bytes_input_decodes_to_ctrl_byte() {
    let bg = run_in_background("cat").await.expect("spawn");
    let handle = registry().get(&bg.id).expect("registered");

    let result = WriteToProcessTool
        .execute(
            "w3",
            json!({ "shell_id": bg.id, "bytes_input": "<C-c>" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("write_to_process");
    assert!(
        text_of(&result).contains("Wrote 1 bytes"),
        "got: {}",
        text_of(&result)
    );

    let out = get_output_text(&handle, Some(5), &CancellationToken::new()).await;
    assert!(
        out.contains('\u{3}'),
        "expected decoded Ctrl-C byte echoed back, got: {out:?}"
    );

    KillShellTool
        .execute(
            "w4",
            json!({ "shell_id": bg.id }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("cleanup kill");
}
