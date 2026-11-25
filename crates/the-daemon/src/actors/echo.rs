//! `echo` — the request/reply core worker. Returns its input unchanged; the simplest proof that an
//! edge's request routes to a core actor and a reply routes back.

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};

pub struct Echo;

/// Messages the `echo` actor accepts.
pub enum EchoMsg {
    /// Request/reply: echo the string back through the reply port.
    Ping(String, RpcReplyPort<String>),
}

impl Actor for Echo {
    type Msg = EchoMsg;
    type State = ();
    type Arguments = ();

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        _args: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(())
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        _state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            EchoMsg::Ping(s, reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(s);
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
    async fn echo_pings_back() {
        let (echo, handle) = Actor::spawn(None, Echo, ()).await.expect("spawn echo");
        let reply = ractor::call!(echo, EchoMsg::Ping, "hi".to_string()).expect("ping");
        assert_eq!(reply, "hi");
        echo.stop(None);
        handle.await.expect("join");
    }
}
