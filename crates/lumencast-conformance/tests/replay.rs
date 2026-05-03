//! Conformance smoke tests: round-trip every baseline fixture.

use lumencast_conformance::harness::{Config, Target, run};
use lumencast_conformance::local;

#[test]
fn fixtures_round_trip() {
    local::round_trips_ok().expect("fixtures must round-trip");
}

#[tokio::test]
async fn empty_run_returns_zero_outcomes() {
    let report = run(Config {
        target: Target::Server("ws://127.0.0.1:0/ws".into()),
        tags: vec!["required".into()],
    })
    .await
    .expect("run");
    assert_eq!(report.total, 0);
    assert!(report.all_passed());
}
