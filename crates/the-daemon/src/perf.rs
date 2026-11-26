//! Performance harness (WORK-0047), behind the `conformance` dev feature. Measures the two latencies
//! the spike needs real numbers for: the **in-process actor-call overhead** (routing a request through
//! the ractor layer) and the **control-socket ping round-trip** (the full edge path:
//! connect + frame + route + reply). Run with `just perf`. Prints p50/p99/max; not a pass/fail gate.

use std::sync::Arc;
use std::time::Instant;

use ractor::Actor;
use serde_json::json;
use tokio::sync::watch;

use crate::actors::echo::{Echo, EchoMsg};
use crate::actors::ticker::Ticker;
use crate::socket::{self, Conn, Core, EdgeRegistry};

fn pct(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[idx]
}

/// Measure and print the two latencies.
pub async fn run() {
    // 1) In-process actor-call overhead.
    let (echo, _) = Actor::spawn(None, Echo, ()).await.expect("echo");
    for _ in 0..1_000 {
        let _ = ractor::call!(echo, EchoMsg::Ping, "w".to_string());
    }
    let n = 50_000usize;
    let mut lat = Vec::with_capacity(n);
    for _ in 0..n {
        let s = Instant::now();
        let _ = ractor::call!(echo, EchoMsg::Ping, "x".to_string()).expect("ping");
        lat.push(s.elapsed().as_nanos());
    }
    lat.sort_unstable();
    println!(
        "in-process call!(echo, Ping)   n={n}  p50={}ns  p99={}ns  max={}ns",
        pct(&lat, 0.50),
        pct(&lat, 0.99),
        lat[lat.len() - 1],
    );
    echo.stop(None);

    // 2) Control-socket ping round-trip (connect + send + recv through the edge registry).
    let (echo, _) = Actor::spawn(None, Echo, ()).await.expect("echo");
    let (ticker, _) = Actor::spawn(None, Ticker, ()).await.expect("ticker");
    let core = Core {
        echo: echo.clone(),
        ticker: ticker.clone(),
    };
    let path = std::env::temp_dir().join(format!("thepiad-perf-{}.sock", std::process::id()));
    let listener = socket::bind(&path).await.expect("bind");
    let (sd_tx, sd_rx) = watch::channel(false);
    let registry = Arc::new(EdgeRegistry::with_builtins());
    let server = tokio::spawn(socket::serve(listener, core, registry, sd_rx));

    for _ in 0..200 {
        if let Ok(mut c) = Conn::connect(&path).await {
            let _ = c.send(&json!({ "op": "ping", "msg": "w" })).await;
            let _ = c.recv().await;
        }
    }
    let m = 5_000usize;
    let mut slat = Vec::with_capacity(m);
    for _ in 0..m {
        let s = Instant::now();
        let mut c = Conn::connect(&path).await.expect("connect");
        c.send(&json!({ "op": "ping", "msg": "x" }))
            .await
            .expect("send");
        let _ = c.recv().await.expect("recv");
        slat.push(s.elapsed().as_micros());
    }
    slat.sort_unstable();
    println!(
        "socket ping (connect+send+recv) n={m}   p50={}us  p99={}us  max={}us",
        pct(&slat, 0.50),
        pct(&slat, 0.99),
        slat[slat.len() - 1],
    );

    let _ = sd_tx.send(true);
    let _ = server.await;
    echo.stop(None);
    ticker.stop(None);
    let _ = std::fs::remove_file(&path);
}
