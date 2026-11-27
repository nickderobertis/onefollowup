//! Spike harness, not a journey: price every request a real-board measurement recorded
//! (`scripts/spike-real-board/recordings/<run>/requests.jsonl`) by the loopback's own reading
//! of GitHub's published calculation (`graphql_cost`), beside the cost GitHub reported for it,
//! and write `<run>/loopback-prices.jsonl`. Ignored: it reads a recording, and asserts nothing.
//!
//! `SPIKE_RUN=<dir> cargo test -p onefollowup-e2e --test e2e real_board_prices -- --ignored`

use std::io::{BufRead, Write};

use serde_json::{Value, json};

#[test]
#[ignore = "a spike harness over a recorded real-board run; set SPIKE_RUN"]
fn price_recorded_requests_by_the_loopback_calculation() {
    let run = std::path::PathBuf::from(std::env::var("SPIKE_RUN").expect("SPIKE_RUN names a run"));
    let requests = std::fs::File::open(run.join("requests.jsonl")).expect("the run's requests");
    let mut out =
        std::fs::File::create(run.join("loopback-prices.jsonl")).expect("the prices file");
    for line in std::io::BufReader::new(requests).lines() {
        let request: Value = serde_json::from_str(&line.expect("a line")).expect("JSON");
        let Some(sha) = request["sha"].as_str() else {
            continue;
        };
        if request.get("refused").is_some() {
            continue;
        }
        let document = std::fs::read_to_string(run.join(format!("documents/{sha}.graphql")))
            .expect("the recorded document");
        let loopback = crate::graphql_cost::points(&document, &request["variables"]);
        writeln!(
            out,
            "{}",
            json!({
                "seq": request["seq"],
                "document": request["document"],
                "github": request["cost"],
                "loopback": loopback,
            })
        )
        .expect("written");
    }
}
