//! `thepiad` — the Thepia communications-hub daemon entry point (WORK-0047 spike).
//!
//! This increment has no resident loop yet; the actor core, lifecycle (singleton lock, control
//! socket, graceful shutdown), and edges are added in subsequent increments. Behind the `conformance`
//! cargo feature (a debug/dev flag), `--emit-conformance <path>` writes the WORK-0047 claim report.

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
        }
    }
    eprintln!("thepiad: spike scaffold (no resident loop yet)");
}
