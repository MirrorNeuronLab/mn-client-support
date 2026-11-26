//! Client-side contracts shared by the command line and REST server.
pub mod catalog;
pub mod connection;
pub mod control;
pub mod resources;

use serde_json::{Value, json};
use std::collections::HashMap;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub status: u16,
    pub exit_code: i32,
    pub hint: Option<String>,
    pub details: Value,
    pub headers: Box<HashMap<String, String>>,
}

impl Error {
    pub fn new(status: u16, code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            status,
            exit_code: if status == 422 { 2 } else { 1 },
            hint: None,
            details: json!({}),
            headers: Box::default(),
        }
    }
    pub fn invalid(message: &str) -> Self {
        Self::new(422, "MN_VALIDATION_FAILED", message)
    }
    pub fn protocol() -> Self {
        Self::new(
            502,
            "MN_RUNTIME_PROTOCOL_ERROR",
            "Invalid response from the runtime.",
        )
    }
    pub fn storage() -> Self {
        Self::new(
            503,
            "MN_CLIENT_CONFIG_INVALID",
            "Cannot read or write the installation configuration.",
        )
    }
    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.to_string());
        self
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}

pub mod adapter;
pub mod backend;
pub mod wit;
