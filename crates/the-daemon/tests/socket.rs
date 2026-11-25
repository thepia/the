//! Integration tests for the thepiad control socket (WORK-0047 increment 2): the `ping` request/reply
//! edge and the `tail` stream edge route onto the actor core, and an oversized frame is rejected
//! without taking the daemon down.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ractor::{Actor, ActorRef};
use serde_json::json;
use the_daemon::actors::echo::EchoMsg;
use the_daemon::actors::ticker::TickerMsg;
use the_daemon::socket::{self, Conn, Core};
use the_daemon::{Echo, Ticker};
use tokio::sync::watch;

fn temp_sock(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("thepiad-it-{}-{tag}-{n}.sock", std::process::id()))
}

struct Server {
    path: PathBuf,
    sd_tx: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
    echo: ActorRef<EchoMsg>,
    ticker: ActorRef<TickerMsg>,
}

impl Server {
    async fn start(tag: &str) -> Self {
        let path = temp_sock(tag);
        let (echo, _) = Actor::spawn(None, Echo, ()).await.expect("echo");
        let (ticker, _) = Actor::spawn(None, Ticker, ()).await.expect("ticker");
        let core = Core {
            echo: echo.clone(),
            ticker: ticker.clone(),
        };
        let listener = socket::bind(&path).await.expect("bind");
        let (sd_tx, sd_rx) = watch::channel(false);
        let handle = tokio::spawn(socket::serve(listener, core, sd_rx));
        Server {
            path,
            sd_tx,
            handle,
            echo,
            ticker,
        }
    }

    async fn stop(self) {
        let _ = self.sd_tx.send(true);
        let _ = self.handle.await;
        self.echo.stop(None);
        self.ticker.stop(None);
        let _ = std::fs::remove_file(&self.path);
    }
}

#[tokio::test]
async fn ping_round_trips() {
    let srv = Server::start("ping").await;
    let mut c = Conn::connect(&srv.path).await.expect("connect");
    c.send(&json!({"op":"ping","msg":"hi"}))
        .await
        .expect("send");
    let resp = c.recv().await.expect("recv").expect("some response");
    assert_eq!(resp.get("ok").and_then(|v| v.as_str()), Some("hi"));
    srv.stop().await;
}

#[tokio::test]
async fn tail_streams_an_emitted_note() {
    let srv = Server::start("tail").await;

    let mut sub = Conn::connect(&srv.path).await.expect("connect sub");
    sub.send(&json!({"op":"subscribe"}))
        .await
        .expect("subscribe");
    // Let the subscribe register (handle_conn casts SubscribeChannel to the ticker).
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut emitter = Conn::connect(&srv.path).await.expect("connect emit");
    emitter
        .send(&json!({"op":"emit","msg":"tick"}))
        .await
        .expect("emit");
    let _ = emitter.recv().await;

    let note = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("note within 2s")
        .expect("recv ok")
        .expect("some note");
    assert_eq!(note.get("note").and_then(|v| v.as_str()), Some("tick"));

    srv.stop().await;
}

#[tokio::test]
async fn oversized_frame_is_rejected_and_server_survives() {
    let srv = Server::start("s6").await;

    // Claim a frame far larger than MAX_FRAME; the server must reject before allocating.
    let mut attacker = Conn::connect(&srv.path).await.expect("connect attacker");
    attacker
        .send_len_prefix(u32::MAX)
        .await
        .expect("send len prefix");
    let _ = attacker.recv().await; // server closes the connection
    drop(attacker);

    // The server survived: a normal ping on a fresh connection still works.
    let mut c = Conn::connect(&srv.path).await.expect("connect");
    c.send(&json!({"op":"ping","msg":"alive"}))
        .await
        .expect("send");
    let resp = c.recv().await.expect("recv").expect("some response");
    assert_eq!(resp.get("ok").and_then(|v| v.as_str()), Some("alive"));

    srv.stop().await;
}
