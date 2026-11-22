//! The grooming journeys (`tests/groom/`), each driving the built binary as a subprocess.

use crate::groom::world::{Ran, Runner, World};

/// The built `onefollowup`, run in the world's root with the world's environment alone.
struct Binary;

impl Runner for Binary {
    fn run(&self, world: &World, args: &[&str]) -> Ran {
        let output = assert_cmd::Command::cargo_bin("onefollowup")
            .expect("the binary is built")
            .current_dir(&world.root)
            .env_clear()
            .envs(world.environment())
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
