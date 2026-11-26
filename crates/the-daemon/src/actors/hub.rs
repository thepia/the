//! `hub` — the supervisor. It spawns a linked [`super::faulty::Faulty`] child and, when that child
//! **fails** (panics), restarts it — but **bounded by a meltdown cap** ([`MAX_RESTARTS`] restarts
//! within [`WINDOW`]): a crash storm cannot spin forever; past the cap the hub stops restarting and
//! marks itself `degraded` instead. Proves in-process crash isolation (one actor's crash is contained
//! and auto-healed, the daemon stays up) AND that a restart storm is bounded (break-finder S4). This is
//! the in-process half of the two-level supervision model (the out-of-process half is thepiad
//! supervising sibling daemons / forked instances, WORK-0046/0047). The hand-rolled window cap here is
//! the spike stand-in for the production `ractor-supervisor` meltdown policy named in ADR-0007.

use std::time::{Duration, Instant};

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort, SupervisionEvent};

use super::faulty::{Faulty, FaultyMsg};

/// Max child restarts allowed within [`WINDOW`] before the hub melts down (stops restarting).
pub const MAX_RESTARTS: u32 = 5;
/// The sliding window over which [`MAX_RESTARTS`] is counted.
const WINDOW: Duration = Duration::from_secs(10);

pub struct Hub;

pub struct HubState {
    faulty: Option<ActorRef<FaultyMsg>>,
    restarts: u32,
    /// Restart timestamps within the current window (pruned on each failure).
    recent: Vec<Instant>,
    /// Set once the meltdown cap trips; the child is no longer restarted.
    degraded: bool,
}

pub enum HubMsg {
    /// Current `faulty` child ref (its id changes across a restart; `None` once degraded).
    FaultyRef(RpcReplyPort<Option<ActorRef<FaultyMsg>>>),
    /// How many times the child has been restarted.
    Restarts(RpcReplyPort<u32>),
    /// Whether the meltdown cap has tripped (restart storm bounded).
    Degraded(RpcReplyPort<bool>),
}

impl Actor for Hub {
    type Msg = HubMsg;
    type State = HubState;
    type Arguments = ();

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        _args: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        let (faulty, _) = Actor::spawn_linked(None, Faulty, (), myself.get_cell()).await?;
        Ok(HubState {
            faulty: Some(faulty),
            restarts: 0,
            recent: Vec::new(),
            degraded: false,
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            HubMsg::FaultyRef(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(state.faulty.clone());
                }
            }
            HubMsg::Restarts(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(state.restarts);
                }
            }
            HubMsg::Degraded(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(state.degraded);
                }
            }
        }
        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        myself: ActorRef<Self::Msg>,
        evt: SupervisionEvent,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        // Restart only on failure (a panic), never on a graceful terminate — so a clean shutdown of
        // the linked child during hub stop does not resurrect it.
        if let SupervisionEvent::ActorFailed(cell, _) = &evt {
            let is_ours = state
                .faulty
                .as_ref()
                .is_some_and(|f| f.get_id() == cell.get_id());
            if is_ours {
                // Meltdown bound: prune restarts outside the window, and if the cap is already
                // reached, stop restarting (mark degraded) rather than spinning on a crash storm.
                let now = Instant::now();
                state.recent.retain(|t| now.duration_since(*t) < WINDOW);
                if state.recent.len() as u32 >= MAX_RESTARTS {
                    state.faulty = None;
                    state.degraded = true;
                    return Ok(());
                }
                let (faulty, _) = Actor::spawn_linked(None, Faulty, (), myself.get_cell()).await?;
                state.faulty = Some(faulty);
                state.recent.push(now);
                state.restarts += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn hub_restarts_a_crashed_faulty() {
        let (hub, hh) = Actor::spawn(None, Hub, ()).await.expect("spawn hub");

        let f1 = ractor::call!(hub, HubMsg::FaultyRef)
            .expect("faulty-ref")
            .expect("faulty present");
        assert_eq!(ractor::call!(f1, FaultyMsg::Ping).expect("ping"), "ok");
        let id1 = f1.get_id();

        // Crash it.
        f1.cast(FaultyMsg::Panic).expect("panic cast");

        // Wait for the supervisor to observe the failure and respawn a new child.
        let mut f2 = None;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if let Some(cur) = ractor::call!(hub, HubMsg::FaultyRef).expect("faulty-ref")
                && cur.get_id() != id1
            {
                f2 = Some(cur);
                break;
            }
        }
        let f2 = f2.expect("faulty restarted with a new id");

        // The hub survived and the new child serves requests.
        assert_eq!(ractor::call!(f2, FaultyMsg::Ping).expect("ping2"), "ok");
        assert!(ractor::call!(hub, HubMsg::Restarts).expect("restarts") >= 1);

        hub.stop(None);
        hh.await.expect("join hub");
    }
}
