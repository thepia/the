//! Minimal AHP host (WORK-0046 #2, ST-11) — Microsoft's Agent Host Protocol, increment 1.
//!
//! AHP is a "display protocol to let many interfaces operate on shared session state": JSON-RPC 2.0 over
//! WebSocket, URI-addressed **channels**, `subscribe → snapshot → ordered delta actions`, the host as
//! **arbiter** assigning a `serverSeq` and echoing one ordered action stream to every subscriber. This
//! increment implements that core over a WebSocket listener, routing a channel onto a ractor **channel
//! actor** (AHP channel = actor address): `initialize`, `subscribe` (snapshot), `dispatchAction` (host
//! assigns `serverSeq`, applies a reducer, fans the action to all subscribers). It runs alongside the unix
//! control socket on the same actor core (C7: two wire formats, one core). Deferred to later increments:
//! the full channel taxonomy (root/session/chat), capability negotiation, reconnection/replay, and the
//! client-side write-ahead reconciliation (the host side — serverSeq ordering + echo — is here).

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

/// The AHP protocol version this host advertises (ST-11 tracks v1.1.0).
pub const PROTOCOL_VERSION: &str = "1.1.0";

fn to_io<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(e.to_string())
}

// ---- the channel actor (AHP channel = actor address) ----

/// A channel holds immutable-snapshot state, a `serverSeq` cursor, and its subscribers' notification
/// senders. The host arbitrates: each `Dispatch` assigns the next `serverSeq`, applies the reducer, and
/// fans the action to every subscriber.
pub struct Channel;

pub struct ChannelState {
    state: Value,
    seq: u64,
    subs: Vec<mpsc::Sender<(u64, Value)>>,
}

pub enum ChannelMsg {
    /// Current `serverSeq`.
    Seq(RpcReplyPort<u64>),
    /// Subscribe a notification sender; reply with the current `(serverSeq, snapshot)`.
    Subscribe(mpsc::Sender<(u64, Value)>, RpcReplyPort<(u64, Value)>),
    /// Apply an action: assign the next `serverSeq`, reduce, fan to subscribers; reply the `serverSeq`.
    Dispatch(Value, RpcReplyPort<u64>),
}

fn reduce(state: &mut Value, action: &Value) {
    // Minimal reducer: `{ "op": "set", "key": K, "value": V }` sets a key. (The full AHP action
    // vocabulary — chat/responsePart, chat/delta, … — is a later increment.)
    if action.get("op").and_then(|o| o.as_str()) == Some("set")
        && let (Some(k), Some(v)) = (
            action.get("key").and_then(|k| k.as_str()),
            action.get("value"),
        )
        && let Some(obj) = state.as_object_mut()
    {
        obj.insert(k.to_string(), v.clone());
    }
}

impl Actor for Channel {
    type Msg = ChannelMsg;
    type State = ChannelState;
    type Arguments = ();

    async fn pre_start(
        &self,
        _m: ActorRef<Self::Msg>,
        _a: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(ChannelState {
            state: json!({}),
            seq: 0,
            subs: Vec::new(),
        })
    }

    async fn handle(
        &self,
        _m: ActorRef<Self::Msg>,
        msg: Self::Msg,
        st: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            ChannelMsg::Seq(reply) => {
                let _ = reply.send(st.seq);
            }
            ChannelMsg::Subscribe(tx, reply) => {
                st.subs.push(tx);
                let _ = reply.send((st.seq, st.state.clone()));
            }
            ChannelMsg::Dispatch(action, reply) => {
                st.seq += 1;
                reduce(&mut st.state, &action);
                let seq = st.seq;
                // Fan to subscribers; drop closed, shed on a full (bounded) channel.
                st.subs.retain(|s| {
                    !matches!(
                        s.try_send((seq, action.clone())),
                        Err(mpsc::error::TrySendError::Closed(_))
                    )
                });
                let _ = reply.send(seq);
            }
        }
        Ok(())
    }
}

// ---- the WebSocket / JSON-RPC host ----

async fn method(
    name: &str,
    params: &Value,
    channel: &ActorRef<ChannelMsg>,
    tx: &mpsc::Sender<(u64, Value)>,
) -> Result<Value, String> {
    match name {
        "initialize" => {
            let seq = ractor::call!(channel, ChannelMsg::Seq).map_err(|e| e.to_string())?;
            Ok(json!({ "protocolVersion": PROTOCOL_VERSION, "serverSeq": seq, "capabilities": {} }))
        }
        "subscribe" => {
            let (seq, snapshot) = ractor::call!(channel, ChannelMsg::Subscribe, tx.clone())
                .map_err(|e| e.to_string())?;
            Ok(json!({ "serverSeq": seq, "snapshot": snapshot }))
        }
        "dispatchAction" => {
            let action = params.get("action").cloned().unwrap_or(Value::Null);
            let seq =
                ractor::call!(channel, ChannelMsg::Dispatch, action).map_err(|e| e.to_string())?;
            Ok(json!({ "serverSeq": seq }))
        }
        other => Err(format!("unknown method: {other}")),
    }
}

async fn handle_ws(
    stream: TcpStream,
    channel: ActorRef<ChannelMsg>,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(to_io)?;
    let (mut write, mut read) = ws.split();
    let (tx, mut rx) = mpsc::channel::<(u64, Value)>(256);

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            inbound = read.next() => {
                let Some(Ok(msg)) = inbound else { break };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(req) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                let id = req.get("id").cloned();
                let name = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let params = req.get("params").cloned().unwrap_or(Value::Null);
                let result = method(name, &params, &channel, &tx).await;
                if let Some(id) = id {
                    let resp = match result {
                        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
                        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": e } }),
                    };
                    write.send(Message::text(resp.to_string())).await.map_err(to_io)?;
                }
            }
            action = rx.recv() => {
                let Some((seq, action)) = action else { continue };
                let note = json!({ "jsonrpc": "2.0", "method": "action", "params": { "serverSeq": seq, "action": action } });
                write.send(Message::text(note.to_string())).await.map_err(to_io)?;
            }
        }
    }
    Ok(())
}

/// Accept AHP WebSocket connections, routing each onto the channel actor, until `shutdown` fires.
pub async fn serve_ahp(
    listener: TcpListener,
    channel: ActorRef<ChannelMsg>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let ch = channel.clone();
                        let sd = shutdown.clone();
                        tokio::spawn(async move { let _ = handle_ws(stream, ch, sd).await; });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

/// A minimal AHP client (JSON-RPC over WebSocket) for tests / validation.
pub struct AhpClient {
    ws: WebSocketStream<TcpStream>,
    id: u64,
    pending: VecDeque<Value>,
}

impl AhpClient {
    pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
        let tcp = TcpStream::connect(addr).await?;
        let (ws, _) = tokio_tungstenite::client_async(format!("ws://{addr}/"), tcp)
            .await
            .map_err(to_io)?;
        Ok(Self {
            ws,
            id: 0,
            pending: VecDeque::new(),
        })
    }

    async fn recv(&mut self) -> io::Result<Value> {
        loop {
            let msg = self
                .ws
                .next()
                .await
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "ws closed"))?
                .map_err(to_io)?;
            if let Message::Text(t) = msg {
                return Ok(serde_json::from_str(t.as_str())?);
            }
        }
    }

    /// Send a JSON-RPC request and return its `result` (buffering any notifications that arrive first).
    pub async fn request(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.id += 1;
        let id = self.id;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.ws
            .send(Message::text(req.to_string()))
            .await
            .map_err(to_io)?;
        loop {
            let v = self.recv().await?;
            if v.get("id") == Some(&json!(id)) {
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            self.pending.push_back(v);
        }
    }

    /// Wait for the next server `action` notification (its `params`).
    pub async fn next_action(&mut self) -> io::Result<Value> {
        loop {
            if let Some(v) = self.pending.pop_front() {
                if v.get("method").and_then(|m| m.as_str()) == Some("action") {
                    return Ok(v.get("params").cloned().unwrap_or(Value::Null));
                }
                continue;
            }
            let v = self.recv().await?;
            self.pending.push_back(v);
        }
    }
}
