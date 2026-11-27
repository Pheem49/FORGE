use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::{App, Plugin, State};

struct DeepLinkState {
    current: Arc<Mutex<Vec<String>>>,
}

/// Custom URI scheme deep-link plugin (`deep_link.get_current`),
/// extracting `<scheme>://...` URLs matching `[app].schemes` on startup, macOS `Event::Opened`,
/// and `SingleInstanceGuard` secondary launches (`deep-link:open`).
#[derive(Clone, Default)]
pub struct DeepLinkPlugin {
    explicit_schemes: Option<Vec<String>>,
    initial_args_override: Option<Vec<String>>,
}

impl DeepLinkPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    /// Overrides the URI schemes matched by this plugin (defaults to `[app].schemes` in `forge.toml`).
    pub fn with_schemes<I, S>(mut self, schemes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.explicit_schemes = Some(
            schemes
                .into_iter()
                .map(|s| s.into().trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
        );
        self
    }

    /// Overrides the initial process CLI arguments inspected on startup (ideal for deterministic tests).
    pub fn with_initial_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.initial_args_override = Some(args.into_iter().map(Into::into).collect());
        self
    }
}

impl Plugin for DeepLinkPlugin {
    fn apply(self, app: App) -> App {
        let explicit_schemes = self.explicit_schemes;
        let args_override = self.initial_args_override;

        app.setup(move |ctx| {
            let schemes: Vec<String> = explicit_schemes
                .unwrap_or_else(|| ctx.config().app.schemes.clone())
                .into_iter()
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect();

            let raw_startup_args: Vec<String> = match args_override {
                Some(custom) => custom,
                None => std::env::args().skip(1).collect(),
            };

            let app_handle = ctx.app_handle();
            let startup_urls = extract_matching_deep_links(&raw_startup_args, &schemes);
            if !startup_urls.is_empty() {
                let _ = app_handle.emit_all("deep-link:open", json!({ "urls": &startup_urls }));
            }

            let current = Arc::new(Mutex::new(startup_urls));

            // 1. Secondary launch handoff (`SingleInstanceGuard` on Linux & Windows)
            {
                let current_for_si = Arc::clone(&current);
                let handle_for_si = app_handle.clone();
                let schemes_for_si = schemes.clone();
                app_handle
                    .registry
                    .register_second_instance_hook(Arc::new(move |payload| {
                        let matched = extract_matching_deep_links(&payload.args, &schemes_for_si);
                        if !matched.is_empty() {
                            if let Ok(mut guard) = current_for_si.lock() {
                                guard.clone_from(&matched);
                            }
                            let _ = handle_for_si
                                .emit_all("deep-link:open", json!({ "urls": matched }));
                        }
                    }));
            }

            // 2. Native OS URL open event (`tao::event::Event::Opened` on macOS)
            {
                let current_for_open = Arc::clone(&current);
                let handle_for_open = app_handle.clone();
                let schemes_for_open = schemes;
                app_handle
                    .registry
                    .register_urls_opened_hook(Arc::new(move |raw_urls| {
                        let matched = extract_matching_deep_links(raw_urls, &schemes_for_open);
                        if !matched.is_empty() {
                            if let Ok(mut guard) = current_for_open.lock() {
                                guard.clone_from(&matched);
                            }
                            if let Some(main_win) = handle_for_open.window("main") {
                                main_win.show();
                                main_win.focus();
                            }
                            let _ = handle_for_open
                                .emit_all("deep-link:open", json!({ "urls": matched }));
                        }
                    }));
            }

            ctx.insert_state(DeepLinkState { current });
            Ok(())
        })
        .command("deep_link.get_current", deep_link_get_current)
    }
}

async fn deep_link_get_current(state: State<DeepLinkState>) -> Result<Vec<String>, Infallible> {
    let urls = state.current.lock().map(|g| g.clone()).unwrap_or_default();
    Ok(urls)
}

/// Filters CLI arguments or OS open events down to valid `<scheme>://...` URLs matching `allowed_schemes`.
pub fn extract_matching_deep_links(args: &[String], allowed_schemes: &[String]) -> Vec<String> {
    if allowed_schemes.is_empty() {
        return Vec::new();
    }
    let mut matched = Vec::new();
    for arg in args {
        let trimmed = arg.trim();
        if trimmed.chars().any(|c| c.is_control()) {
            continue;
        }
        if let Some((scheme_part, rest)) = trimmed.split_once("://") {
            let lower = scheme_part.to_ascii_lowercase();
            if !rest.is_empty() && allowed_schemes.iter().any(|s| s == &lower) {
                matched.push(trimmed.to_string());
            }
        }
    }
    matched
}
