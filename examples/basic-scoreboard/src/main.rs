//! Minimal scoreboard demo — boots a Lumencast server, registers one
//! scene, and pushes a periodic score delta. Connect a runtime to
//! `ws://127.0.0.1:4000/ws` with `Sec-WebSocket-Protocol: lsdp.v1`.

use std::time::Duration;

use lumencast_server::{MapAuthenticator, Role, Server};
use serde_json::json;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut auth = MapAuthenticator::new();
    auth.insert("op-token", Role::Operator);
    auth.insert("viewer-token", Role::Viewer);

    let srv = Server::builder()
        .listen("127.0.0.1:4000")
        .auth(auth)
        .build()
        .await?;

    let scene = srv.new_scene("main-stage")?;
    scene.seed([
        ("show.title".to_string(), json!("Lumencast demo")),
        ("score.home".to_string(), json!(0)),
        ("score.away".to_string(), json!(0)),
    ]);

    info!(addr = ?srv.local_addr()?, "server listening");

    // Background "match" task: bumps the home score every second.
    let driver = scene.clone();
    tokio::spawn(async move {
        let mut home = 0i64;
        let mut away = 0i64;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if rand_bool() {
                home += 1;
            } else {
                away += 1;
            }
            let _ = driver.emit([
                ("score.home".to_string(), json!(home)),
                ("score.away".to_string(), json!(away)),
            ]);
        }
    });

    // Run until SIGINT / Ctrl-C.
    srv.run_with_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutting down");
    })
    .await?;

    Ok(())
}

fn rand_bool() -> bool {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    nanos.is_multiple_of(2)
}
