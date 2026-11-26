//! The agent-config Language Server (WORK-0046 #6, ST-13) — the daemon's first editor feature.
//!
//! An LSP server that surfaces the existing `the-skills` / `the-agents` lint engines as **live
//! diagnostics** on `SKILL.md` and agent-definition files, so editing them shows findings inline in VS
//! Code. LSP is JSON-RPC 2.0 with `Content-Length` framing and is **transport-agnostic**: [`run`] works
//! over any reader/writer, [`run_stdio`] over stdio (the standard VS Code model, which also covers
//! Remote-SSH, where vscode-server spawns the server on the remote node). A daemon-hosted socket
//! transport (reachable over the mesh) is a later increment; the core here is shared.
//!
//! Increment 1: lifecycle (`initialize` / `initialized` / `shutdown` / `exit`) + `didOpen` / `didChange`
//! → `publishDiagnostics`, wired to both lint engines (SKILL.md via the-skills, agent defs via
//! the-agents). Full document sync. Deferred: hover, completion, incremental sync, code actions.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

/// Build an LSP `Diagnostic` from a lint finding's (1-based) line/col + severity.
fn diag(
    line: Option<usize>,
    col: Option<usize>,
    is_error: bool,
    code: &str,
    message: &str,
) -> Value {
    let l = line.map(|x| x.saturating_sub(1)).unwrap_or(0) as u64;
    let c = col.map(|x| x.saturating_sub(1)).unwrap_or(0) as u64;
    json!({
        "range": {
            "start": { "line": l, "character": c },
            "end": { "line": l, "character": c + 1 },
        },
        "severity": if is_error { 1 } else { 2 }, // 1 = Error, 2 = Warning
        "code": code,
        "source": "thepia",
        "message": message,
    })
}

/// Lint a document and return its LSP diagnostics. A file named `SKILL.md` goes through the-skills; any
/// other `.md` is treated as an agent definition and goes through the-agents (Claude rules).
fn diagnostics_for(path: &Path, text: &str) -> Vec<Value> {
    let parsed = the_skills::parse::parse(text);
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let mut out = Vec::new();
    if file_name.eq_ignore_ascii_case("SKILL.md") {
        // the-skills `lint` skips when frontmatter fails to parse (it is surfaced elsewhere); the LSP
        // must report it, so emit the frontmatter error itself.
        if let Some(e) = &parsed.frontmatter_error {
            out.push(diag(e.line, e.col, e.is_error(), e.code, &e.message));
        }
        let dir_name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        for p in the_skills::parse::lint(&parsed, dir_name) {
            out.push(diag(p.line, p.col, p.is_error(), p.code, &p.message));
        }
    } else {
        let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        for f in the_agents::lint::lint(
            &parsed,
            text,
            stem,
            parent,
            the_agents::assistants::claude(),
            false,
        ) {
            out.push(diag(f.line, f.col, f.is_error(), f.code, &f.message));
        }
    }
    out
}

fn uri_to_path(uri: &str) -> PathBuf {
    PathBuf::from(uri.strip_prefix("file://").unwrap_or(uri))
}

async fn read_message<R: AsyncBufRead + Unpin>(r: &mut R) -> io::Result<Option<Value>> {
    let mut content_len: usize = 0;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await? == 0 {
            return Ok(None); // EOF
        }
        let line = line.trim_end();
        if line.is_empty() {
            break; // end of headers
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            content_len = v
                .trim()
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length"))?;
        }
    }
    let mut body = vec![0u8; content_len];
    r.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body)?))
}

async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, v: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(v)?;
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    w.write_all(&body).await?;
    w.flush().await
}

async fn publish<W: AsyncWrite + Unpin>(w: &mut W, uri: &str, text: &str) -> io::Result<()> {
    let diags = diagnostics_for(&uri_to_path(uri), text);
    write_message(
        w,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": diags },
        }),
    )
    .await
}

/// Run the language server over the given reader/writer until `exit` (or EOF).
pub async fn run<R, W>(reader: R, writer: W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut r = BufReader::new(reader);
    let mut w = writer;
    while let Some(msg) = read_message(&mut r).await? {
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();
        match method {
            "initialize" => {
                let result = json!({
                    "capabilities": { "textDocumentSync": 1 }, // 1 = Full
                    "serverInfo": { "name": "thepiad-lsp", "version": env!("CARGO_PKG_VERSION") },
                });
                if let Some(id) = id {
                    write_message(
                        &mut w,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    )
                    .await?;
                }
            }
            "initialized" => {}
            "shutdown" => {
                if let Some(id) = id {
                    write_message(
                        &mut w,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null }),
                    )
                    .await?;
                }
            }
            "exit" => break,
            "textDocument/didOpen" => {
                if let Some(td) = msg.pointer("/params/textDocument") {
                    let uri = td
                        .get("uri")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    let text = td.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    publish(&mut w, &uri, text).await?;
                }
            }
            "textDocument/didChange" => {
                let uri = msg
                    .pointer("/params/textDocument/uri")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();
                // Full sync: the last content change carries the whole document.
                let text = msg
                    .pointer("/params/contentChanges")
                    .and_then(|c| c.as_array())
                    .and_then(|a| a.last())
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                publish(&mut w, &uri, &text).await?;
            }
            _ => {
                // Unknown request: a polite null result; unknown notifications are ignored.
                if let Some(id) = id {
                    write_message(
                        &mut w,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null }),
                    )
                    .await?;
                }
            }
        }
    }
    Ok(())
}

/// Run the language server over stdio (the standard VS Code model).
pub async fn run_stdio() -> io::Result<()> {
    run(tokio::io::stdin(), tokio::io::stdout()).await
}
