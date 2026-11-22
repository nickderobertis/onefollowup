//! Running a configured command — a hook or a route's `command` — with a JSON document on its
//! stdin, in the verb's own working directory and environment.
//!
//! A command is found on the `PATH` of that environment. On Windows, which starts only
//! executables and batch files, a name without an extension is also tried with each `PATHEXT`
//! extension, and a script is started through the interpreter its `#!` line names, found on
//! the same `PATH` — as Git for Windows runs a hook — so one configuration naming a shell
//! script runs on every platform.

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
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
    let (executable, leading) = launcher(program, working_directory, environment, Host::current())
        .map_err(|error| format!("`{program}` could not be started: {error}"))?;
    let mut command = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        if leading.is_empty() {
            command.args(arguments);
        } else {
            // An interpreter a `#!` line names is, on Windows, most often a Cygwin or MSYS
            // shell, whose runtime expands wildcards and braces in every argument not quoted
            // on its command line: `{context}` would reach the script as `context`. Every
            // argument is quoted, so each reaches the script as configured.
            for argument in leading.iter().map(|one| one.to_string_lossy().into_owned()) {
                command.raw_arg(quoted(&argument));
            }
            for argument in arguments {
                command.raw_arg(quoted(argument));
            }
        }
    }
    #[cfg(not(windows))]
    {
        command.args(leading).args(arguments);
    }
    let mut child = command
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

/// How the host starts a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    /// The kernel finds a script's interpreter on its `#!` line itself.
    Unix,
    /// Only executables and batch files start; a name without an extension may mean any
    /// `PATHEXT` extension, and a variable's name is matched without regard to case.
    Windows,
}

impl Host {
    const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// What starts `program`: the executable, and the arguments that go before the configured
/// ones — on Windows, a script's interpreter and the script.
///
/// # Errors
///
/// On Windows, a script whose `#!` line names no interpreter, or one not on `PATH`.
fn launcher(
    program: &str,
    working_directory: &Path,
    environment: &[(String, String)],
    host: Host,
) -> Result<(PathBuf, Vec<OsString>), String> {
    let found = resolve(program, environment, host);
    if host == Host::Unix {
        return Ok((found, Vec::new()));
    }
    let script = working_directory.join(&found);
    let Some(line) = shebang(&script) else {
        return Ok((found, Vec::new()));
    };
    let (interpreter, options) =
        interpreter(&line).ok_or_else(|| format!("its `#!` line `{line}` names no interpreter"))?;
    let started = resolve(&interpreter, environment, host);
    if !started.is_absolute() {
        return Err(format!(
            "its interpreter `{interpreter}` (from its `#!` line) is not on PATH"
        ));
    }
    let mut leading: Vec<OsString> = options.into_iter().map(OsString::from).collect();
    leading.push(script.into_os_string());
    Ok((started, leading))
}

/// `argument` quoted as a Windows command line quotes one, whatever it holds: within double
/// quotes, a double quote and the backslashes before it escaped, and the backslashes before
/// the closing quote doubled.
#[cfg(any(windows, test))]
fn quoted(argument: &str) -> String {
    let mut line = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                line.push_str(&"\\".repeat(backslashes * 2 + 1));
                line.push('"');
                backslashes = 0;
            }
            _ => {
                line.push_str(&"\\".repeat(backslashes));
                line.push(character);
                backslashes = 0;
            }
        }
    }
    line.push_str(&"\\".repeat(backslashes * 2));
    line.push('"');
    line
}

/// The `#!` line `path` opens with, without the `#!`, or `None` for a file that does not open
/// with one or cannot be read.
fn shebang(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    line.strip_prefix("#!").map(|rest| rest.trim().to_owned())
}

/// The interpreter a `#!` line names, by its file name, and the options it hands it: for
/// `/usr/bin/env bash -e`, `bash` with `-e`; for `/bin/sh`, `sh` with none.
fn interpreter(line: &str) -> Option<(String, Vec<String>)> {
    let mut words = line.split_whitespace();
    let named = words.next()?;
    let name = named.rsplit(['/', '\\']).next().unwrap_or(named);
    let name = if name == "env" { words.next()? } else { name };
    Some((name.to_owned(), words.map(str::to_owned).collect()))
}

/// `program` found on the `PATH` of `environment`, which is not this process's own when the
/// library was handed another; a name with a separator is taken as given, and one found
/// nowhere is left to the operating system.
fn resolve(program: &str, environment: &[(String, String)], host: Host) -> PathBuf {
    if program.contains(std::path::MAIN_SEPARATOR) || program.contains('/') {
        return program.into();
    }
    let variable = |wanted: &str| {
        environment
            .iter()
            .find(|(name, _)| match host {
                Host::Unix => name == wanted,
                Host::Windows => name.eq_ignore_ascii_case(wanted),
            })
            .map(|(_, value)| value.clone())
    };
    let extensions: Vec<String> = match host {
        Host::Windows if Path::new(program).extension().is_none() => variable("PATHEXT")
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    let path = variable("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .flat_map(|directory| {
            extensions
                .iter()
                .map(|extension| directory.join(format!("{program}{extension}")))
                .chain(std::iter::once(directory.join(program)))
                .collect::<Vec<_>>()
        })
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| program.into())
}

#[cfg(test)]
mod tests {
    //! How each host starts a configured command, Windows's rules applied to a directory this
    //! host holds: the hosted Windows job runs the journeys that start them for real.

    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use super::{Host, launcher, quoted};

    /// A directory holding `files`, each `(name, text)`, and the environment whose `Path`
    /// (spelled as Windows spells it) and `PATHEXT` name it.
    fn bin(files: &[(&str, &str)]) -> (tempfile::TempDir, Vec<(String, String)>) {
        let directory = tempfile::tempdir().expect("a directory");
        for (name, text) in files {
            let path = directory.path().join(name);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
            std::fs::write(path, text).expect("a file");
        }
        let environment = vec![
            ("Path".to_owned(), directory.path().display().to_string()),
            ("PATHEXT".to_owned(), ".COM;.EXE;.CMD".to_owned()),
        ];
        (directory, environment)
    }

    fn started(
        program: &str,
        at: &Path,
        environment: &[(String, String)],
    ) -> (PathBuf, Vec<OsString>) {
        launcher(program, at, environment, Host::Windows).expect("it starts")
    }

    #[test]
    fn on_windows_a_script_starts_through_the_interpreter_its_first_line_names() {
        let (directory, environment) = bin(&[
            ("answer-batch", "#!/bin/sh\necho answered\n"),
            ("sh.EXE", "MZ"),
            ("bash.EXE", "MZ"),
            ("strict", "#!/usr/bin/env bash -e\nfalse\n"),
        ]);
        let root = directory.path();
        assert_eq!(
            started("answer-batch", root, &environment),
            (root.join("sh.EXE"), vec![root.join("answer-batch").into()])
        );
        assert_eq!(
            started("strict", root, &environment),
            (
                root.join("bash.EXE"),
                vec!["-e".into(), root.join("strict").into()]
            )
        );
    }

    #[test]
    fn on_windows_a_script_named_by_path_is_read_against_the_working_directory() {
        let (directory, environment) = bin(&[
            ("scripts/inputs.sh", "#!/bin/sh\necho '{}'\n"),
            ("sh.EXE", "MZ"),
        ]);
        let root = directory.path();
        assert_eq!(
            started("scripts/inputs.sh", root, &environment),
            (
                root.join("sh.EXE"),
                vec![root.join("scripts/inputs.sh").into()]
            )
        );
    }

    #[test]
    fn on_windows_a_bare_name_is_found_with_a_pathext_extension_and_started_itself() {
        let (directory, environment) = bin(&[("notify.CMD", "@echo off\n"), ("git.EXE", "MZ")]);
        let root = directory.path();
        assert_eq!(
            started("notify", root, &environment),
            (root.join("notify.CMD"), Vec::new())
        );
        assert_eq!(
            started("git", root, &environment),
            (root.join("git.EXE"), Vec::new())
        );
        // Found nowhere, it is left to the operating system to refuse.
        assert_eq!(
            started("missing", root, &environment),
            (PathBuf::from("missing"), Vec::new())
        );
    }

    #[test]
    fn on_windows_a_script_whose_interpreter_cannot_be_found_is_refused_naming_it() {
        let (directory, environment) = bin(&[
            ("needs-zsh", "#!/bin/zsh\n"),
            ("names-nothing", "#!\n"),
            ("env-alone", "#!/usr/bin/env\n"),
        ]);
        let root = directory.path();
        let refused = |program: &str| {
            launcher(program, root, &environment, Host::Windows).expect_err("refused")
        };
        assert_eq!(
            refused("needs-zsh"),
            "its interpreter `zsh` (from its `#!` line) is not on PATH"
        );
        assert_eq!(
            refused("names-nothing"),
            "its `#!` line `` names no interpreter"
        );
        assert_eq!(
            refused("env-alone"),
            "its `#!` line `/usr/bin/env` names no interpreter"
        );
    }

    #[test]
    fn on_unix_a_script_is_started_itself_and_path_is_matched_exactly() {
        let (directory, mut environment) = bin(&[("hook", "#!/bin/sh\n"), ("sh", "")]);
        let root = directory.path();
        assert_eq!(
            launcher("hook", root, &environment, Host::Unix),
            Ok((PathBuf::from("hook"), Vec::new())),
            "`Path` is not `PATH` off Windows"
        );
        environment[0].0 = "PATH".to_owned();
        assert_eq!(
            launcher("hook", root, &environment, Host::Unix),
            Ok((root.join("hook"), Vec::new()))
        );
    }

    #[test]
    fn an_argument_is_quoted_as_a_windows_command_line_reads_it_back() {
        assert_eq!(quoted("{context}"), r#""{context}""#);
        assert_eq!(quoted("--plain"), r#""--plain""#);
        assert_eq!(quoted(""), r#""""#);
        assert_eq!(
            quoted(r"C:\Users\runner\a file.md"),
            r#""C:\Users\runner\a file.md""#
        );
        assert_eq!(quoted(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quoted(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quoted(r"C:\dir\"), r#""C:\dir\\""#);
    }
}
