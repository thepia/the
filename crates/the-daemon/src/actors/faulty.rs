//! `faulty` — a worker that panics on demand, to prove **crash isolation + supervision**: when it
//! panics, its supervisor ([`super::hub::Hub`]) restarts it and the daemon stays up.

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};

pub struct Faulty;

pub enum FaultyMsg {
    /// Liveness: replies `"ok"`.
    Ping(RpcReplyPort<&'static str>),
    /// Intentionally panic the actor (the supervisor should restart it).
    Panic,
}

impl Actor for Faulty {
    type Msg = FaultyMsg;
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
            FaultyMsg::Ping(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send("ok");
                }
            }
            FaultyMsg::Panic => panic!("faulty: intentional crash (supervision proof)"),
        }
        Ok(())
    }
}
