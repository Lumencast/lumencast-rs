//! `lumencast` — command-line driver for the cross-language interop
//! matrix.
//!
//! Two subcommands :
//!
//! - `lumencast serve-scenario --ws-port N --test-control-port M`
//!   spawns an LSDP/1 server and the test control plane on separate
//!   ports, prints the discovery JSON line to stdout, then runs until
//!   SIGINT/SIGTERM.
//!
//! - `lumencast conformance --server <ws-url> [--control-url <http-url>]
//!   [--scenarios <dir>] [--scenario <name>]` runs the scenario player
//!   against an external server and exits 0 on full pass.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use lumencast_conformance::Tag;
use lumencast_conformance::harness::{Config, ScenariosSource, Target};
use tracing_subscriber::EnvFilter;

mod serve;

/// `lumencast` CLI entry point.
#[derive(Parser, Debug)]
#[command(
    name = "lumencast",
    about = "Lumencast SDK for Rust — interop CLI",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Spawn a Lumencast server with the test control plane attached.
    ///
    /// Prints exactly one JSON line on stdout
    /// (`{"control_url":"...","ws_url":"..."}`) before accepting any
    /// connection, then logs to stderr via `tracing`.
    ServeScenario(ServeArgs),

    /// Run the conformance suite against an external server.
    ///
    /// `--control-url` engages the HTTP control plane (per
    /// `interop/CONTROL.md`). Without it, only client→server scenarios
    /// that don't need to prime authoritative state can run.
    Conformance(ConformanceArgs),
}

#[derive(Parser, Debug)]
struct ServeArgs {
    /// Port for the LSDP/1 WebSocket endpoint. `0` picks a free port.
    #[arg(long, default_value_t = 0)]
    ws_port: u16,

    /// Port for the HTTP test control plane. `0` picks a free port.
    #[arg(long, default_value_t = 0)]
    test_control_port: u16,

    /// Bind address. Defaults to `127.0.0.1`.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
}

#[derive(Parser, Debug)]
struct ConformanceArgs {
    /// LSDP/1 WebSocket URL of the server under test.
    #[arg(long)]
    server: String,

    /// HTTP control-plane base URL. When set, the harness drives
    /// `setup`/`reset`/`state`/`emit` against it.
    #[arg(long)]
    control_url: Option<String>,

    /// Optional scenario directory. When omitted, the harness has no
    /// scenarios and reports `0 / 0 passed` (useful as a smoke test).
    #[arg(long)]
    scenarios: Option<PathBuf>,

    /// Optional single-scenario filter (matches by name).
    #[arg(long)]
    scenario: Option<String>,

    /// Tag filter — `required`, `recommended`, or `extended`. May be
    /// repeated. Defaults to `required` when omitted.
    #[arg(long, value_enum)]
    tag: Vec<TagArg>,

    /// Path to the canonical token map JSON
    /// (`interop/fixtures/canonical-tokens.json`). When omitted, the
    /// harness uses a built-in fallback.
    #[arg(long)]
    tokens: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[allow(clippy::enum_variant_names)]
enum TagArg {
    Required,
    Recommended,
    Extended,
}

impl From<TagArg> for Tag {
    fn from(t: TagArg) -> Self {
        match t {
            TagArg::Required => Tag::Required,
            TagArg::Recommended => Tag::Recommended,
            TagArg::Extended => Tag::Extended,
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to start tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };

    match cli.command {
        Command::ServeScenario(args) => match runtime.block_on(serve::run(args)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                tracing::error!(?e, "serve-scenario failed");
                ExitCode::from(1)
            }
        },
        Command::Conformance(args) => match runtime.block_on(run_conformance(args)) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(1),
            Err(e) => {
                tracing::error!(?e, "conformance failed");
                ExitCode::from(2)
            }
        },
    }
}

async fn run_conformance(args: ConformanceArgs) -> Result<bool, Box<dyn std::error::Error>> {
    let control_url = args
        .control_url
        .clone()
        .ok_or("--control-url is required for v0.1 (the harness needs the test control plane)")?;

    let tokens = load_tokens(args.tokens.as_deref())?;
    let scenarios = match args.scenarios.as_deref() {
        Some(dir) => ScenariosSource::Directory(dir.to_path_buf()),
        None => ScenariosSource::Empty,
    };

    let mut tags: Vec<Tag> = args.tag.iter().copied().map(Tag::from).collect();
    if tags.is_empty() {
        tags.push(Tag::Required);
    }

    let config = Config {
        target: Target::Server {
            ws_url: args.server,
            control_url,
        },
        tags,
        scenario_filter: args.scenario,
        tokens,
        scenarios,
    };

    let report = lumencast_conformance::harness::run(config).await?;
    print_report(&report);
    Ok(report.all_passed())
}

fn print_report(report: &lumencast_conformance::Report) {
    if report.outcomes.is_empty() {
        eprintln!("[conformance] no scenarios discovered (passed: 0 / 0)");
        return;
    }
    for outcome in &report.outcomes {
        let tag = if outcome.skipped {
            "SKIP"
        } else if outcome.passed {
            "PASS"
        } else {
            "FAIL"
        };
        match &outcome.message {
            Some(m) => eprintln!("[conformance] {tag} {} — {m}", outcome.name),
            None => eprintln!("[conformance] {tag} {}", outcome.name),
        }
    }
    let failed = report.total - report.passed - report.skipped;
    eprintln!(
        "[conformance] {} / {} passed ({} skipped, {} failed)",
        report.passed, report.total, report.skipped, failed
    );
}

/// Load the placeholder→token map. Default = canonical interop tokens.
fn load_tokens(path: Option<&std::path::Path>) -> std::io::Result<BTreeMap<String, String>> {
    if let Some(p) = path {
        let bytes = std::fs::read(p)?;
        let parsed: BTreeMap<String, String> = serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        return Ok(parsed);
    }
    Ok(default_tokens())
}

fn default_tokens() -> BTreeMap<String, String> {
    [
        ("$TOKEN_OPERATOR", "interop-tok-operator-7f3a"),
        ("$TOKEN_VIEWER", "interop-tok-viewer-7f3a"),
        ("$TOKEN_SERVICE", "interop-tok-service-7f3a"),
        ("$TOKEN_TEST", "interop-tok-test-7f3a"),
        ("$TOKEN_INVALID", "interop-tok-invalid-7f3a"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}
