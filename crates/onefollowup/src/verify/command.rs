//! Running a configured command — a hook or a route's `command` — with a JSON document on its
//! stdin, in the verb's own working directory and environment.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// What a command did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Its exit code; `-1` when a signal ended it.
    pub exit: i32,
    /// What it printed on stdout.
    pub stdout: String,
    /// What it printed on stderr.
    pub stderr: String,
}

/// Run `argv` with `stdin` written to it.
///
/// # Errors
///
/// A command that names nothing, or that could not be started.
pub fn run(
    argv: &[String],
    stdin: &str,
    working_directory: &Path,
    environment: &[(String, String)],
) -> Result<Ran, String> {
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| "the command is empty".to_owned())?;
    let mut child = Command::new(resolve(program, environment))
        .args(arguments)
        .current_dir(working_directory)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("`{program}` could not be started: {error}"))?;
    if let Some(mut input) = child.stdin.take() {
        // A command that exits without reading its input is its own business; its exit
        // code says how it went.
        let _ignored = input.write_all(stdin.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("`{program}` could not be waited for: {error}"))?;
    Ok(Ran {
        exit: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// `program` found on the `PATH` of `environment`, which is not this process's own when the
/// library was handed another; a name with a separator is taken as given.
fn resolve(program: &str, environment: &[(String, String)]) -> std::path::PathBuf {
    if program.contains(std::path::MAIN_SEPARATOR) || program.contains('/') {
        return program.into();
    }
    let path = environment
        .iter()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| program.into())
}
