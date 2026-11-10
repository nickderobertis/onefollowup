//! What every journey shares: the built binary, run in a directory of its own with an
//! environment holding nothing of the developer's.
#![allow(
    dead_code,
    reason = "each test binary compiles this module and uses only part of it"
)]

use std::path::Path;

use assert_cmd::Command;

/// The built `onefollowup`, in `directory`, with no `ONEFOLLOWUP_` variable or anything
/// else of this process's environment.
pub fn onefollowup(directory: &Path) -> Command {
    let mut command = Command::cargo_bin("onefollowup").expect("the binary is built");
    command.current_dir(directory).env_clear();
    command
}

/// The JSON document a successful run printed.
pub fn json(output: &std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the verb printed JSON")
}
