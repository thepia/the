//! The thepiad control socket (WORK-0047) — a Unix-domain socket speaking length-prefixed JSON frames.
//!
//! **Edges are registered adapters, not a hand-wired `match`** (the organisation the spike proves, C2;
//! the daemon analogue of the command `Target`/`Registry`, ADR-0006/WORK-0038). Each edge is an
//! [`Edge`] with an `op` key; [`serve`] dispatches a request by **registry lookup**, so adding an edge
//! is `registry.register(Box::new(MyEdge))` with **zero change to the dispatch or the core**. The
//! builtin edges are `ping` (request/reply → [`crate::actors::echo`]) and `tail` (`subscribe` + `emit`,
//! streaming → [`crate::actors::ticker`]). The public thin crate uses JSON frames here; the private
//! `cli` build routes `thepia-proto`.
//!
//! Framing: a 4-byte big-endian length prefix then that many JSON bytes. The length is checked against
//! [`MAX_FRAME`] **before** a buffer is allocated, so one oversized frame cannot OOM the daemon
//! (break-finder S6; the daemon has no per-actor memory isolation).

use std::future::Future;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

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

// ---- the edge-adapter contract (C2) ----

/// Everything an edge needs to handle one request: the parsed request, the actor core, the connection
/// to reply on, and the shutdown signal (for long-lived streaming edges).
pub struct EdgeCtx<'a> {
    pub req: &'a Value,
    pub core: &'a Core,
    pub stream: &'a mut UnixStream,
    pub shutdown: &'a mut watch::Receiver<bool>,
}

impl EdgeCtx<'_> {
    /// A string argument from the request body (`""` if absent).
    pub fn arg(&self, key: &str) -> &str {
        self.req.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }

    /// Write one framed JSON response.
    pub async fn reply(&mut self, v: &Value) -> io::Result<()> {
        write_json(self.stream, v).await
    }
}

/// The boxed future an [`Edge::handle`] returns (so `dyn Edge` is object-safe).
pub type EdgeFuture<'a> = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;

/// A control-socket edge: a routable adapter keyed by its `op`.
pub trait Edge: Send + Sync {
    /// The request `op` this edge handles (its routing key).
    fn op(&self) -> &'static str;
    /// Handle one request on the given connection.
    fn handle<'a>(&'a self, ctx: EdgeCtx<'a>) -> EdgeFuture<'a>;
}

/// The live registry of edges. Routing is a lookup, not a `match`, so a new edge is one `register`.
pub struct EdgeRegistry {
    edges: Vec<Box<dyn Edge>>,
}

impl EdgeRegistry {
    /// The builtin edges: `ping`, `emit`, `subscribe`.
    pub fn with_builtins() -> Self {
        let mut r = Self { edges: Vec::new() };
        r.register(Box::new(PingEdge));
        r.register(Box::new(EmitEdge));
        r.register(Box::new(SubscribeEdge));
        r
    }

    /// Register an edge (drop-in; no dispatch change).
    pub fn register(&mut self, edge: Box<dyn Edge>) -> &mut Self {
        self.edges.push(edge);
        self
    }

    /// Route an `op` to its edge.
    pub fn get(&self, op: &str) -> Option<&dyn Edge> {
        self.edges.iter().map(|b| b.as_ref()).find(|e| e.op() == op)
    }

    /// The registered op set (for a drift test).
    pub fn ops(&self) -> Vec<&'static str> {
        self.edges.iter().map(|e| e.op()).collect()
    }
}

impl Default for EdgeRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

struct PingEdge;
impl Edge for PingEdge {
    fn op(&self) -> &'static str {
        "ping"
    }
    fn handle<'a>(&'a self, mut ctx: EdgeCtx<'a>) -> EdgeFuture<'a> {
        Box::pin(async move {
            let msg = ctx.arg("msg").to_string();
            let reply = ractor::call!(ctx.core.echo, EchoMsg::Ping, msg).map_err(to_io)?;
            ctx.reply(&json!({ "ok": reply })).await
        })
    }
}

struct EmitEdge;
impl Edge for EmitEdge {
    fn op(&self) -> &'static str {
        "emit"
    }
    fn handle<'a>(&'a self, mut ctx: EdgeCtx<'a>) -> EdgeFuture<'a> {
        Box::pin(async move {
            let msg = ctx.arg("msg").to_string();
            ractor::call!(ctx.core.ticker, TickerMsg::Emit, msg).map_err(to_io)?;
            ctx.reply(&json!({ "ok": "emitted" })).await
        })
    }
}

struct SubscribeEdge;
impl Edge for SubscribeEdge {
    fn op(&self) -> &'static str {
        "subscribe"
    }
    fn handle<'a>(&'a self, ctx: EdgeCtx<'a>) -> EdgeFuture<'a> {
        Box::pin(async move {
            // Destructure into disjoint locals so the stream write and the shutdown await don't
            // conflict on a borrow of `ctx`.
            let core = ctx.core;
            let stream = ctx.stream;
            let shutdown = ctx.shutdown;
            let (tx, mut rx) = mpsc::channel::<String>(256);
            core.ticker
                .cast(TickerMsg::SubscribeChannel(tx))
                .map_err(to_io)?;
            loop {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    note = rx.recv() => match note {
                        Some(n) => write_json(stream, &json!({ "note": n })).await?,
                        None => break,
                    },
                }
            }
            Ok(())
        })
    }
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

/// Accept connections and route each through the edge registry, until `shutdown` fires.
pub async fn serve(
    listener: UnixListener,
    core: Core,
    registry: Arc<EdgeRegistry>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let core = core.clone();
                        let reg = registry.clone();
                        let sd = shutdown.clone();
                        tokio::spawn(async move {
                            let _ = handle_conn(stream, core, reg, sd).await;
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
    registry: Arc<EdgeRegistry>,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let Some(body) = read_frame(&mut stream, MAX_FRAME).await? else {
        return Ok(());
    };
    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let op = req
        .get("op")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match registry.get(&op) {
        Some(edge) => {
            let ctx = EdgeCtx {
                req: &req,
                core: &core,
                stream: &mut stream,
                shutdown: &mut shutdown,
            };
            edge.handle(ctx).await?;
        }
        None => write_json(&mut stream, &json!({ "err": "unknown op" })).await?,
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
