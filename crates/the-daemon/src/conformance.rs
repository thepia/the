//! Conformance self-report (WORK-0047), behind the `conformance` cargo feature (a debug/dev compile
//! flag, never in a release build). It exercises the daemon's own core actors and emits a per-claim
//! report (`agent/vectors/report.schema.json` in thepia-app-spec) that the spec repo's
//! `cadence/check-daemon-conformance.py` reconciles against its claim manifest.
//!
//! Increment 1 (actor core) proves **C3** (request/reply + stream + cancel on the shared core actors)
//! and **C4** (crash isolation + supervision). Claims C1/C2/C5/C6/C7/C8 are omitted here, so the
//! reconciler reports them `pending` until later increments cover them.

use std::time::Duration;

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use serde_json::{Map, Value, json};

use crate::actors::echo::{Echo, EchoMsg};
use crate::actors::faulty::FaultyMsg;
use crate::actors::hub::{Hub, HubMsg};
use crate::actors::slow::{Slow, SlowMsg, SlowOutcome};
use crate::actors::ticker::{Ticker, TickerMsg};

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
    let got = ractor::call!(coll, CollectorMsg::Drain).map_err(|e| e.to_string())?;
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

    // S1 (break-finder): flooding one actor must not starve a sibling.
    let s1 = check_flood_isolation().await;
    vectors.insert(
        "S1".to_string(),
        break_vector(
            s1,
            "20k-message flood of one actor; sibling echo still answers Ping within 2s",
        ),
    );

    json!({
        "generated_by": concat!("the-daemon ", env!("CARGO_PKG_VERSION"), " (feature=conformance)"),
        "increment": "1: actor core",
        "vectors": Value::Object(vectors),
    })
}
