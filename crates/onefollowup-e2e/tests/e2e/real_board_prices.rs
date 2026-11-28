//! Spike harness, not a journey: price every request a real-board measurement recorded
//! (`scripts/spike-real-board/recordings/<run>/requests.jsonl`) by the loopback's own reading
//! of GitHub's published calculation (`graphql_cost`), write each beside the cost GitHub
//! reported for it to `<run>/loopback-prices.jsonl`, and assert the two agree on every request.
//! Ignored: it reads a recording a real-board run left, which no CI run has.
//!
//! `SPIKE_RUN=<dir> cargo test -p onefollowup-e2e --test e2e real_board_prices -- --ignored`

use std::io::{BufRead, Write};

use serde_json::{Value, json};

#[test]
#[ignore = "a spike harness over a recorded real-board run; set SPIKE_RUN"]
fn loopback_prices_every_recorded_request_as_github_reported_it() {
    let run = std::path::PathBuf::from(std::env::var("SPIKE_RUN").expect("SPIKE_RUN names a run"));
    let requests = std::fs::File::open(run.join("requests.jsonl")).expect("the run's requests");
    let mut out =
        std::fs::File::create(run.join("loopback-prices.jsonl")).expect("the prices file");
    let mut priced = 0;
    let mut disagreements = Vec::new();
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
        priced += 1;
        if request["cost"].as_u64() != Some(loopback) {
            disagreements.push(format!(
                "#{} {}: GitHub reported {}, the loopback prices {loopback}",
                request["seq"], request["document"], request["cost"]
            ));
        }
    }
    assert!(priced > 0, "{} records no priced request", run.display());
    assert!(
        disagreements.is_empty(),
        "the loopback's price differs from GitHub's reported cost:\n{}",
        disagreements.join("\n")
    );
}
