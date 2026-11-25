//! The thepiad core actors — stand-ins that each prove one verb of the edge/core contract:
//! - [`echo`] — request/reply
//! - [`ticker`] — stream (pub/sub)
//! - [`slow`] — cancel
//! - [`faulty`] + [`hub`] — crash isolation + supervision
//!
//! Protocol edges (AHP, MCP, ACP, LSP, the CLI RPC socket, tail) attach as adapter actors that route
//! their inbound frames onto these workers and stream replies back.

pub mod echo;
pub mod faulty;
pub mod hub;
pub mod slow;
pub mod ticker;
