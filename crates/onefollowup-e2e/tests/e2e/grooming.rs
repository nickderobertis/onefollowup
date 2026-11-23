//! The grooming journeys (`tests/groom/`), each driving the built binary as a subprocess.

use crate::groom::world::{Ran, Runner, World};

/// The built `onefollowup`, run in the world's root with the world's environment alone.
pub struct Binary;

impl Runner for Binary {
    fn run(&self, world: &World, args: &[&str]) -> Ran {
        let output = crate::isolated::command(
            assert_cmd::cargo::cargo_bin("onefollowup"),
            &world.root,
            world.environment(),
        )
        .args(args)
        .output()
        .expect("the binary runs");
        Ran {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

crate::groom::groom_journeys!(Binary);
