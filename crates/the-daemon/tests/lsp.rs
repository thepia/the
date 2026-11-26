//! Integration test for the agent-config language server (WORK-0046 #6): drive it over an in-memory
//! duplex — initialize, didOpen a malformed SKILL.md, and assert `publishDiagnostics` reports findings.

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

async fn write_msg<W: AsyncWriteExt + Unpin>(w: &mut W, v: &Value) {
    let body = serde_json::to_vec(v).unwrap();
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .unwrap();
    w.write_all(&body).await.unwrap();
    w.flush().await.unwrap();
}

async fn read_msg<R: AsyncBufReadExt + Unpin>(r: &mut R) -> Value {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        r.read_line(&mut line).await.unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse().unwrap();
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn lsp_publishes_diagnostics_for_a_malformed_skill() {
    let (client, server) = tokio::io::duplex(1 << 16);
    let (sr, sw) = tokio::io::split(server);
    let handle = tokio::spawn(the_daemon::lsp::run(sr, sw));

    let (cr, mut cw) = tokio::io::split(client);
    let mut cr = BufReader::new(cr);

    write_msg(
        &mut cw,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
    )
    .await;
    let init = read_msg(&mut cr).await;
    assert_eq!(init.get("id").and_then(|v| v.as_u64()), Some(1));
    assert!(
        init.pointer("/result/capabilities/textDocumentSync")
            .is_some()
    );

    // A SKILL.md with no YAML frontmatter is a lint error.
    write_msg(
        &mut cw,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": "file:///tmp/demo/SKILL.md", "text": "just prose, no frontmatter" } },
        }),
    )
    .await;
    let note = read_msg(&mut cr).await;
    assert_eq!(
        note.get("method").and_then(|m| m.as_str()),
        Some("textDocument/publishDiagnostics")
    );
    let diags = note
        .pointer("/params/diagnostics")
        .and_then(|d| d.as_array())
        .expect("diagnostics array");
    assert!(!diags.is_empty(), "expected at least one diagnostic");

    write_msg(
        &mut cw,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
    )
    .await;
    let _ = read_msg(&mut cr).await;
    write_msg(
        &mut cw,
        &json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    )
    .await;

    drop(cw);
    let _ = handle.await;
}
