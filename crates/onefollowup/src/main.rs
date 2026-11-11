//! The `onefollowup` binary: the command line over the library.

use std::process::ExitCode;

fn main() -> ExitCode {
    onefollowup::cli::main()
}
