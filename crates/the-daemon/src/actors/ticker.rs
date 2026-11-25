//! `ticker` — the pub/sub stream source. Holds a [`ractor::OutputPort`]; subscribers register an
//! actor + a converter, and `Emit` fans a notification to every subscriber. This is the core proof of
//! the **stream** verb (the `tail` edge and AHP/MCP/LSP notification streams all attach here).

use std::sync::Arc;

use ractor::{Actor, ActorProcessingErr, ActorRef, OutputPort, RpcReplyPort};

pub struct Ticker;

pub struct TickerState {
    out: Arc<OutputPort<String>>,
}

/// Messages the `ticker` actor accepts.
pub enum TickerMsg {
    /// Hand back the output port so a caller can subscribe its own actor to the stream.
    GetPort(RpcReplyPort<Arc<OutputPort<String>>>),
    /// Fan a notification to every subscriber; the reply fires once the fan-out has been dispatched,
    /// which makes ordering deterministic for callers that await it.
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
            TickerMsg::Emit(s, reply) => {
                state.out.send(s);
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

        // Awaiting each Emit guarantees the Notes are enqueued on the collector before Drain.
        ractor::call!(ticker, TickerMsg::Emit, "a".to_string()).expect("emit a");
        ractor::call!(ticker, TickerMsg::Emit, "b".to_string()).expect("emit b");

        let got = ractor::call!(coll, CollectorMsg::Drain).expect("drain");
        assert_eq!(got, vec!["a".to_string(), "b".to_string()]);

        ticker.stop(None);
        coll.stop(None);
        th.await.expect("join ticker");
        ch.await.expect("join collector");
    }
}
