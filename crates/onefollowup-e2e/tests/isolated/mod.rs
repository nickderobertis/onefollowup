//! A child process whose environment holds only what its caller names, plus what the host
//! needs any process to have. Every test that starts a process with a cleared environment
//! starts it through [`command`], in both suites: `onefollowup-e2e`'s, and the `onefollowup`
//! crate's, which includes this file by path.

#![allow(
    dead_code,
    reason = "each test crate including this module uses its own part of it"
)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The variables every child gets from this process's own environment, because the host
/// needs them. On Windows that is `SystemRoot`. Winsock loads its service providers from
/// under it, so in a process without it, every socket fails with WSAEPROVIDERFAILEDINIT
/// (os error 10106). Nothing elsewhere.
pub const HOST: &[&str] = if cfg!(windows) { &["SystemRoot"] } else { &[] };

/// The variables the platform sets in every process it starts, whatever environment that
/// process was given, so a child holds them though no parent passed them. On macOS that is
/// `__CF_USER_TEXT_ENCODING`: CoreFoundation, which every process linking it initialises at
/// launch, sets it to the user's text encoding (`0x<uid>:<encoding>:<region>`) when it is
/// absent. Nothing elsewhere. No child receives these from [`command`]; a test of what a child
/// received exempts them, and only them.
pub const PLATFORM_SET: &[&str] = if cfg!(target_os = "macos") {
    &["__CF_USER_TEXT_ENCODING"]
} else {
    &[]
};

/// No variable beyond [`HOST`].
pub const NOTHING: [(&str, &str); 0] = [];

/// Where git is kept off Windows.
pub const UNIX_TOOLS: &[&str] = &["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin"];

/// The built `onefollowup` that every test starting it as a process runs, in both suites: the
/// one in the target profile directory holding this test executable (`<profile>/deps/`'s
/// parent), where `cargo build -p onefollowup` puts it. It reads no `CARGO_BIN_EXE_` variable:
/// Cargo sets that only for the tests of the package defining the binary, which
/// `onefollowup-e2e` is not. Panics naming the missing path and how to build it.
pub fn binary() -> PathBuf {
    let test = std::env::current_exe().expect("this test executable's path");
    let mut profile = test.parent().expect("its directory").to_path_buf();
    if profile.ends_with("deps") {
        profile.pop();
    }
    let binary = profile.join(format!("onefollowup{}", std::env::consts::EXE_SUFFIX));
    assert!(
        binary.is_file(),
        "the built onefollowup is not at {}: run `cargo build -p onefollowup` in the same profile first",
        binary.display()
    );
    binary
}

/// `program` in `directory`, with an environment of `variables` and [`HOST`] alone: nothing
/// else of this process's environment reaches it.
pub fn command<K, V>(
    program: impl AsRef<OsStr>,
    directory: &Path,
    variables: impl IntoIterator<Item = (K, V)>,
) -> Command
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.current_dir(directory).env_clear();
    for name in HOST {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(variables);
    command
}

/// A `PATH` of `first`, then where the host keeps the tools a journey's commands run: `unix`
/// off Windows, and on Windows — which keeps them in no fixed directory — each directory of
/// this process's own `PATH` holding `sh` or `git`. Joined as the host joins one.
pub fn search_path(first: Option<&Path>, unix: &[&str]) -> String {
    let host: Vec<PathBuf> = if cfg!(windows) {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&inherited)
            .filter(|directory| {
                ["sh.exe", "git.exe"]
                    .iter()
                    .any(|tool| directory.join(tool).is_file())
            })
            .collect()
    } else {
        unix.iter().map(PathBuf::from).collect()
    };
    std::env::join_paths(first.map(Path::to_path_buf).into_iter().chain(host))
        .expect("a PATH joins")
        .into_string()
        .expect("a UTF-8 PATH")
}
