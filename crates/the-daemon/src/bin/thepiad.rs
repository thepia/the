//! `thepiad` — the Thepia communications-hub daemon entry point (WORK-0047).
//!
//! Increment 2: a resident daemon serving the control socket. It spawns the actor core, binds the
//! single-instance control socket, serves the `ping` / `tail` edges, and shuts down gracefully on
//! SIGINT/SIGTERM (draining the server, releasing the socket). Behind the `conformance` cargo feature
//! (a debug/dev flag), `--emit-conformance <path>` writes the WORK-0047 claim report instead.

use std::path::PathBuf;
use std::sync::Arc;

use ractor::Actor;
use the_daemon::socket::{self, Core, EdgeRegistry};
use the_daemon::{Echo, Ticker};
use tokio::sync::watch;

fn socket_path() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--socket")
        && let Some(p) = args.get(i + 1)
    {
        return PathBuf::from(p);
    }
    std::env::temp_dir().join("thepiad.sock")
}

async fn wait_for_shutdown() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

async fn run_daemon() {
    let path = socket_path();

    let (echo, _eh) = Actor::spawn(None, Echo, ()).await.expect("spawn echo");
    let (ticker, _th) = Actor::spawn(None, Ticker, ()).await.expect("spawn ticker");
    let core = Core {
        echo: echo.clone(),
        ticker: ticker.clone(),
    };

    let listener = match socket::bind(&path).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("thepiad: cannot bind {}: {e}", path.display());
            return;
        }
    };
    eprintln!("thepiad: listening on {}", path.display());

    let (sd_tx, sd_rx) = watch::channel(false);
    let registry = Arc::new(EdgeRegistry::with_builtins());
    let server = tokio::spawn(socket::serve(listener, core, registry, sd_rx));

    wait_for_shutdown().await;

    eprintln!("thepiad: shutting down");
    let _ = sd_tx.send(true);
    let _ = server.await;
    echo.stop(None);
    ticker.stop(None);
    let _ = std::fs::remove_file(&path);
}

fn main() {
    #[cfg(feature = "conformance")]
    {
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--emit-conformance" {
                let path = args
                    .next()
                    .unwrap_or_else(|| "daemon-conformance-report.json".to_string());
                let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
                let report = rt.block_on(the_daemon::conformance::run());
                let json = serde_json::to_string_pretty(&report).expect("serialize report");
                std::fs::write(&path, json).expect("write report");
                eprintln!("thepiad: wrote conformance report to {path}");
                return;
            }
            if arg == "--perf" {
                let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
                rt.block_on(the_daemon::perf::run());
                return;
            }
        }
    }

    // The agent-config language server (stdio). A real feature, not behind the dev feature.
    if std::env::args().skip(1).any(|a| a == "--lsp") {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        if let Err(e) = rt.block_on(the_daemon::lsp::run_stdio()) {
            eprintln!("thepiad lsp: {e}");
        }
        return;
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(run_daemon());
}
