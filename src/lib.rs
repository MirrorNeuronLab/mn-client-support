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
