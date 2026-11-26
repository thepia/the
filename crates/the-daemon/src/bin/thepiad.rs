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

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "thepiad — the Thepia communications-hub daemon

USAGE:
    thepiad [--socket <path>]   run the daemon, serving the control socket
    thepiad --lsp               run the agent-config language server over stdio (VS Code)
    thepiad --version, -V       print the version
    thepiad --help, -h          print this help
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Version/help are plain, non-daemon modes: the version string is the stdout result; help goes
    // to stdout too (it is the requested output), each exiting 0 without touching the daemon.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("thepiad {VERSION}");
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return;
    }

    #[cfg(feature = "conformance")]
    {
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if arg == "--emit-conformance" {
                let path = it
                    .next()
                    .cloned()
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
    if args.iter().any(|a| a == "--lsp") {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        if let Err(e) = rt.block_on(the_daemon::lsp::run_stdio()) {
            eprintln!("thepiad lsp: {e}");
        }
        return;
    }

    // The only flag the daemon mode itself accepts is `--socket <path>`. Reject anything else with a
    // usage error instead of silently starting the daemon on a typo (e.g. `thepiad --verison`).
    if let Some(bad) = unknown_daemon_arg(&args) {
        eprintln!("thepiad: unrecognized argument `{bad}`\n\n{USAGE}");
        std::process::exit(2);
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(run_daemon());
}

/// The first argument that daemon mode does not understand (everything except `--socket <path>`),
/// or `None` if the arguments are all valid for `run_daemon`.
fn unknown_daemon_arg(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--socket" {
            it.next(); // consume the path value
        } else {
            return Some(a.clone());
        }
    }
    None
}
