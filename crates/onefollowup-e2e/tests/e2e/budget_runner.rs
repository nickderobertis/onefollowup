//! The one runner of `budgets.yaml`, which every budget there names as its command.
//!
//! [`report`] measures nothing: it reads the telemetry the budget's journey recorded while the
//! journeys ran — the budget is the one `ONEBUDGETSPEC_BUDGET_ID` names — and hands that figure
//! to `onebudgetspec check` through `onebudgetspec_core::report`; the comparison with the
//! threshold is onebudgetspec's alone. In an ordinary run no budget is named, so it reports
//! nothing and passes.

use std::env::VarError;
use std::process::Command;

use crate::telemetry;

#[test]
fn report() {
    let budget = match std::env::var("ONEBUDGETSPEC_BUDGET_ID") {
        Ok(budget) => budget,
        Err(VarError::NotPresent) => return,
        Err(VarError::NotUnicode(raw)) => refuse(&format!(
            "ONEBUDGETSPEC_BUDGET_ID {raw:?} is not a budget id: it is not even Unicode"
        )),
    };
    let (value, detail) = telemetry::recorded(&budget).unwrap_or_else(|reason| refuse(&reason));
    let reported = onebudgetspec_core::report(value, detail.as_deref()).unwrap_or_else(|error| {
        refuse(&format!("{budget}'s figure could not be reported: {error}"))
    });
    if !reported {
        refuse(&format!(
            "ONEBUDGETSPEC_RESULT is not set, so {budget}'s figure has nowhere to go; run it through `onebudgetspec check crates/onefollowup-e2e/budgets.yaml`"
        ));
    }
}

/// End the runner with `reason` as the last of its stderr.
fn refuse(reason: &str) -> ! {
    eprintln!("{reason}");
    std::process::exit(1)
}

#[test]
fn the_runner_reports_what_was_recorded_and_refuses_what_it_cannot_read() {
    let directory = tempfile::tempdir().expect("a directory");
    let result = directory.path().join("result.json");
    std::fs::write(
        directory.path().join("budget-recorded.json"),
        r#"{"value": 6, "detail": "six operations"}"#,
    )
    .expect("written");
    let executable = std::env::current_exe().expect("this executable");
    let run = |budget: &str| {
        Command::new(&executable)
            .args(["budget_runner::report", "--exact", "--nocapture"])
            .env(telemetry::DIRECTORY, directory.path())
            .env("ONEBUDGETSPEC_BUDGET_ID", budget)
            .env("ONEBUDGETSPEC_RESULT", &result)
            .output()
            .expect("the runner starts")
    };
    let reported = run("budget-recorded");
    assert!(reported.status.success(), "{reported:?}");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&result).expect("a result")).expect("JSON");
    assert_eq!(written["value"], 6.0);
    assert_eq!(written["detail"], "six operations");
    let missing = run("budget-unrecorded");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no telemetry"));
}
