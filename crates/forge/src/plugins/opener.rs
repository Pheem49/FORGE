use std::process::Command;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{ForgeError, codes};
use crate::{App, Plugin, State};

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenUrlReq {
    pub url: String,
}

/// Seam for delegating validated external URLs to the host OS browser / mail handler.
pub trait UrlOpener: Send + Sync + 'static {
    fn open_url(&self, url: &str) -> Result<(), ForgeError>;
}

/// Default OS URL opener (`xdg-open` on Linux, `open` on macOS, `rundll32 url.dll,FileProtocolHandler` on Windows).
#[derive(Default)]
pub struct OsUrlOpener;

impl UrlOpener for OsUrlOpener {
    fn open_url(&self, url: &str) -> Result<(), ForgeError> {
        #[cfg(target_os = "macos")]
        let res = Command::new("open").arg("--").arg(url).spawn();

        #[cfg(target_os = "windows")]
        let res = Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .spawn();

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let res = Command::new("xdg-open").arg(url).spawn();

        res.map(|_| ()).map_err(|e| {
            ForgeError::internal(
                "OPENER_FAILED",
                format!("failed to launch system URL handler: {e}"),
            )
        })
    }
}

/// Test adapter recording opened URLs in memory without spawning external OS browsers.
#[derive(Clone, Default)]
pub struct RecordedUrlOpener {
    opened: Arc<Mutex<Vec<String>>>,
}

impl RecordedUrlOpener {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take_opened(&self) -> Vec<String> {
        self.opened
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }
}

impl UrlOpener for RecordedUrlOpener {
    fn open_url(&self, url: &str) -> Result<(), ForgeError> {
        if let Ok(mut guard) = self.opened.lock() {
            guard.push(url.to_string());
        }
        Ok(())
    }
}

struct OpenerState {
    allowed_schemes: Vec<String>,
    opener: Arc<dyn UrlOpener>,
}

/// Safe external URL launcher plugin (`opener.open_url`), enforcing a strict scheme allow-list
/// (`["https", "http", "mailto"]` by default) and blocking `javascript:`, `file:`, `data:`, and CLI flag injection.
pub struct OpenerPlugin {
    allowed_schemes: Vec<String>,
    opener: Arc<dyn UrlOpener>,
}

impl Default for OpenerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenerPlugin {
    pub fn new() -> Self {
        Self {
            allowed_schemes: vec![
                "https".to_string(),
                "http".to_string(),
                "mailto".to_string(),
            ],
            opener: Arc::new(OsUrlOpener),
        }
    }

    /// Replaces the default `["https", "http", "mailto"]` scheme allow-list.
    pub fn with_allowed_schemes<I, S>(mut self, schemes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_schemes = schemes
            .into_iter()
            .map(|s| s.into().trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }

    /// Injects a custom [`UrlOpener`] adapter (such as [`RecordedUrlOpener`] in headless tests).
    pub fn with_opener(mut self, opener: impl UrlOpener) -> Self {
        self.opener = Arc::new(opener);
        self
    }
}

impl Plugin for OpenerPlugin {
    fn apply(self, app: App) -> App {
        app.state(OpenerState {
            allowed_schemes: self.allowed_schemes,
            opener: self.opener,
        })
        .command("opener.open_url", opener_open_url)
    }
}

async fn opener_open_url(state: State<OpenerState>, req: OpenUrlReq) -> Result<(), ForgeError> {
    let url = validate_openable_url(&req.url, &state.allowed_schemes)?;
    tokio::task::spawn_blocking(move || state.opener.open_url(&url))
        .await
        .map_err(|e| ForgeError::internal("OPENER_TASK_FAILED", e.to_string()))?
}

fn validate_openable_url(raw_url: &str, allowed_schemes: &[String]) -> Result<String, ForgeError> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            "URL cannot be empty",
        ));
    }
    if trimmed.starts_with('-') {
        return Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            "URL cannot start with '-'",
        ));
    }
    let Some((scheme_raw, rest)) = trimmed.split_once(':') else {
        return Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            "URL must include an explicit URI scheme (e.g. https://...)",
        ));
    };
    let scheme = scheme_raw.to_ascii_lowercase();
    if matches!(
        scheme.as_str(),
        "javascript" | "vbscript" | "data" | "file" | "blob"
    ) || !allowed_schemes.iter().any(|s| s == &scheme)
    {
        return Err(ForgeError::forbidden(
            codes::FORBIDDEN,
            format!("URI scheme '{scheme}:' is not allowed by OpenerPlugin"),
        ));
    }
    if trimmed.chars().any(|c| {
        c.is_control() || c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '<' | '>' | '`')
    }) {
        return Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            "URL contains disallowed control, whitespace, or quote characters",
        ));
    }
    if matches!(scheme.as_str(), "http" | "https") && !rest.starts_with("//") {
        return Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            format!("'{scheme}:' URL must start with '{scheme}://'"),
        ));
    }
    if rest.trim_start_matches('/').is_empty() {
        return Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            "URL target authority/address cannot be empty",
        ));
    }
    Ok(trimmed.to_string())
}
