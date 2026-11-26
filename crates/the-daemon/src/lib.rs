//! thepiad — the Thepia communications-hub daemon (WORK-0047 spike).
//!
//! The daemon is an **actor system** (ADR-0007: Tokio + ractor). Every protocol edge, session, and
//! worker is an actor; this crate holds the thin core (the stand-in demo actors + the supervisor) and
//! the registration points edges attach to (WORK-0046). The spike proves that heterogeneous edges
//! (AHP, MCP, ACP, LSP, the CLI RPC socket, tail) all route onto one core via the three verbs
//! request/reply, stream, and cancel, plus supervision and location transparency.
//!
//! **Increment 1 (this crate as it stands):** the ractor core and its verb-proving tests —
//! [`actors::echo`] (request/reply), [`actors::ticker`] (stream), [`actors::slow`] (cancel), and
//! [`actors::hub`] supervising [`actors::faulty`] (crash isolation). Subsequent increments add the
//! control socket (CLI-RPC + tail), the iroh node bridge, forked-instance supervision, and the AHP host.

pub mod actors;
pub mod ahp;
pub mod socket;
pub mod watchdog;

pub use socket::Core;
pub use watchdog::StallMonitor;

/// Conformance self-report (WORK-0047), compiled only behind the `conformance` cargo feature.
#[cfg(feature = "conformance")]
pub mod conformance;

/// Performance harness (WORK-0047), compiled only behind the `conformance` cargo feature.
#[cfg(feature = "conformance")]
pub mod perf;

pub use actors::echo::{Echo, EchoMsg};
pub use actors::faulty::{Faulty, FaultyMsg};
pub use actors::hub::{Hub, HubMsg};
pub use actors::slow::{Slow, SlowMsg, SlowOutcome};
pub use actors::ticker::{Ticker, TickerMsg};
