//! Client-side contracts shared by the command line and REST server.
pub mod adapter;
mod app;
pub mod backend;
pub mod catalog;
pub mod config;
pub mod control;
mod error;
pub mod nodes;
pub mod resources;
pub mod wit;

pub use app::run;
pub use config as connection;
pub use error::{Error, Result};

#[cfg(target_os = "wasi")]
mod session;
#[cfg(target_os = "wasi")]
pub mod wasi;

/// The GAR tag is supplied by release builds. Local builds report the crate version.
pub const fn release_version(package: &'static str) -> &'static str {
    match option_env!("MN_CLIENT_RELEASE_VERSION") {
        Some(version) => version,
        None => package,
    }
}
