//! `thepiad` — the Thepia communications-hub daemon entry point (WORK-0047 spike).
//!
//! This first increment boots the Tokio runtime and exits immediately; the actor core, lifecycle
//! (singleton lock, control socket, graceful shutdown), and edges are added in subsequent increments.

fn main() {
    eprintln!("thepiad: spike scaffold (no resident loop yet)");
}
