//! Integration test for the AHP host (WORK-0046 #2): JSON-RPC over WebSocket — initialize, subscribe
//! (snapshot), and dispatchAction with host-assigned `serverSeq` fanned to every subscriber.

use ractor::{Actor, ActorRef};
use serde_json::json;
use the_daemon::ahp::{self, AhpClient, Channel, ChannelMsg};
use tokio::net::TcpListener;
use tokio::sync::watch;

async fn start() -> (
    std::net::SocketAddr,
    watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
    ActorRef<ChannelMsg>,
) {
    let (channel, _) = Actor::spawn(None, Channel, ()).await.expect("channel");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().unwrap();
    let (sd_tx, sd_rx) = watch::channel(false);
    let handle = tokio::spawn(ahp::serve_ahp(listener, channel.clone(), sd_rx));
    (addr, sd_tx, handle, channel)
}

#[tokio::test]
async fn ahp_initialize_subscribe_dispatch_fans_to_all_clients() {
    let (addr, sd_tx, handle, channel) = start().await;

    let mut a = AhpClient::connect(addr).await.expect("connect a");
    let init = a
        .request("initialize", json!({}))
        .await
        .expect("initialize");
    assert_eq!(
        init.get("protocolVersion").and_then(|v| v.as_str()),
        Some(ahp::PROTOCOL_VERSION)
    );

    let snap_a = a
        .request("subscribe", json!({ "channel": "ahp-root://" }))
        .await
        .expect("subscribe a");
    assert_eq!(snap_a.get("serverSeq").and_then(|v| v.as_u64()), Some(0));

    let mut b = AhpClient::connect(addr).await.expect("connect b");
    b.request("subscribe", json!({ "channel": "ahp-root://" }))
        .await
        .expect("subscribe b");

    // A dispatches; the host assigns serverSeq 1 and echoes the action to BOTH subscribers.
    let res = a
        .request(
            "dispatchAction",
            json!({ "channel": "ahp-root://", "action": { "op": "set", "key": "x", "value": 1 } }),
        )
        .await
        .expect("dispatch");
    assert_eq!(res.get("serverSeq").and_then(|v| v.as_u64()), Some(1));

    let act_a = a.next_action().await.expect("action a");
    let act_b = b.next_action().await.expect("action b");
    assert_eq!(act_a.get("serverSeq").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(act_b.get("serverSeq").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(act_a.pointer("/action/value"), Some(&json!(1)));
    assert_eq!(
        act_b.pointer("/action/key").and_then(|v| v.as_str()),
        Some("x")
    );

    let _ = sd_tx.send(true);
    channel.stop(None);
    let _ = handle.await;
}
