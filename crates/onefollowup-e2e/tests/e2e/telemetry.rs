//! Where the budgets' telemetry lives: one JSON file per budget, `{"value", "detail"}`, named
//! after the budget's id, under `<target>/<profile>/telemetry/onefollowup-e2e` — found from the
//! running test executable, so the journeys that write it and the runner that reads it agree
//! on it whatever the target directory is. [`DIRECTORY`] names another.

use std::path::PathBuf;

use serde_json::json;

/// The variable that, when set, names the telemetry directory in place of the build's.
pub const DIRECTORY: &str = "ONEFOLLOWUP_BUDGET_TELEMETRY_DIR";

/// The directory below the build directory.
pub const BELOW_BUILD: &str = "telemetry/onefollowup-e2e";

/// The directory the telemetry is in.
pub fn directory() -> PathBuf {
    if let Some(directory) = std::env::var_os(DIRECTORY) {
        return PathBuf::from(directory);
    }
    let executable = std::env::current_exe().expect("this test executable has a path");
    // `<target>/<profile>/deps/<executable>`.
    executable
        .parent()
        .and_then(std::path::Path::parent)
        .expect("a test executable sits inside a build directory")
        .join(BELOW_BUILD)
}

/// Record `value` as `budget`'s figure, and how it was counted.
pub fn record(budget: &str, value: usize, detail: &str) {
    let directory = directory();
    std::fs::create_dir_all(&directory).expect("the telemetry directory is writable");
    std::fs::write(
        directory.join(format!("{budget}.json")),
        json!({"value": value, "detail": detail}).to_string(),
    )
    .expect("the telemetry file is writable");
}

/// The figure and detail `budget`'s journey recorded, or why there is none.
pub fn recorded(budget: &str) -> Result<(f64, Option<String>), String> {
    let id = regex::Regex::new(onebudgetspec_core::model::ID_PATTERN)
        .expect("onebudgetspec's id pattern compiles");
    if !id.is_match(budget) {
        return Err(format!(
            "{budget:?} is not a budget id ({}); name one of budgets.yaml's",
            onebudgetspec_core::model::ID_PATTERN
        ));
    }
    let path = directory().join(format!("{budget}.json"));
    let text = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "{budget}: no telemetry at {} ({error}); the journey that measures it records it while the onefollowup-e2e journeys run, so run `cargo nextest run -p onefollowup-e2e` and re-run",
            path.display()
        )
    })?;
    let recorded: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{budget}: {} is not JSON ({error})", path.display()))?;
    let value = recorded["value"].as_u64().ok_or_else(|| {
        format!(
            "{budget}: {} records no `value` that is a whole, non-negative number: {text}",
            path.display()
        )
    })?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "a count of store operations is far below 2^52"
    )]
    let value = value as f64;
    Ok((value, recorded["detail"].as_str().map(str::to_owned)))
}
