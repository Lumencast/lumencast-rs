//! `lumencast serve-scenario` — wire LSDP/1 server + test control
//! plane on two separate ports, print the discovery line, run.

use std::io::Write;
use std::net::SocketAddr;

use lumencast_server::test_control::{TestControlState, router as control_router};
use lumencast_server::{MapAuthenticator, Server};
use serde::Serialize;
use tokio::sync::oneshot;

use crate::ServeArgs;

/// JSON discovery line printed to stdout — the matrix driver waits
/// for this before connecting.
#[derive(Debug, Serialize)]
struct Discovery {
    control_url: String,
    ws_url: String,
}

pub(crate) async fn run(args: ServeArgs) -> Result<(), Box<dyn std::error::Error>> {
    // 1. LSDP/1 server with an empty MapAuthenticator. Tokens are
    //    installed at runtime via /test/setup.
    let auth = MapAuthenticator::new();
    let auth_for_control = auth.clone();

    let ws_addr = format!("{}:{}", args.host, args.ws_port);
    let server = Server::builder()
        .listen(ws_addr.clone())
        .auth(auth)
        .build()
        .await?;
    let ws_local: SocketAddr = server.local_addr()?;
    let server_handle = server.handle();

    // 2. Control plane on its own port.
    let control_listener =
        tokio::net::TcpListener::bind(format!("{}:{}", args.host, args.test_control_port)).await?;
    let control_local = control_listener.local_addr()?;

    // Per `interop/CONTROL.md`, the cross-language matrix dials
    // `ws://host:port/lsdp.v1`. Match that.
    let ws_url = format!("ws://{ws_local}/lsdp.v1");
    let control_url = format!("http://{control_local}");

    let control_state = TestControlState {
        server: server_handle,
        auth: auth_for_control,
        ws_url: ws_url.clone(),
    };
    let control_app = control_router(control_state);

    // 3. Print the discovery line BEFORE accepting any connection.
    let discovery = Discovery {
        control_url: control_url.clone(),
        ws_url: ws_url.clone(),
    };
    let line = serde_json::to_string(&discovery)?;
    {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        writeln!(out, "{line}")?;
        out.flush()?;
    }

    // 4. Spawn the LSDP/1 server with graceful shutdown via a oneshot.
    let (ws_stop_tx, ws_stop_rx) = oneshot::channel::<()>();
    let ws_task = tokio::spawn(async move {
        let _ = server
            .run_with_shutdown(async move {
                let _ = ws_stop_rx.await;
            })
            .await;
    });

    // 5. Spawn the control-plane server.
    let (ctrl_stop_tx, ctrl_stop_rx) = oneshot::channel::<()>();
    let ctrl_task = tokio::spawn(async move {
        let _ = axum::serve(control_listener, control_app)
            .with_graceful_shutdown(async move {
                let _ = ctrl_stop_rx.await;
            })
            .await;
    });

    tracing::info!(%ws_url, %control_url, "lumencast serve-scenario ready");

    // 6. Wait for SIGINT/SIGTERM.
    wait_for_shutdown().await;
    tracing::info!("shutting down");

    let _ = ws_stop_tx.send(());
    let _ = ctrl_stop_tx.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let _ = tokio::join!(ws_task, ctrl_task);
    })
    .await;

    Ok(())
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
