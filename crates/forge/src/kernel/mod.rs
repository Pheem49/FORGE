mod assets;
mod bootstrap;
pub(crate) mod builtins;
#[cfg(feature = "dev-tools")]
mod devtools;
pub(crate) mod http;
mod ipc;
pub(crate) mod permissions;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "dev-tools")]
use ::http::StatusCode;
#[cfg(any(feature = "webview", feature = "cef"))]
use tokio::task::AbortHandle;

use self::permissions::PermissionPolicy;
use crate::config::{Config, unpack_tar_gz_assets};
use crate::error::{Error, Result};
use crate::handler::{ErasedCommand, StateMap};
#[cfg(feature = "dev-tools")]
use crate::runtime::AppHandle;
use crate::runtime::{ProtocolRequest, ProtocolResponse, WindowRegistry};

include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));

pub(crate) struct ForgeKernel {
    pub(crate) config: Arc<Config>,
    pub(crate) canonical_assets_root: Option<PathBuf>,
    pub(crate) states: Arc<StateMap>,
    pub(crate) commands: HashMap<&'static str, Arc<dyn ErasedCommand>>,
    pub(crate) permissions: PermissionPolicy,
    pub(crate) permission_warnings: Vec<String>,
    pub(crate) launch_token: String,
    pub(crate) smoke_test_enabled: bool,
    pub(crate) registry: Arc<WindowRegistry>,
    #[allow(dead_code)]
    pub(crate) tokio_handle: tokio::runtime::Handle,
    #[cfg(feature = "dev-tools")]
    pub(crate) dev_proxy: Option<(String, ureq::Agent)>,
    #[cfg(feature = "dev-tools")]
    _watcher_guard: Option<Box<dyn Send + Sync>>,
}

impl ForgeKernel {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        root_dir: &Path,
        mut config: Config,
        states: StateMap,
        commands: HashMap<&'static str, Arc<dyn ErasedCommand>>,
        permissions: PermissionPolicy,
        permission_warnings: Vec<String>,
        registry: Arc<WindowRegistry>,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<Self> {
        let dev_watch_enabled = std::env::var("FORGE_DEV_WATCH").ok().as_deref() == Some("1");
        let smoke_test_enabled = std::env::var("FORGE_SMOKE_TEST").ok().as_deref() == Some("1");
        if smoke_test_enabled {
            tokio_handle.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                eprintln!("[FORGE_SMOKE_TEST] timed out after 30s waiting for __forge_ready");
                std::process::exit(124);
            });
        }
        let active_dev_url = config
            .dev
            .url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .filter(|_| dev_watch_enabled || std::env::var("FORGE_DEV_URL").is_ok());

        #[cfg(feature = "dev-tools")]
        let dev_proxy = active_dev_url.as_ref().map(|url| {
            let trimmed = url.trim_end_matches('/').to_string();
            let agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .into();
            (trimmed, agent)
        });

        #[cfg(not(feature = "dev-tools"))]
        if active_dev_url.is_some() {
            return Err(Error::Config {
                path: None,
                message: "a [dev].url dev server is configured, but Cargo feature 'forge/dev-tools' is disabled".into(),
            });
        }

        if config.in_memory_assets.is_none()
            && active_dev_url.is_none()
            && let Some(archive_bytes) = EMBEDDED_UI_ARCHIVE
        {
            let embedded_map = unpack_tar_gz_assets(archive_bytes)?;
            config.in_memory_assets = Some(embedded_map);
        }

        let canonical_assets_root = if config.in_memory_assets.is_some() || active_dev_url.is_some()
        {
            None
        } else {
            let candidate = if config.app.assets.is_absolute() {
                config.app.assets.clone()
            } else {
                root_dir.join(&config.app.assets)
            };
            if !candidate.is_dir() {
                return Err(Error::AssetsNotFound { dir: candidate });
            }
            let canon = candidate
                .canonicalize()
                .map_err(|_| Error::AssetsNotFound {
                    dir: candidate.clone(),
                })?;
            Some(canon)
        };

        #[cfg(feature = "dev-tools")]
        let watcher_guard = if dev_watch_enabled
            && active_dev_url.is_none()
            && let Some(watch_root) = &canonical_assets_root
        {
            devtools::start_live_reload_watcher(
                watch_root,
                AppHandle::new(Arc::clone(&registry)),
                &tokio_handle,
            )
            .ok()
        } else {
            None
        };

        let launch_token = crate::util::random_hex_token::<32>()?;

        Ok(Self {
            config: Arc::new(config),
            canonical_assets_root,
            states: Arc::new(states),
            commands,
            permissions,
            permission_warnings,
            launch_token,
            smoke_test_enabled,
            registry,
            tokio_handle,
            #[cfg(feature = "dev-tools")]
            dev_proxy,
            #[cfg(feature = "dev-tools")]
            _watcher_guard: watcher_guard,
        })
    }

    #[cfg(any(feature = "webview", feature = "cef"))]
    pub(crate) fn dispatch_async<F>(
        self: &Arc<Self>,
        req: ProtocolRequest,
        respond: F,
    ) -> AbortHandle
    where
        F: FnOnce(ProtocolResponse) + Send + 'static,
    {
        let kernel = Arc::clone(self);
        let join = self.tokio_handle.spawn(async move {
            let res = kernel.handle_request(req).await;
            respond(res);
        });
        join.abort_handle()
    }

    pub(crate) async fn handle_request(&self, req: ProtocolRequest) -> ProtocolResponse {
        let raw_path = req.uri.path();

        if raw_path == "/__forge/bootstrap.js" {
            return self.serve_bootstrap(&req.method, &req.window);
        }

        if let Some(encoded_cmd) = raw_path.strip_prefix("/__forge_ipc/") {
            let encoded_cmd = encoded_cmd.to_owned();
            return self.handle_ipc(&encoded_cmd, req).await;
        }

        #[cfg(feature = "dev-tools")]
        if let Some((base_url, agent)) = &self.dev_proxy {
            let path_and_query = req
                .uri
                .path_and_query()
                .map(|pq| pq.as_str().to_string())
                .unwrap_or_else(|| "/".to_string());
            let base = base_url.clone();
            let agent = agent.clone();
            let method = req.method.clone();
            return match tokio::task::spawn_blocking(move || {
                devtools::proxy_dev_request(&agent, &base, &method, &path_and_query)
            })
            .await
            {
                Ok(res) => res,
                Err(e) => http::plain_response(
                    StatusCode::BAD_GATEWAY,
                    format!("Dev server proxy task failed: {e}").into_bytes(),
                    http::DEV_PROXY_CSP,
                ),
            };
        }

        self.serve_static_asset(&req.method, raw_path)
    }

    pub(super) fn active_csp(&self) -> &'static str {
        #[cfg(feature = "dev-tools")]
        if self.dev_proxy.is_some() {
            return http::DEV_PROXY_CSP;
        }
        http::STRICT_CSP
    }
}
