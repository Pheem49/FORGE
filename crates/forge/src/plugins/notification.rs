use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::{App, AppConfig, ForgeError, Plugin, State};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SendNotificationReq {
    pub title: String,
    pub body: String,
}

#[derive(Clone)]
enum NotificationBackend {
    Native,
    Recording(Arc<Mutex<Vec<SendNotificationReq>>>),
}

#[derive(Clone)]
struct NotificationState(NotificationBackend);

/// Native OS desktop notification plugin (`notification.send`).
#[derive(Clone)]
pub struct NotificationPlugin {
    backend: NotificationBackend,
}

impl Default for NotificationPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationPlugin {
    pub fn new() -> Self {
        Self {
            backend: NotificationBackend::Native,
        }
    }

    /// Creates a recording notification plugin for headless tests, returning the plugin
    /// and a handle to inspect delivered notifications.
    pub fn recording() -> (Self, Arc<Mutex<Vec<SendNotificationReq>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                backend: NotificationBackend::Recording(Arc::clone(&log)),
            },
            log,
        )
    }
}

impl Plugin for NotificationPlugin {
    fn apply(self, app: App) -> App {
        let backend = self.backend;
        app.setup(move |ctx| {
            ctx.insert_state(NotificationState(backend));
            Ok(())
        })
        .command("notification.send", send_notification)
    }
}

async fn send_notification(
    state: State<NotificationState>,
    cfg: AppConfig,
    req: SendNotificationReq,
) -> Result<(), ForgeError> {
    if req.title.trim().is_empty() {
        return Err(ForgeError::bad_request(
            "NOTIFICATION_EMPTY_TITLE",
            "Notification title must not be empty",
        ));
    }

    match &state.0 {
        NotificationBackend::Recording(log) => {
            if let Ok(mut guard) = log.lock() {
                guard.push(req);
            }
            Ok(())
        }
        NotificationBackend::Native => {
            let app_name = cfg.app.name.clone();
            tokio::task::spawn_blocking(move || {
                notify_rust::Notification::new()
                    .appname(&app_name)
                    .summary(&req.title)
                    .body(&req.body)
                    .show()
                    .map(|_| ())
                    .map_err(|e| ForgeError::new("NOTIFICATION_FAILED", e.to_string()))
            })
            .await
            .map_err(|e| ForgeError::new("NOTIFICATION_TASK_FAILED", e.to_string()))?
        }
    }
}
