use std::borrow::Cow;
use std::convert::Infallible;
use std::fmt;
use std::path::PathBuf;

use http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::manifest::RuntimeKind;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Canonical error codes produced by FORGE's IPC bridge, permission gate, and windowing subsystems.
pub mod codes {
    pub const APP_ERROR: &str = "APP_ERROR";
    pub const IO_ERROR: &str = "IO_ERROR";
    pub const FORBIDDEN: &str = "FORGE_FORBIDDEN";
    pub const PERMISSION_DENIED: &str = "FORGE_PERMISSION_DENIED";
    pub const UNKNOWN_COMMAND: &str = "FORGE_UNKNOWN_COMMAND";
    pub const INVALID_COMMAND: &str = "FORGE_INVALID_COMMAND";
    pub const INVALID_PAYLOAD: &str = "FORGE_INVALID_PAYLOAD";
    pub const DESERIALIZE_ERROR: &str = "FORGE_DESERIALIZE_ERROR";
    pub const SERIALIZE_ERROR: &str = "FORGE_SERIALIZE_ERROR";
    pub const METHOD_NOT_ALLOWED: &str = "FORGE_METHOD_NOT_ALLOWED";
    pub const HANDLER_PANIC: &str = "FORGE_HANDLER_PANIC";
    pub const UNSUPPORTED: &str = "FORGE_UNSUPPORTED";
    pub const DUPLICATE_WINDOW: &str = "FORGE_DUPLICATE_WINDOW";
    pub const WINDOW_NOT_FOUND: &str = "FORGE_WINDOW_NOT_FOUND";
}

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

/// Semantic HTTP status category owned by [`ForgeError`], decoupling wire status codes from string matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorKind {
    /// HTTP `400 Bad Request` — malformed input or invalid arguments.
    BadRequest,
    /// HTTP `403 Forbidden` — capability token, CSRF, sandbox, or permission rejection.
    Forbidden,
    /// HTTP `404 Not Found` — unknown command or missing resource.
    NotFound,
    /// HTTP `501 Not Implemented` — operation unsupported on the active runtime or OS.
    Unsupported,
    /// HTTP `422 Unprocessable Entity` — structured domain validation error.
    #[default]
    Unprocessable,
    /// HTTP `500 Internal Server Error` — unhandled failure or caught handler panic.
    Internal,
}

impl ErrorKind {
    pub(crate) fn unprocessable() -> Self {
        Self::Unprocessable
    }

    pub(crate) fn status(self) -> StatusCode {
        match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unsupported => StatusCode::NOT_IMPLEMENTED,
            Self::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub(crate) fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::BAD_REQUEST => Self::BadRequest,
            StatusCode::FORBIDDEN => Self::Forbidden,
            StatusCode::NOT_FOUND => Self::NotFound,
            StatusCode::NOT_IMPLEMENTED => Self::Unsupported,
            StatusCode::INTERNAL_SERVER_ERROR => Self::Internal,
            _ => Self::Unprocessable,
        }
    }
}

/// Structured error returned by async IPC commands and hydrated into a rejected JS `Promise`
/// carrying `{ code, message, details }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeError {
    pub code: Cow<'static, str>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip, default = "ErrorKind::unprocessable")]
    kind: ErrorKind,
}

impl PartialEq for ForgeError {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code && self.message == other.message && self.details == other.details
    }
}

impl Eq for ForgeError {}

impl ForgeError {
    /// Creates a domain error mapped to HTTP `422 Unprocessable Entity`.
    pub fn new(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::Unprocessable, code, message)
    }

    /// Creates a client input error mapped to HTTP `400 Bad Request`.
    pub fn bad_request(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::BadRequest, code, message)
    }

    /// Creates an authorization/confinement error mapped to HTTP `403 Forbidden`.
    pub fn forbidden(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::Forbidden, code, message)
    }

    /// Creates a missing-target error mapped to HTTP `404 Not Found`.
    pub fn not_found(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::NotFound, code, message)
    }

    /// Creates a runtime capability error (`FORGE_UNSUPPORTED`) mapped to HTTP `501 Not Implemented`.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::Unsupported, codes::UNSUPPORTED, message)
    }

    /// Creates an internal server error mapped to HTTP `500 Internal Server Error`.
    pub fn internal(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::Internal, code, message)
    }

    pub(crate) fn with_kind(
        kind: ErrorKind,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
            kind,
        }
    }

    pub(crate) fn set_kind(mut self, kind: ErrorKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn with_details<T: Serialize>(mut self, details: T) -> Self {
        self.details = serde_json::to_value(details).ok();
        self
    }

    pub(crate) fn status_code(&self) -> StatusCode {
        self.kind.status()
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
        Self::internal(codes::APP_ERROR, msg)
    }
}

impl From<String> for ForgeError {
    fn from(msg: String) -> Self {
        Self::internal(codes::APP_ERROR, msg)
    }
}

impl From<std::io::Error> for ForgeError {
    fn from(err: std::io::Error) -> Self {
        Self::new(codes::IO_ERROR, err.to_string())
    }
}

impl From<Infallible> for ForgeError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}
