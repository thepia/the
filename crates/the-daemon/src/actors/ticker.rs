//! `ticker` — the pub/sub stream source. Holds a [`ractor::OutputPort`]; subscribers register an
//! actor + a converter, and `Emit` fans a notification to every subscriber. This is the core proof of
//! the **stream** verb (the `tail` edge and AHP/MCP/LSP notification streams all attach here).

use std::sync::Arc;

use ractor::{Actor, ActorProcessingErr, ActorRef, OutputPort, RpcReplyPort};
use tokio::sync::mpsc;

pub struct Ticker;

pub struct TickerState {
    out: Arc<OutputPort<String>>,
    channels: Vec<mpsc::Sender<String>>,
}

/// Messages the `ticker` actor accepts.
pub enum TickerMsg {
    /// Hand back the output port so a caller can subscribe its own actor to the stream.
    GetPort(RpcReplyPort<Arc<OutputPort<String>>>),
    /// Subscribe an mpsc channel to the stream (used by the socket `tail` edge). The channel is
    /// bounded, so a slow consumer sheds notes rather than growing memory unbounded.
    SubscribeChannel(mpsc::Sender<String>),
    /// Fan a notification to every subscriber; the reply fires once the value has been published to
    /// the output port. Delivery to each subscriber is then forwarded on a separate task, so a caller
    /// that needs to observe receipt must poll the subscriber, not assume synchronous delivery.
    Emit(String, RpcReplyPort<()>),
}

impl Actor for Ticker {
    type Msg = TickerMsg;
    type State = TickerState;
    type Arguments = ();

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        _args: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(TickerState {
            out: Arc::new(OutputPort::default()),
            channels: Vec::new(),
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            TickerMsg::GetPort(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(state.out.clone());
                }
            }
            TickerMsg::SubscribeChannel(tx) => {
                state.channels.push(tx);
            }
            TickerMsg::Emit(s, reply) => {
                state.out.send(s.clone());
                // Fan to channel subscribers; drop closed ones, shed on a full (bounded) channel.
                state.channels.retain(|c| {
                    !matches!(
                        c.try_send(s.clone()),
                        Err(mpsc::error::TrySendError::Closed(_))
                    )
                });
                if !reply.is_closed() {
                    let _ = reply.send(());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny subscriber that collects notifications; proves fan-out delivery.
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

    #[tokio::test]
    async fn ticker_fans_out_to_subscribers() {
        let (ticker, th) = Actor::spawn(None, Ticker, ()).await.expect("spawn ticker");
        let (coll, ch) = Actor::spawn(None, Collector, ())
            .await
            .expect("spawn collector");

        let port = ractor::call!(ticker, TickerMsg::GetPort).expect("get port");
        port.subscribe(coll.clone(), |s| Some(CollectorMsg::Note(s)));

        ractor::call!(ticker, TickerMsg::Emit, "a".to_string()).expect("emit a");
        ractor::call!(ticker, TickerMsg::Emit, "b".to_string()).expect("emit b");

        // OutputPort forwards to subscribers on a separate task, so poll until both notes arrive.
        let mut got = Vec::new();
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            got = ractor::call!(coll, CollectorMsg::Drain).expect("drain");
            if got.len() >= 2 {
                break;
            }
        }
        assert_eq!(got, vec!["a".to_string(), "b".to_string()]);

        ticker.stop(None);
        coll.stop(None);
        th.await.expect("join ticker");
        ch.await.expect("join collector");
    }
}
