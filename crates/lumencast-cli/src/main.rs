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
//!   against an external server and exits 0 on full pass. Without
//!   `--scenarios`, the suite is discovered from
//!   `$LUMENCAST_PROTOCOL_REPO/conformance/v1/scenarios` or a
//!   conventional relative layout; a run that executes zero scenarios
//!   exits non-zero rather than reporting a vacuous success.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

    /// Scenario directory. When omitted, it is discovered from
    /// `$LUMENCAST_PROTOCOL_REPO/conformance/v1/scenarios`, then from
    /// conventional relative layouts (see `scenario_dir_candidates`).
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
    let dir = resolve_scenarios_dir(args.scenarios.as_deref())?;
    tracing::info!(dir = %dir.display(), "using scenario directory");
    let scenarios = ScenariosSource::Directory(dir);

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
    if report.total == 0 {
        // A run with zero scenarios used to exit 0 (`all_passed()` on an
        // empty report is vacuously true), which made every interop
        // matrix cell report PASS without executing anything. An empty
        // run is a configuration failure, never a success.
        return Err("no scenario executed — refusing to report success".into());
    }
    Ok(report.all_passed())
}

/// Relative path, below a protocol-repo root, of the scenario suite.
const SCENARIOS_REL: [&str; 3] = ["conformance", "v1", "scenarios"];

/// Locate the conformance scenario directory, mirroring the js CLI
/// (`packages/protocol/src/cli.ts::resolveScenariosDir`): explicit flag,
/// then `LUMENCAST_PROTOCOL_REPO`, then conventional relative layouts.
///
/// An explicit flag or env var is honoured as-is — a wrong value must
/// surface as an error, not silently fall back to a heuristic.
fn resolve_scenarios_dir(flag: Option<&Path>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(dir) = flag {
        if !dir.is_dir() {
            return Err(format!("--scenarios {} is not a directory", dir.display()).into());
        }
        return Ok(dir.to_path_buf());
    }

    if let Some(repo) = std::env::var_os("LUMENCAST_PROTOCOL_REPO") {
        let dir = SCENARIOS_REL
            .iter()
            .fold(PathBuf::from(repo), |acc, part| acc.join(part));
        if !dir.is_dir() {
            return Err(format!(
                "LUMENCAST_PROTOCOL_REPO is set but {} is not a directory",
                dir.display()
            )
            .into());
        }
        return Ok(dir);
    }

    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    let candidates = scenario_dir_candidates(cwd.as_deref(), exe.as_deref().and_then(Path::parent));
    for candidate in &candidates {
        if candidate.is_dir() {
            return Ok(candidate.clone());
        }
    }

    Err(format!(
        "no scenario directory found — pass --scenarios, set LUMENCAST_PROTOCOL_REPO, \
         or run from a conventional layout (tried: {})",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .into())
}

/// Conventional locations of `conformance/v1/scenarios`, relative to the
/// working directory and to the binary (the interop driver launches
/// `target/release/lumencast` from an arbitrary cwd, so the binary's own
/// ancestors are the reliable anchor for a sibling checkout).
fn scenario_dir_candidates(cwd: Option<&Path>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(cwd) = cwd {
        bases.extend(cwd.ancestors().take(4).map(Path::to_path_buf));
    }
    if let Some(exe_dir) = exe_dir {
        bases.extend(exe_dir.ancestors().take(6).map(Path::to_path_buf));
    }

    let mut out: Vec<PathBuf> = Vec::new();
    for base in bases {
        // Inside a lumencast-protocol checkout…
        let direct = SCENARIOS_REL
            .iter()
            .fold(base.clone(), |acc, p| acc.join(p));
        // …or next to one (sibling SDK checkouts).
        let sibling = SCENARIOS_REL
            .iter()
            .fold(base.join("lumencast-protocol"), |acc, p| acc.join(p));
        for candidate in [direct, sibling] {
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scenarios_under(root: &Path) -> PathBuf {
        SCENARIOS_REL
            .iter()
            .fold(root.to_path_buf(), |a, p| a.join(p))
    }

    #[test]
    fn explicit_flag_must_exist() {
        let missing = std::env::temp_dir().join("lumencast-no-such-scenarios-dir");
        let err = resolve_scenarios_dir(Some(&missing))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a directory"), "{err}");
    }

    #[test]
    fn candidates_cover_protocol_checkout_and_sibling() {
        let cwd = Path::new("/w/lumencast-protocol/interop");
        let exe_dir = Path::new("/w/lumencast-rs/target/release");
        let candidates = scenario_dir_candidates(Some(cwd), Some(exe_dir));

        // cwd is inside the protocol checkout → parent hit.
        assert!(candidates.contains(&scenarios_under(Path::new("/w/lumencast-protocol"))));
        // binary lives in a sibling SDK checkout → sibling hit.
        assert!(candidates.contains(&scenarios_under(
            &Path::new("/w").join("lumencast-protocol")
        )));
        // no duplicates — the two anchors overlap.
        let mut deduped = candidates.clone();
        deduped.dedup();
        assert_eq!(deduped.len(), candidates.len());
    }

    #[test]
    fn candidates_tolerate_missing_anchors() {
        assert!(scenario_dir_candidates(None, None).is_empty());
    }

    #[test]
    fn empty_report_is_not_a_pass() {
        // Guards the regression this discovery fix exists for: an empty
        // report is vacuously `all_passed()`, so the CLI must not rely on
        // it alone to decide its exit code.
        let empty = lumencast_conformance::Report {
            total: 0,
            passed: 0,
            skipped: 0,
            outcomes: Vec::new(),
        };
        assert!(empty.all_passed());
    }
}
