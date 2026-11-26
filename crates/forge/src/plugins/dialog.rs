use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rfd::{AsyncFileDialog, AsyncMessageDialog, MessageButtons, MessageDialogResult};
use serde::{Deserialize, Serialize};

use crate::{App, ForgeError, Plugin, State};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FileFilterSpec {
    pub name: String,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenDialogReq {
    pub title: Option<String>,
    pub directory: bool,
    pub filters: Vec<FileFilterSpec>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SaveDialogReq {
    pub title: Option<String>,
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfirmDialogReq {
    pub title: String,
    pub description: String,
}

#[derive(Clone)]
enum DialogBackend {
    Native,
    Mock {
        next_open: Arc<Mutex<Option<String>>>,
        next_save: Arc<Mutex<Option<String>>>,
        next_confirm: Arc<Mutex<bool>>,
    },
}

#[derive(Clone)]
struct DialogState(DialogBackend);

/// Native OS file & confirmation dialog plugin (`dialog.open`, `dialog.save`, `dialog.confirm`).
#[derive(Clone)]
pub struct DialogPlugin {
    backend: DialogBackend,
}

impl Default for DialogPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl DialogPlugin {
    pub fn new() -> Self {
        Self {
            backend: DialogBackend::Native,
        }
    }

    /// Creates a deterministic mock dialog plugin for headless `cargo test` suites.
    pub fn mock(
        open_path: Option<PathBuf>,
        save_path: Option<PathBuf>,
        confirm_answer: bool,
    ) -> Self {
        Self {
            backend: DialogBackend::Mock {
                next_open: Arc::new(Mutex::new(
                    open_path.map(|p| p.to_string_lossy().into_owned()),
                )),
                next_save: Arc::new(Mutex::new(
                    save_path.map(|p| p.to_string_lossy().into_owned()),
                )),
                next_confirm: Arc::new(Mutex::new(confirm_answer)),
            },
        }
    }
}

impl Plugin for DialogPlugin {
    fn apply(self, app: App) -> App {
        let backend = self.backend;
        app.setup(move |ctx| {
            ctx.insert_state(DialogState(backend));
            Ok(())
        })
        .command("dialog.open", dialog_open)
        .command("dialog.save", dialog_save)
        .command("dialog.confirm", dialog_confirm)
    }
}

async fn dialog_open(
    state: State<DialogState>,
    req: OpenDialogReq,
) -> Result<Option<String>, ForgeError> {
    match &state.0 {
        DialogBackend::Mock { next_open, .. } => Ok(next_open.lock().ok().and_then(|g| g.clone())),
        DialogBackend::Native => {
            let mut dlg = AsyncFileDialog::new();
            if let Some(title) = &req.title {
                dlg = dlg.set_title(title);
            }
            for filter in &req.filters {
                let refs: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
                dlg = dlg.add_filter(&filter.name, &refs);
            }
            let picked = if req.directory {
                dlg.pick_folder().await
            } else {
                dlg.pick_file().await
            };
            Ok(picked.map(|h| h.path().to_string_lossy().into_owned()))
        }
    }
}

async fn dialog_save(
    state: State<DialogState>,
    req: SaveDialogReq,
) -> Result<Option<String>, ForgeError> {
    match &state.0 {
        DialogBackend::Mock { next_save, .. } => Ok(next_save.lock().ok().and_then(|g| g.clone())),
        DialogBackend::Native => {
            let mut dlg = AsyncFileDialog::new();
            if let Some(title) = &req.title {
                dlg = dlg.set_title(title);
            }
            if let Some(file_name) = &req.file_name {
                dlg = dlg.set_file_name(file_name);
            }
            let picked = dlg.save_file().await;
            Ok(picked.map(|h| h.path().to_string_lossy().into_owned()))
        }
    }
}

async fn dialog_confirm(
    state: State<DialogState>,
    req: ConfirmDialogReq,
) -> Result<bool, ForgeError> {
    match &state.0 {
        DialogBackend::Mock { next_confirm, .. } => {
            Ok(next_confirm.lock().map(|g| *g).unwrap_or(false))
        }
        DialogBackend::Native => {
            let res = AsyncMessageDialog::new()
                .set_title(&req.title)
                .set_description(&req.description)
                .set_buttons(MessageButtons::OkCancel)
                .show()
                .await;
            Ok(matches!(
                res,
                MessageDialogResult::Ok | MessageDialogResult::Yes
            ))
        }
    }
}
