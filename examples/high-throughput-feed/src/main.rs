//! Stress-test demo — emits a configurable rate of deltas to
//! demonstrate the protocol's leaf-grain throughput.
//!
//! Defaults: 1000 deltas/sec, 50 paths in rotation.
//!
//! ```sh
//! cargo run --release -p high-throughput-feed -- --rate 1000 --paths 50
//! ```

use std::time::{Duration, Instant};

use lumencast_server::{MapAuthenticator, Role, Server};
use serde_json::json;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone)]
struct Args {
    rate: u32,
    paths: u32,
    bind: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            rate: 1000,
            paths: 50,
            bind: "127.0.0.1:4001".into(),
        }
    }
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--rate" => {
                args.rate = iter
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(args.rate);
            }
            "--paths" => {
                args.paths = iter
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(args.paths);
            }
            "--bind" => {
                if let Some(b) = iter.next() {
                    args.bind = b;
                }
            }
            _ => {}
        }
    }
    args
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = parse_args();
    info!(?args, "starting high-throughput-feed");

    let mut auth = MapAuthenticator::new();
    auth.insert("viewer", Role::Viewer);

    let srv = Server::builder()
        .listen(args.bind.clone())
        .auth(auth)
        .build()
        .await?;

    let scene = srv.new_scene("feed")?;

    // Seed all paths to zero.
    let seed: Vec<(String, serde_json::Value)> = (0..args.paths)
        .map(|i| (format!("metric.{i}"), json!(0)))
        .collect();
    scene.seed(seed);

    info!(addr = ?srv.local_addr()?, "feed listening");

    // Producer task — schedules `rate` patches per second across the
    // configured number of paths.
    let driver = scene.clone();
    let rate = args.rate.max(1);
    let paths = args.paths.max(1);
    tokio::spawn(async move {
        let interval = Duration::from_secs_f64(1.0 / f64::from(rate));
        let mut next = Instant::now() + interval;
        let mut counter: u64 = 0;
        let mut emitted: u64 = 0;
        let mut last_report = Instant::now();
        loop {
            tokio::time::sleep_until(next.into()).await;
            next += interval;
            let path = format!("metric.{}", counter % u64::from(paths));
            counter = counter.wrapping_add(1);
            let _ = driver.set(&path, json!(counter));
            emitted += 1;

            if last_report.elapsed() >= Duration::from_secs(5) {
                info!(emitted, "patches in last window");
                last_report = Instant::now();
                emitted = 0;
            }
        }
    });

    srv.run_with_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutting down");
    })
    .await?;

    Ok(())
}
