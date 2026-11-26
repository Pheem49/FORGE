use std::borrow::Cow;
use std::convert::Infallible;
use std::fmt;
use std::path::PathBuf;

use http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::config::RuntimeKind;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Host lifecycle, startup, and configuration errors returned by [`crate::App::run`] and [`crate::App::headless`].
#[derive(Debug)]
pub enum Error {
    Config {
        path: Option<PathBuf>,
        message: String,
    },
    AssetsNotFound {
        dir: PathBuf,
    },
    MissingState {
        command: &'static str,
        type_name: &'static str,
    },
    DuplicateCommand {
        command: &'static str,
    },
    ReservedCommand {
        command: &'static str,
    },
    RuntimeUnavailable {
        requested: RuntimeKind,
        reason: String,
    },
    NotMainThread,
    SetupFailed(String),
    Runtime(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config {
                path: Some(p),
                message,
            } => write!(f, "configuration error in {}: {message}", p.display()),
            Self::Config {
                path: None,
                message,
            } => write!(f, "configuration error: {message}"),
            Self::AssetsNotFound { dir } => {
                write!(f, "UI assets directory not found: {}", dir.display())
            }
            Self::MissingState { command, type_name } => write!(
                f,
                "command '{command}' requires State<{type_name}>, which was not registered via App::state() or SetupContext::insert_state()"
            ),
            Self::DuplicateCommand { command } => {
                write!(f, "duplicate command registration for '{command}'")
            }
            Self::ReservedCommand { command } => write!(
                f,
                "command name '{command}' uses reserved '__forge' prefix or invalid characters"
            ),
            Self::RuntimeUnavailable { requested, reason } => {
                write!(f, "runtime '{requested}' is unavailable: {reason}")
            }
            Self::NotMainThread => write!(
                f,
                "App::run() must be invoked on the OS main thread for desktop windowing runtimes"
            ),
            Self::SetupFailed(msg) => write!(f, "application setup failed: {msg}"),
            Self::Runtime(msg) => write!(f, "runtime error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Structured error returned by async IPC commands and hydrated into a rejected JS `Promise`
/// carrying `{ code, message, details }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeError {
    pub code: Cow<'static, str>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ForgeError {
    pub fn new(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn bad_request(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }

    pub fn with_details<T: Serialize>(mut self, details: T) -> Self {
        self.details = serde_json::to_value(details).ok();
        self
    }

    pub(crate) fn status_code(&self) -> StatusCode {
        match self.code.as_ref() {
            "FORGE_FORBIDDEN" => StatusCode::FORBIDDEN,
            "FORGE_UNKNOWN_COMMAND" => StatusCode::NOT_FOUND,
            "FORGE_INVALID_PAYLOAD" => StatusCode::BAD_REQUEST,
            "FORGE_HANDLER_PANIC" | "APP_ERROR" => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ForgeError {}

impl From<&'static str> for ForgeError {
    fn from(msg: &'static str) -> Self {
        Self::new("APP_ERROR", msg)
    }
}

impl From<String> for ForgeError {
    fn from(msg: String) -> Self {
        Self::new("APP_ERROR", msg)
    }
}

impl From<std::io::Error> for ForgeError {
    fn from(err: std::io::Error) -> Self {
        Self::new("IO_ERROR", err.to_string())
    }
}

impl From<Infallible> for ForgeError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}
