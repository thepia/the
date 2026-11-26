//! `slow` — a long-running, cancellable op. Proves the **cancel** verb (CLI Ctrl-C, LSP
//! `$/cancelRequest`, AHP cancel-turn all route to this). The work runs in a spawned task so the actor
//! stays responsive; a `Cancel` signals it, and the reply resolves either `Completed` or `Cancelled`.

use std::time::Duration;

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use tokio::sync::oneshot;

pub struct Slow;

#[derive(Debug, PartialEq, Eq)]
pub enum SlowOutcome {
    Completed,
    Cancelled,
}

pub enum SlowMsg {
    /// Start an op that completes after `ms`, replying with the outcome; cancellable meanwhile.
    Start(u64, RpcReplyPort<SlowOutcome>),
    /// Cancel the in-flight op (if any).
    Cancel,
}

#[derive(Default)]
pub struct SlowState {
    cancel: Option<oneshot::Sender<()>>,
}

impl Actor for Slow {
    type Msg = SlowMsg;
    type State = SlowState;
    type Arguments = ();

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        _args: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(SlowState::default())
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            SlowMsg::Start(ms, reply) => {
                let (ctx, crx) = oneshot::channel::<()>();
                state.cancel = Some(ctx);
                tokio::spawn(async move {
                    let outcome = tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(ms)) => SlowOutcome::Completed,
                        _ = crx => SlowOutcome::Cancelled,
                    };
                    if !reply.is_closed() {
                        let _ = reply.send(outcome);
                    }
                });
            }
            SlowMsg::Cancel => {
                if let Some(tx) = state.cancel.take() {
                    let _ = tx.send(());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slow_can_be_cancelled() {
        let (slow, sh) = Actor::spawn(None, Slow, ()).await.expect("spawn slow");
        let slow2 = slow.clone();
        // Issue a long op on a separate task so we can cancel it mid-flight.
        let jh =
            tokio::spawn(
                async move { ractor::call!(slow2, SlowMsg::Start, 5_000u64).expect("start") },
            );
        tokio::time::sleep(Duration::from_millis(50)).await;
        slow.cast(SlowMsg::Cancel).expect("cancel");
        let outcome = jh.await.expect("join op");
        assert_eq!(outcome, SlowOutcome::Cancelled);

        slow.stop(None);
        sh.await.expect("join slow");
    }

    #[tokio::test]
    async fn slow_completes_when_not_cancelled() {
        let (slow, sh) = Actor::spawn(None, Slow, ()).await.expect("spawn slow");
        let outcome = ractor::call!(slow, SlowMsg::Start, 10u64).expect("start");
        assert_eq!(outcome, SlowOutcome::Completed);
        slow.stop(None);
        sh.await.expect("join slow");
    }
}
