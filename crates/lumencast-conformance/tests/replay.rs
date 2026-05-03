//! Conformance smoke tests: round-trip every baseline fixture.

use std::collections::BTreeMap;

use lumencast_conformance::harness::{Config, ScenariosSource, Target, run};
use lumencast_conformance::local;
use lumencast_conformance::scenario::Tag;

#[test]
fn fixtures_round_trip() {
    local::round_trips_ok().expect("fixtures must round-trip");
}

#[tokio::test]
async fn empty_run_returns_zero_outcomes() {
    let report = run(Config {
        target: Target::Server {
            ws_url: "ws://127.0.0.1:0/ws".into(),
            control_url: "http://127.0.0.1:0".into(),
        },
        tags: vec![Tag::Required],
        scenario_filter: None,
        tokens: BTreeMap::new(),
        scenarios: ScenariosSource::Empty,
    })
    .await
    .expect("run");
    assert_eq!(report.total, 0);
    assert!(report.all_passed());
}
