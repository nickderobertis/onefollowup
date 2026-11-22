//! Where the budgets' telemetry lives: one JSON file per budget, `{"value", "detail"}`, named
//! after the budget's id, under `<target>/<profile>/telemetry/onefollowup-e2e` — found from the
//! running test executable, so the journeys that write it and the runner that reads it agree
//! on it whatever the target directory is. [`DIRECTORY`] names another. A budget measured by
//! several journeys keeps one file per journey under a directory named after it instead, and
//! its figure is the largest of theirs. A journey's own record — what it observed, for a reader
//! after the run — sits beside them as `<journey>.json`.

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

/// Record `value` as the figure `journey` measured of `budget`, one of several journeys
/// measuring it, and how it was counted.
pub fn record_part(budget: &str, journey: &str, value: usize, detail: &str) {
    let directory = directory().join(budget);
    std::fs::create_dir_all(&directory).expect("the telemetry directory is writable");
    std::fs::write(
        directory.join(format!("{journey}.json")),
        json!({"value": value, "detail": detail}).to_string(),
    )
    .expect("the telemetry file is writable");
}

/// Keep what `journey` observed, for a reader after the run.
pub fn record_journey(journey: &str, observed: &serde_json::Value) {
    let directory = directory();
    std::fs::create_dir_all(&directory).expect("the telemetry directory is writable");
    std::fs::write(
        directory.join(format!("{journey}.json")),
        serde_json::to_string_pretty(observed).expect("JSON") + "\n",
    )
    .expect("the record is writable");
}

/// The largest figure the journeys measuring `budget` recorded, the detail naming each
/// journey's; `None` when no journey recorded a part of it.
fn recorded_parts(budget: &str) -> Result<Option<(f64, Option<String>)>, String> {
    let directory = directory().join(budget);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(None);
    };
    let mut parts: Vec<(String, f64, Option<String>)> = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{budget}: {} is unreadable ({error})", directory.display()))?
            .path();
        let Some(journey) = path
            .extension()
            .filter(|extension| *extension == "json")
            .and(path.file_stem())
        else {
            continue;
        };
        let (value, detail) = read(budget, &path)?;
        parts.push((journey.to_string_lossy().into_owned(), value, detail));
    }
    parts.sort_by(|a, b| a.0.cmp(&b.0));
    let Some(largest) = parts.iter().max_by(|a, b| a.1.total_cmp(&b.1)) else {
        return Ok(None);
    };
    let detail = format!(
        "the largest of {} journeys', {}'s: {}",
        parts.len(),
        largest.0,
        parts
            .iter()
            .map(|(journey, value, detail)| format!(
                "[{journey}: {value} — {}]",
                detail.as_deref().unwrap_or("no detail")
            ))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(Some((largest.1, Some(detail))))
}

/// The figure and detail `budget`'s journey recorded — or, for a budget several journeys
/// measure, the largest of theirs — or why there is none.
pub fn recorded(budget: &str) -> Result<(f64, Option<String>), String> {
    let id = regex::Regex::new(onebudgetspec_core::model::ID_PATTERN)
        .expect("onebudgetspec's id pattern compiles");
    if !id.is_match(budget) {
        return Err(format!(
            "{budget:?} is not a budget id ({}); name one of budgets.yaml's",
            onebudgetspec_core::model::ID_PATTERN
        ));
    }
    if let Some(parts) = recorded_parts(budget)? {
        return Ok(parts);
    }
    read(budget, &directory().join(format!("{budget}.json")))
}

/// The figure and detail one telemetry file of `budget` holds.
fn read(budget: &str, path: &std::path::Path) -> Result<(f64, Option<String>), String> {
    let text = std::fs::read_to_string(path).map_err(|error| {
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
