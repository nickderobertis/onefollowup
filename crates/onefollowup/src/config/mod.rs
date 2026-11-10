//! The configuration: its typed shape, its shipped defaults, and how the effective value
//! of every key is resolved — command-line flag, then `ONEFOLLOWUP_` variable, then the
//! `onefollowup.yaml` file, then the default.

mod resolve;
mod types;
mod validate;

pub use resolve::{
    CONFIG_VARIABLE, ConfigError, FILE_NAME, Inputs, Origin, Settings, VARIABLE_PREFIX, defaults,
    resolve,
};
pub use types::*;
