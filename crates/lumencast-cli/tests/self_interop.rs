//! Self-interop test : spawn `lumencast serve-scenario`, drive the
//! conformance harness against it through the test control plane.
//!
//! Acceptance criterion #4 from `chantier-interop-rs.md`.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use lumencast_conformance::Tag;
use lumencast_conformance::harness::{Config, ScenariosSource, Target, run};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Debug, Deserialize)]
struct Discovery {
    control_url: String,
    ws_url: String,
}

fn cli_path() -> std::path::PathBuf {
    // The integration test crate sits next to its binary in target/.
    // CARGO_BIN_EXE_<name> is set by Cargo for [[bin]] in the same
    // package. Our binary is in lumencast-cli (this crate), named
    // `lumencast`.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_lumencast"))
}

#[tokio::test]
async fn self_interop_smoke() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new(cli_path())
        .args([
            "serve-scenario",
            "--ws-port",
            "0",
            "--test-control-port",
            "0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    // Read the discovery line from stdout (1 second timeout).
    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = BufReader::new(stdout).lines();
    let discovery_line = tokio::time::timeout(Duration::from_secs(5), reader.next_line())
        .await??
        .ok_or("server died before printing discovery line")?;
    let discovery: Discovery = serde_json::from_str(&discovery_line)?;
    assert!(discovery.ws_url.starts_with("ws://"));
    assert!(discovery.control_url.starts_with("http://"));
    assert!(discovery.ws_url.ends_with("/lsdp.v1"));

    // 1. /test/health roundtrip via the harness's ControlClient.
    let control = lumencast_conformance::ControlClient::new(discovery.control_url.clone());
    let started = Instant::now();
    let health = loop {
        match control.health().await {
            Ok(h) => break h,
            Err(_) if started.elapsed() < Duration::from_secs(2) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(format!("health failed: {e}").into()),
        }
    };
    assert_eq!(health.status, "ok");
    assert_eq!(health.control_plane_version, 1);
    assert_eq!(health.server.as_deref(), Some("lumencast-rs"));

    // 2. Run the harness with an empty scenario set — exercises the
    //    full path (scenario loader, filters, control client) and
    //    must report 0 / 0 passed without errors.
    let report = run(Config {
        target: Target::Server {
            ws_url: discovery.ws_url.clone(),
            control_url: discovery.control_url.clone(),
        },
        tags: vec![Tag::Required],
        scenario_filter: None,
        tokens: BTreeMap::new(),
        scenarios: ScenariosSource::Empty,
    })
    .await?;
    assert_eq!(report.total, 0);
    assert!(report.all_passed());

    // 3. Run an inline scenario that primes the server, opens a WS,
    //    and walks subscribe → snapshot → input → delta → state.
    let scenario_yaml = inline_scenario(&discovery.ws_url);
    let report = run(Config {
        target: Target::Server {
            ws_url: discovery.ws_url.clone(),
            control_url: discovery.control_url.clone(),
        },
        tags: vec![Tag::Required],
        scenario_filter: None,
        tokens: canonical_tokens(),
        scenarios: ScenariosSource::Inline(vec![("inline-smoke".into(), scenario_yaml)]),
    })
    .await?;
    for o in &report.outcomes {
        if !o.passed {
            eprintln!(
                "[scenario] {}: {}",
                o.name,
                o.message.as_deref().unwrap_or("")
            );
        }
    }
    assert_eq!(report.total, 1);
    assert!(report.all_passed(), "inline scenario failed");

    // Tear down the child process.
    let _ = child.kill().await;
    let _ = child.wait().await;
    Ok(())
}

fn canonical_tokens() -> BTreeMap<String, String> {
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

fn inline_scenario(_ws_url: &str) -> String {
    // A minimal end-to-end scenario covering the protocol + control
    // surfaces. The harness drives setup/state/emit; the player drives
    // the WebSocket. The scenario:
    //   1. Subscribe as operator → expect snapshot
    //   2. Operator input → expect delta echo
    //   3. Server-driven /test/emit → expect delta
    //   4. expect-server-state matches what we expect.
    r"
name: inline-smoke
tag: required
target: any
bundles:
  - id: t
    inline:
      v: 1
      kind: frame
      id: t
      state:
        title: Hello
        count: 0
initial_state:
  title: Hello
  count: 0
steps:
  - kind: client-sends
    frame:
      v: 1
      type: subscribe
      token: $TOKEN_OPERATOR
      scene: t
  - kind: server-sends
    frame:
      v: 1
      type: snapshot
      seq: 1
      scene_id: t
  - kind: expect-runtime-state
    state:
      title: Hello
      count: 0
  - kind: client-sends
    frame:
      v: 1
      type: input
      patches:
        - path: __inputs.title
          value: Updated
  - kind: server-sends
    frame:
      v: 1
      type: delta
      seq: 2
  - kind: expect-runtime-state
    state:
      __inputs.title: Updated
  - kind: server-emits
    frame:
      v: 1
      type: delta
      seq: 3
      patches:
        - path: count
          value: 7
  - kind: expect-server-state
    state:
      title: Hello
      __inputs.title: Updated
      count: 7
"
    .to_string()
}
