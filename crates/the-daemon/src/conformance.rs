//! Conformance self-report (WORK-0047), behind the `conformance` cargo feature (a debug/dev compile
//! flag, never in a release build). It exercises the daemon's own core actors and emits a per-claim
//! report (`agent/vectors/report.schema.json` in thepia-app-spec) that the spec repo's
//! `cadence/check-daemon-conformance.py` reconciles against its claim manifest.
//!
//! Covers, as of increment 2 (control socket): **C3** (request/reply + stream + cancel), **C4** (crash
//! isolation + supervision), and break-finders **S1** (flood isolation), **S2** (bounded mailbox),
//! **S3** (blocking-handler detection), **S4** (restart-storm bound), **S6** (oversized-frame guard on
//! the control socket). The remaining claims are omitted, so the reconciler reports them `pending`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use serde_json::{Map, Value, json};

use crate::actors::echo::{Echo, EchoMsg};
use crate::actors::faulty::FaultyMsg;
use crate::actors::hub::{Hub, HubMsg, MAX_RESTARTS};
use crate::actors::slow::{Slow, SlowMsg, SlowOutcome};
use crate::actors::ticker::{Ticker, TickerMsg};
use crate::socket::{self, Conn, Core, Edge, EdgeCtx, EdgeFuture, EdgeRegistry};
use crate::watchdog::StallMonitor;
use tokio::sync::watch;

// A tiny subscriber used to observe the stream verb.
struct Collector;
enum CollectorMsg {
    Note(String),
    Drain(RpcReplyPort<Vec<String>>),
}
impl Actor for Collector {
    type Msg = CollectorMsg;
    type State = Vec<String>;
    type Arguments = ();
    async fn pre_start(
        &self,
        _m: ActorRef<Self::Msg>,
        _a: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(Vec::new())
    }
    async fn handle(
        &self,
        _m: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            CollectorMsg::Note(s) => state.push(s),
            CollectorMsg::Drain(reply) => {
                let _ = reply.send(state.clone());
            }
        }
        Ok(())
    }
}

// A fire-and-forget sink used to flood one actor's mailbox (break-finder S1).
struct Flood;
enum FloodMsg {
    Tick,
}
impl Actor for Flood {
    type Msg = FloodMsg;
    type State = u64;
    type Arguments = ();
    async fn pre_start(
        &self,
        _m: ActorRef<Self::Msg>,
        _a: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(0)
    }
    async fn handle(
        &self,
        _m: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            FloodMsg::Tick => *state += 1,
        }
        Ok(())
    }
}

async fn check_request_reply() -> Result<(), String> {
    let (echo, h) = Actor::spawn(None, Echo, ())
        .await
        .map_err(|e| e.to_string())?;
    let r = ractor::call!(echo, EchoMsg::Ping, "x".to_string()).map_err(|e| e.to_string())?;
    echo.stop(None);
    let _ = h.await;
    (r == "x")
        .then_some(())
        .ok_or(format!("echo returned {r:?}"))
}

async fn check_stream() -> Result<(), String> {
    let (ticker, th) = Actor::spawn(None, Ticker, ())
        .await
        .map_err(|e| e.to_string())?;
    let (coll, ch) = Actor::spawn(None, Collector, ())
        .await
        .map_err(|e| e.to_string())?;
    let port = ractor::call!(ticker, TickerMsg::GetPort).map_err(|e| e.to_string())?;
    port.subscribe(coll.clone(), |s| Some(CollectorMsg::Note(s)));
    ractor::call!(ticker, TickerMsg::Emit, "a".to_string()).map_err(|e| e.to_string())?;
    ractor::call!(ticker, TickerMsg::Emit, "b".to_string()).map_err(|e| e.to_string())?;
    // OutputPort forwards to subscribers on a separate task, so poll until both notes arrive.
    let mut got = Vec::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        got = ractor::call!(coll, CollectorMsg::Drain).map_err(|e| e.to_string())?;
        if got.len() >= 2 {
            break;
        }
    }
    ticker.stop(None);
    coll.stop(None);
    let _ = th.await;
    let _ = ch.await;
    (got == ["a", "b"])
        .then_some(())
        .ok_or(format!("stream got {got:?}"))
}

async fn check_cancel() -> Result<(), String> {
    let (slow, sh) = Actor::spawn(None, Slow, ())
        .await
        .map_err(|e| e.to_string())?;
    let slow2 = slow.clone();
    let jh = tokio::spawn(async move { ractor::call!(slow2, SlowMsg::Start, 5_000u64) });
    tokio::time::sleep(Duration::from_millis(50)).await;
    slow.cast(SlowMsg::Cancel).map_err(|e| e.to_string())?;
    let outcome = jh
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    slow.stop(None);
    let _ = sh.await;
    (outcome == SlowOutcome::Cancelled)
        .then_some(())
        .ok_or(format!("cancel got {outcome:?}"))
}

async fn check_crash_isolation() -> Result<(), String> {
    let (hub, hh) = Actor::spawn(None, Hub, ())
        .await
        .map_err(|e| e.to_string())?;
    let f1 = ractor::call!(hub, HubMsg::FaultyRef)
        .map_err(|e| e.to_string())?
        .ok_or("no faulty child")?;
    let id1 = f1.get_id();
    f1.cast(FaultyMsg::Panic).map_err(|e| e.to_string())?;

    let mut ok = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if let Some(cur) = ractor::call!(hub, HubMsg::FaultyRef).map_err(|e| e.to_string())?
            && cur.get_id() != id1
        {
            let p = ractor::call!(cur, FaultyMsg::Ping).map_err(|e| e.to_string())?;
            ok = p == "ok";
            break;
        }
    }
    hub.stop(None);
    let _ = hh.await;
    ok.then_some(())
        .ok_or("faulty was not restarted".to_string())
}

// Break-finder S1: flooding one actor's mailbox must not starve a sibling actor. If the flood
// monopolized the scheduler, the sibling's Ping would not answer within the deadline.
async fn check_flood_isolation() -> Result<(), String> {
    let (flood, fh) = Actor::spawn(None, Flood, ())
        .await
        .map_err(|e| e.to_string())?;
    let (echo, eh) = Actor::spawn(None, Echo, ())
        .await
        .map_err(|e| e.to_string())?;
    for _ in 0..20_000u32 {
        flood.cast(FloodMsg::Tick).map_err(|e| e.to_string())?;
    }
    let answered = tokio::time::timeout(Duration::from_secs(2), async {
        ractor::call!(echo, EchoMsg::Ping, "live".to_string())
    })
    .await;
    flood.stop(None);
    echo.stop(None);
    let _ = fh.await;
    let _ = eh.await;
    match answered {
        Err(_) => {
            Err("sibling starved: Ping did not answer within 2s under a 20k flood".to_string())
        }
        Ok(Err(e)) => Err(format!("sibling ping errored: {e}")),
        Ok(Ok(v)) if v == "live" => Ok(()),
        Ok(Ok(v)) => Err(format!("sibling returned {v:?}")),
    }
}

// Break-finder S4: a restart storm must be bounded. Crash the child faster than the meltdown cap; the
// hub must stop restarting at MAX_RESTARTS and mark itself degraded, rather than spinning forever.
async fn check_restart_storm_bounded() -> Result<(), String> {
    let (hub, hh) = Actor::spawn(None, Hub, ())
        .await
        .map_err(|e| e.to_string())?;
    let mut last_id = ractor::call!(hub, HubMsg::FaultyRef)
        .map_err(|e| e.to_string())?
        .map(|f| f.get_id());
    for _ in 0..(MAX_RESTARTS + 5) {
        let Some(f) = ractor::call!(hub, HubMsg::FaultyRef).map_err(|e| e.to_string())? else {
            break; // degraded: no child left to crash
        };
        f.cast(FaultyMsg::Panic).map_err(|e| e.to_string())?;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            match ractor::call!(hub, HubMsg::FaultyRef).map_err(|e| e.to_string())? {
                None => break,
                Some(nf) if Some(nf.get_id()) != last_id => {
                    last_id = Some(nf.get_id());
                    break;
                }
                _ => {}
            }
        }
    }
    let degraded = ractor::call!(hub, HubMsg::Degraded).map_err(|e| e.to_string())?;
    let restarts = ractor::call!(hub, HubMsg::Restarts).map_err(|e| e.to_string())?;
    hub.stop(None);
    let _ = hh.await;
    if degraded && restarts == MAX_RESTARTS {
        Ok(())
    } else {
        Err(format!(
            "storm not bounded: degraded={degraded}, restarts={restarts} (cap {MAX_RESTARTS})"
        ))
    }
}

// Break-finder S2: a bounded mailbox must hold under a fast-producer / slow-consumer flood. A bounded
// channel sheds once full (try_send), so memory never grows past `cap` — the bounded-sink policy the
// real edges adopt (ractor's own mailbox is unbounded, so edges wrap it in this).
async fn check_bounded_mailbox() -> Result<(), String> {
    use tokio::sync::mpsc;
    let cap = 1024usize;
    let (tx, mut rx) = mpsc::channel::<u64>(cap);
    let consumer = tokio::spawn(async move {
        let mut n = 0u64;
        while rx.recv().await.is_some() {
            n += 1;
            tokio::time::sleep(Duration::from_micros(20)).await;
            if n >= 1000 {
                break;
            }
        }
        n
    });
    let mut shed = 0u64;
    let mut sent = 0u64;
    for i in 0..20_000u64 {
        match tx.try_send(i) {
            Ok(()) => sent += 1,
            Err(mpsc::error::TrySendError::Full(_)) => shed += 1,
            Err(mpsc::error::TrySendError::Closed(_)) => break,
        }
    }
    drop(tx);
    let _ = consumer.await;
    if shed > 0 {
        Ok(())
    } else {
        Err(format!(
            "bound did not engage under flood: sent {sent}, shed {shed}"
        ))
    }
}

// An actor that instruments its own handler time via a StallMonitor, and can be told to run a
// synchronous (non-cooperative) blocking handler — the case tokio-console cannot see (break-finder S3).
struct Blocker {
    mon: Arc<StallMonitor>,
}
enum BlockerMsg {
    /// Block the handler synchronously for `ms` (std::thread::sleep — blocks the worker).
    Block(u64, RpcReplyPort<()>),
    /// A well-behaved, fast handler.
    Noop(RpcReplyPort<()>),
}
impl Actor for Blocker {
    type Msg = BlockerMsg;
    type State = ();
    type Arguments = ();
    async fn pre_start(
        &self,
        _m: ActorRef<Self::Msg>,
        _a: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(())
    }
    async fn handle(
        &self,
        _m: ActorRef<Self::Msg>,
        msg: Self::Msg,
        _s: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        let start = Instant::now();
        let reply = match msg {
            BlockerMsg::Block(ms, reply) => {
                std::thread::sleep(Duration::from_millis(ms));
                reply
            }
            BlockerMsg::Noop(reply) => reply,
        };
        self.mon.record(start.elapsed());
        let _ = reply.send(());
        Ok(())
    }
}

// Break-finder S3: a synchronous/CPU-bound (blocking) handler must be DETECTED, not silently starving
// the scheduler. The StallMonitor flags a 200ms blocking handler and attributes it; a fast handler is
// not flagged.
async fn check_blocking_handler_detected() -> Result<(), String> {
    let mon = Arc::new(StallMonitor::new(Duration::from_millis(100)));
    let (blocker, bh) = Actor::spawn(None, Blocker { mon: mon.clone() }, ())
        .await
        .map_err(|e| e.to_string())?;
    ractor::call!(blocker, BlockerMsg::Noop).map_err(|e| e.to_string())?;
    if mon.stalls() != 0 {
        return Err("a fast handler was wrongly flagged as a stall".to_string());
    }
    ractor::call!(blocker, BlockerMsg::Block, 200u64).map_err(|e| e.to_string())?;
    blocker.stop(None);
    let _ = bh.await;
    if mon.stalls() >= 1 && mon.max_micros() >= 150_000 {
        Ok(())
    } else {
        Err(format!(
            "blocking handler not detected: stalls={}, max_us={}",
            mon.stalls(),
            mon.max_micros()
        ))
    }
}

fn temp_sock(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "thepiad-conf-{}-{tag}-{n}.sock",
        std::process::id()
    ))
}

// Break-finder S6: a frame claiming more than MAX_FRAME must be rejected BEFORE allocation (no
// memory isolation → one oversized frame would otherwise OOM the whole daemon). The daemon must
// survive the attack and keep serving.
async fn check_oversized_frame_rejected() -> Result<(), String> {
    let path = temp_sock("s6");
    let (echo, _) = Actor::spawn(None, Echo, ())
        .await
        .map_err(|e| e.to_string())?;
    let (ticker, _) = Actor::spawn(None, Ticker, ())
        .await
        .map_err(|e| e.to_string())?;
    let core = Core {
        echo: echo.clone(),
        ticker: ticker.clone(),
    };
    let listener = socket::bind(&path).await.map_err(|e| e.to_string())?;
    let (sd_tx, sd_rx) = watch::channel(false);
    let server = tokio::spawn(socket::serve(
        listener,
        core,
        Arc::new(EdgeRegistry::with_builtins()),
        sd_rx,
    ));

    let result = async {
        let mut attacker = Conn::connect(&path).await.map_err(|e| e.to_string())?;
        attacker
            .send_len_prefix(u32::MAX)
            .await
            .map_err(|e| e.to_string())?;
        let _ = attacker.recv().await; // the server closes the connection
        drop(attacker);
        let mut c = Conn::connect(&path).await.map_err(|e| e.to_string())?;
        c.send(&json!({ "op": "ping", "msg": "alive" }))
            .await
            .map_err(|e| e.to_string())?;
        let resp = c
            .recv()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("server did not survive the oversized frame")?;
        if resp.get("ok").and_then(|v| v.as_str()) == Some("alive") {
            Ok(())
        } else {
            Err(format!("post-attack ping returned {resp:?}"))
        }
    }
    .await;

    let _ = sd_tx.send(true);
    let _ = server.await;
    echo.stop(None);
    ticker.stop(None);
    let _ = std::fs::remove_file(&path);
    result
}

// Break-finder S10: flooding one edge must not starve an unrelated edge (hub edge-isolation). Sustain
// a flood on the `emit` (ticker) edge while measuring `ping` (echo) latency; the unrelated edge must
// stay responsive. (The rigorous open-loop/HdrHistogram p99-knee version is the nightly S10 job.)
async fn check_edge_isolation() -> Result<(), String> {
    let path = temp_sock("s10");
    let (echo, _) = Actor::spawn(None, Echo, ())
        .await
        .map_err(|e| e.to_string())?;
    let (ticker, _) = Actor::spawn(None, Ticker, ())
        .await
        .map_err(|e| e.to_string())?;
    let core = Core {
        echo: echo.clone(),
        ticker: ticker.clone(),
    };
    let listener = socket::bind(&path).await.map_err(|e| e.to_string())?;
    let (sd_tx, sd_rx) = watch::channel(false);
    let server = tokio::spawn(socket::serve(
        listener,
        core,
        Arc::new(EdgeRegistry::with_builtins()),
        sd_rx,
    ));

    let result = async {
        // Sustained flood on the emit (ticker) edge.
        let flood: Vec<_> = (0..16)
            .map(|_| {
                let p = path.clone();
                tokio::spawn(async move {
                    for _ in 0..60 {
                        if let Ok(mut c) = Conn::connect(&p).await {
                            let _ = c.send(&json!({ "op": "emit", "msg": "x" })).await;
                            let _ = c.recv().await;
                        }
                    }
                })
            })
            .collect();

        // Concurrently measure the ping (echo) edge; it must stay responsive.
        let mut max_ms = 0u128;
        for _ in 0..15 {
            let start = Instant::now();
            let mut c = Conn::connect(&path).await.map_err(|e| e.to_string())?;
            c.send(&json!({ "op": "ping", "msg": "p" }))
                .await
                .map_err(|e| e.to_string())?;
            let resp = c
                .recv()
                .await
                .map_err(|e| e.to_string())?
                .ok_or("no ping response under emit flood")?;
            if resp.get("ok").and_then(|v| v.as_str()) != Some("p") {
                return Err(format!("ping under load returned {resp:?}"));
            }
            max_ms = max_ms.max(start.elapsed().as_millis());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for f in flood {
            let _ = f.await;
        }
        if max_ms < 500 {
            Ok(())
        } else {
            Err(format!(
                "ping p-max {max_ms}ms under emit flood — unrelated edge was starved"
            ))
        }
    }
    .await;

    let _ = sd_tx.send(true);
    let _ = server.await;
    echo.stop(None);
    ticker.stop(None);
    let _ = std::fs::remove_file(&path);
    result
}

// Claim C2: adding an edge is "register an adapter" with ZERO change to dispatch/core. Register a
// fake `pong` edge into the registry; it must route, and the builtins must be undisturbed.
async fn check_edge_registration() -> Result<(), String> {
    struct PongEdge;
    impl Edge for PongEdge {
        fn op(&self) -> &'static str {
            "pong"
        }
        fn handle<'a>(&'a self, mut ctx: EdgeCtx<'a>) -> EdgeFuture<'a> {
            Box::pin(async move { ctx.reply(&json!({ "ok": "pong" })).await })
        }
    }

    let path = temp_sock("c2");
    let (echo, _) = Actor::spawn(None, Echo, ())
        .await
        .map_err(|e| e.to_string())?;
    let (ticker, _) = Actor::spawn(None, Ticker, ())
        .await
        .map_err(|e| e.to_string())?;
    let core = Core {
        echo: echo.clone(),
        ticker: ticker.clone(),
    };
    let mut registry = EdgeRegistry::with_builtins();
    registry.register(Box::new(PongEdge));
    let listener = socket::bind(&path).await.map_err(|e| e.to_string())?;
    let (sd_tx, sd_rx) = watch::channel(false);
    let server = tokio::spawn(socket::serve(listener, core, Arc::new(registry), sd_rx));

    let result = async {
        let mut c = Conn::connect(&path).await.map_err(|e| e.to_string())?;
        c.send(&json!({ "op": "pong" }))
            .await
            .map_err(|e| e.to_string())?;
        let resp = c
            .recv()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("no response from the registered edge")?;
        if resp.get("ok").and_then(|v| v.as_str()) != Some("pong") {
            return Err(format!("registered edge returned {resp:?}"));
        }
        // The builtins still route (the drop-in did not disturb dispatch).
        let mut c2 = Conn::connect(&path).await.map_err(|e| e.to_string())?;
        c2.send(&json!({ "op": "ping", "msg": "hi" }))
            .await
            .map_err(|e| e.to_string())?;
        let resp2 = c2
            .recv()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("no ping response")?;
        if resp2.get("ok").and_then(|v| v.as_str()) != Some("hi") {
            return Err(format!("builtin ping disturbed: {resp2:?}"));
        }
        Ok(())
    }
    .await;

    let _ = sd_tx.send(true);
    let _ = server.await;
    echo.stop(None);
    ticker.stop(None);
    let _ = std::fs::remove_file(&path);
    result
}

fn vector(result: Result<(), String>, evidence: &str) -> Value {
    match result {
        Ok(()) => json!({ "status": "pass", "evidence": evidence }),
        Err(e) => json!({ "status": "fail", "evidence": format!("{evidence}: {e}") }),
    }
}

// A break-finder holds (`pass`) or finds a break (`break`) — never `fail` (break-finding is exploratory).
fn break_vector(result: Result<(), String>, evidence: &str) -> Value {
    match result {
        Ok(()) => json!({ "status": "pass", "evidence": evidence }),
        Err(e) => json!({ "status": "break", "evidence": format!("{evidence}: {e}") }),
    }
}

/// Run the increment's conformance checks and build the report value.
pub async fn run() -> Value {
    let mut vectors = Map::new();

    // C3: the three verbs on the shared core actors.
    let c3 = check_request_reply()
        .await
        .and(check_stream().await)
        .and(check_cancel().await);
    vectors.insert(
        "C3".to_string(),
        vector(
            c3,
            "echo request/reply + ticker stream + slow cancel on the shared core actors",
        ),
    );

    // C4: crash isolation + supervision.
    let c4 = check_crash_isolation().await;
    vectors.insert(
        "C4".to_string(),
        vector(
            c4,
            "hub restarts a panicked faulty child; the daemon stays up",
        ),
    );

    // C2: adding an edge is "register an adapter" with zero dispatch/core change.
    let c2 = check_edge_registration().await;
    vectors.insert(
        "C2".to_string(),
        vector(
            c2,
            "a fake `pong` edge registered into the registry routes with zero dispatch change; builtins undisturbed",
        ),
    );

    // S1 (break-finder): flooding one actor must not starve a sibling.
    let s1 = check_flood_isolation().await;
    vectors.insert(
        "S1".to_string(),
        break_vector(
            s1,
            "20k-message flood of one actor; sibling echo still answers Ping within 2s",
        ),
    );

    // S2 (break-finder): a bounded mailbox holds under a fast-producer / slow-consumer flood.
    let s2 = check_bounded_mailbox().await;
    vectors.insert(
        "S2".to_string(),
        break_vector(
            s2,
            "20k flood into a cap-1024 bounded sink with a slow consumer; excess is shed, memory bounded",
        ),
    );

    // S3 (break-finder): a blocking/CPU-bound handler is detected, not silently starving.
    let s3 = check_blocking_handler_detected().await;
    vectors.insert(
        "S3".to_string(),
        break_vector(
            s3,
            "StallMonitor flags a 200ms synchronous handler (attributable); a fast handler is not flagged",
        ),
    );

    // S4 (break-finder): a restart storm is bounded by the hub meltdown cap.
    let s4 = check_restart_storm_bounded().await;
    vectors.insert(
        "S4".to_string(),
        break_vector(
            s4,
            "crash storm past the cap; hub stops at MAX_RESTARTS and marks degraded (no infinite restart loop)",
        ),
    );

    // S6 (break-finder): an oversized control-socket frame is rejected before allocation.
    let s6 = check_oversized_frame_rejected().await;
    vectors.insert(
        "S6".to_string(),
        break_vector(
            s6,
            "a frame claiming u32::MAX bytes is rejected before allocation; the daemon survives and still serves ping",
        ),
    );

    // S10 (break-finder): flooding one edge does not starve an unrelated edge.
    let s10 = check_edge_isolation().await;
    vectors.insert(
        "S10".to_string(),
        break_vector(
            s10,
            "sustained emit-edge flood; ping-edge p-max stays under 500ms (envelope-load isolation; HdrHistogram knee is the nightly job)",
        ),
    );

    json!({
        "generated_by": concat!("the-daemon ", env!("CARGO_PKG_VERSION"), " (feature=conformance)"),
        "increment": "2: control socket",
        "vectors": Value::Object(vectors),
    })
}
