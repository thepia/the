//! The thepiad control socket (WORK-0047 increment 2) — a Unix-domain socket speaking length-prefixed
//! JSON frames, routing two edges onto the actor core: `ping` (request/reply → [`crate::actors::echo`])
//! and `tail` (`subscribe` + `emit`, streaming → [`crate::actors::ticker`]). The public thin crate uses
//! JSON frames here; the private `cli` build routes `thepia-proto` instead.
//!
//! Framing: a 4-byte big-endian length prefix then that many JSON bytes. The length is checked against
//! [`MAX_FRAME`] **before** a buffer is allocated, so one oversized frame cannot OOM the daemon
//! (break-finder S6; the daemon has no per-actor memory isolation).

use std::io;
use std::path::Path;

use ractor::ActorRef;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

use crate::actors::echo::EchoMsg;
use crate::actors::ticker::TickerMsg;

/// Largest control-socket frame accepted (1 MiB). Rejected before allocation.
pub const MAX_FRAME: usize = 1 << 20;

/// The actor refs the socket edges route onto.
#[derive(Clone)]
pub struct Core {
    pub echo: ActorRef<EchoMsg>,
    pub ticker: ActorRef<TickerMsg>,
}

fn to_io<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(e.to_string())
}

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > max {
        // S6: reject an oversized frame BEFORE allocating a buffer for it.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds max size",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(Some(body))
}

async fn write_json<W: AsyncWriteExt + Unpin>(w: &mut W, v: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(v)?;
    w.write_all(&(body.len() as u32).to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// Bind the control socket, enforcing single-instance: refuse if another daemon is already listening,
/// else clear a stale socket file and bind.
pub async fn bind(path: &Path) -> io::Result<UnixListener> {
    if UnixStream::connect(path).await.is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "another thepiad is already listening on this socket",
        ));
    }
    let _ = std::fs::remove_file(path);
    UnixListener::bind(path)
}

/// Accept connections and route each, until `shutdown` fires (graceful stop).
pub async fn serve(listener: UnixListener, core: Core, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let core = core.clone();
                        let sd = shutdown.clone();
                        tokio::spawn(async move {
                            let _ = handle_conn(stream, core, sd).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn handle_conn(
    mut stream: UnixStream,
    core: Core,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let Some(body) = read_frame(&mut stream, MAX_FRAME).await? else {
        return Ok(());
    };
    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let op = req.get("op").and_then(|v| v.as_str()).unwrap_or("");
    let msg = req
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match op {
        "ping" => {
            let reply = ractor::call!(core.echo, EchoMsg::Ping, msg).map_err(to_io)?;
            write_json(&mut stream, &json!({ "ok": reply })).await?;
        }
        "emit" => {
            ractor::call!(core.ticker, TickerMsg::Emit, msg).map_err(to_io)?;
            write_json(&mut stream, &json!({ "ok": "emitted" })).await?;
        }
        "subscribe" => {
            let (tx, mut rx) = mpsc::channel::<String>(256);
            core.ticker
                .cast(TickerMsg::SubscribeChannel(tx))
                .map_err(to_io)?;
            loop {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    note = rx.recv() => match note {
                        Some(n) => write_json(&mut stream, &json!({ "note": n })).await?,
                        None => break,
                    },
                }
            }
        }
        _ => write_json(&mut stream, &json!({ "err": "unknown op" })).await?,
    }
    Ok(())
}

/// A client connection to the control socket (used by the CLI edges, conformance, and tests).
pub struct Conn {
    stream: UnixStream,
}

impl Conn {
    pub async fn connect(path: &Path) -> io::Result<Self> {
        Ok(Self {
            stream: UnixStream::connect(path).await?,
        })
    }

    pub async fn send(&mut self, v: &Value) -> io::Result<()> {
        write_json(&mut self.stream, v).await
    }

    pub async fn recv(&mut self) -> io::Result<Option<Value>> {
        match read_frame(&mut self.stream, MAX_FRAME).await? {
            Some(b) => Ok(Some(serde_json::from_slice(&b)?)),
            None => Ok(None),
        }
    }

    /// Send only a length prefix (claiming `len` bytes) and no body — for the oversized-frame
    /// break-finder (S6).
    pub async fn send_len_prefix(&mut self, len: u32) -> io::Result<()> {
        self.stream.write_all(&len.to_be_bytes()).await?;
        self.stream.flush().await
    }
}
