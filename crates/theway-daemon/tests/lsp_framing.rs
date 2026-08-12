//! End-to-end test of the LSP Content-Length framing by spawning a mock server (a small
//! Python one-liner) that responds to `initialize` and pushes one diagnostic. We don't ship
//! a Python dep here; the test skips gracefully when Python isn't available, matching the
//! pattern used by the git tool tests.

use std::process::Command;
use std::time::Duration;

#[allow(dead_code)]
#[path = "../src/lsp.rs"]
mod lsp;

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

const MOCK_SERVER: &str = r#"
import sys, json
def write(obj):
    s = json.dumps(obj)
    out = sys.stdout.buffer
    out.write(f"Content-Length: {len(s)}\r\n\r\n".encode())
    out.write(s.encode())
    out.flush()
def read():
    hdrs = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.decode().rstrip("\r\n")
        if not line: break
        if ":" in line:
            k, v = line.split(":", 1)
            hdrs[k.strip()] = v.strip()
    n = int(hdrs["Content-Length"])
    body = sys.stdin.buffer.read(n)
    return json.loads(body)
while True:
    msg = read()
    if msg is None: break
    method = msg.get("method")
    if method == "initialize":
        write({"jsonrpc":"2.0","id":msg["id"],"result":{"capabilities":{}}})
    elif method == "initialized":
        # publish one diagnostic on a known uri
        write({
            "jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{
                "uri":"file:///tmp/x.rs",
                "diagnostics":[{
                    "range":{"start":{"line":3,"character":0},"end":{"line":3,"character":4}},
                    "severity":1,
                    "message":"expected `;`, found `}`",
                    "source":"mock"
                }]
            }
        })
    elif method == "shutdown":
        write({"jsonrpc":"2.0","id":msg["id"],"result":None})
    elif method == "exit":
        break
"#;

#[tokio::test]
async fn lsp_client_round_trips_initialize_and_receives_diagnostics() {
    if !python_available() {
        eprintln!("(skipped: python3 not on PATH)");
        return;
    }
    let client = lsp::LspClient::spawn("python3", &["-c", MOCK_SERVER])
        .await
        .expect("spawn mock server");
    client.initialize("file:///tmp/").await.expect("initialize");

    let received = client
        .await_diagnostics(Duration::from_secs(3))
        .await
        .expect("diagnostics arrived");
    assert_eq!(received.0, "file:///tmp/x.rs");
    assert_eq!(received.1.len(), 1);
    let d = &received.1[0];
    assert!(d.message.contains("expected"), "{}", d.message);
    assert_eq!(d.range.start.line, 3);

    client.shutdown().await;
}
