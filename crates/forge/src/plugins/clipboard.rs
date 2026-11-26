use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::{App, ForgeError, Plugin, State};

#[derive(Debug, Deserialize, Serialize)]
pub struct WriteClipboardReq {
    pub text: String,
}

#[derive(Clone)]
enum ClipboardBackend {
    Native,
    Memory(Arc<Mutex<String>>),
}

#[derive(Clone)]
struct ClipboardState(ClipboardBackend);

/// Cross-platform OS clipboard plugin (`clipboard.read_text`, `clipboard.write_text`).
#[derive(Clone)]
pub struct ClipboardPlugin {
    backend: ClipboardBackend,
}

impl Default for ClipboardPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardPlugin {
    pub fn new() -> Self {
        Self {
            backend: ClipboardBackend::Native,
        }
    }

    /// Creates an isolated in-memory clipboard plugin for headless tests.
    pub fn in_memory() -> Self {
        Self {
            backend: ClipboardBackend::Memory(Arc::new(Mutex::new(String::new()))),
        }
    }
}

impl Plugin for ClipboardPlugin {
    fn apply(self, app: App) -> App {
        let backend = self.backend;
        app.setup(move |ctx| {
            ctx.insert_state(ClipboardState(backend));
            Ok(())
        })
        .command("clipboard.read_text", read_text)
        .command("clipboard.write_text", write_text)
    }
}

async fn read_text(state: State<ClipboardState>) -> Result<String, ForgeError> {
    match &state.0 {
        ClipboardBackend::Memory(buf) => Ok(buf.lock().map(|g| g.clone()).unwrap_or_default()),
        ClipboardBackend::Native => tokio::task::spawn_blocking(|| {
            let mut cb = arboard::Clipboard::new()
                .map_err(|e| ForgeError::new("CLIPBOARD_UNAVAILABLE", e.to_string()))?;
            cb.get_text()
                .map_err(|e| ForgeError::new("CLIPBOARD_READ_FAILED", e.to_string()))
        })
        .await
        .map_err(|e| ForgeError::new("CLIPBOARD_TASK_FAILED", e.to_string()))?,
    }
}

async fn write_text(
    state: State<ClipboardState>,
    req: WriteClipboardReq,
) -> Result<(), ForgeError> {
    match &state.0 {
        ClipboardBackend::Memory(buf) => {
            if let Ok(mut guard) = buf.lock() {
                *guard = req.text;
            }
            Ok(())
        }
        ClipboardBackend::Native => tokio::task::spawn_blocking(move || {
            let mut cb = arboard::Clipboard::new()
                .map_err(|e| ForgeError::new("CLIPBOARD_UNAVAILABLE", e.to_string()))?;
            cb.set_text(req.text)
                .map_err(|e| ForgeError::new("CLIPBOARD_WRITE_FAILED", e.to_string()))
        })
        .await
        .map_err(|e| ForgeError::new("CLIPBOARD_TASK_FAILED", e.to_string()))?,
    }
}
