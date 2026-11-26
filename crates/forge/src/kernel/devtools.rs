use std::path::{Path, PathBuf};

use http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use http::{Method, StatusCode};
use serde_json::json;

use super::assets::inject_bootstrap_script;
use super::http::{DEV_PROXY_CSP, plain_response, security_headers};
use crate::runtime::{AppHandle, ProtocolResponse};

pub(super) fn start_live_reload_watcher(
    watch_root: &Path,
    app_handle: AppHandle,
    tokio_handle: &tokio::runtime::Handle,
) -> std::result::Result<Box<dyn Send + Sync>, notify::Error> {
    use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                        if !event.paths.is_empty() =>
                    {
                        let _ = tx.send(event.paths);
                    }
                    _ => {}
                }
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(watch_root, RecursiveMode::Recursive)?;

    tokio_handle.spawn(async move {
        while let Some(mut changed_paths) = rx.recv().await {
            // Debounce bursty save events across 65ms.
            tokio::time::sleep(std::time::Duration::from_millis(65)).await;
            while let Ok(more) = rx.try_recv() {
                changed_paths.extend(more);
            }
            let meaningful: Vec<&Path> = changed_paths
                .iter()
                .map(PathBuf::as_path)
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| !n.starts_with('.') && !n.ends_with('~'))
                })
                .collect();
            if meaningful.is_empty() {
                continue;
            }
            let css_only = meaningful.iter().all(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("css"))
            });
            let topic = if css_only {
                "__forge:css-reload"
            } else {
                "__forge:reload"
            };
            let _ = app_handle.emit_all(topic, json!({ "count": meaningful.len() }));
        }
    });

    Ok(Box::new(watcher))
}

pub(super) fn proxy_dev_request(
    agent: &ureq::Agent,
    base_url: &str,
    method: &Method,
    path_and_query: &str,
) -> ProtocolResponse {
    if *method != Method::GET && *method != Method::HEAD {
        return plain_response(
            StatusCode::METHOD_NOT_ALLOWED,
            b"Method Not Allowed".to_vec(),
            DEV_PROXY_CSP,
        );
    }

    let target_url = format!("{base_url}{path_and_query}");
    let upstream = match agent.get(&target_url).call() {
        Ok(res) => res,
        Err(err) => {
            return plain_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to reach frontend dev server at {target_url}: {err}").into_bytes(),
                DEV_PROXY_CSP,
            );
        }
    };

    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let raw_bytes = match upstream.into_body().read_to_vec() {
        Ok(b) => b,
        Err(err) => {
            return plain_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read upstream dev server body: {err}").into_bytes(),
                DEV_PROXY_CSP,
            );
        }
    };

    let final_bytes = if content_type.starts_with("text/html") {
        inject_bootstrap_script(&raw_bytes)
    } else {
        raw_bytes
    };

    let content_len = final_bytes.len();
    let body = if *method == Method::HEAD {
        Vec::new()
    } else {
        final_bytes
    };

    let mut headers = security_headers(DEV_PROXY_CSP);
    if let Ok(hv) = HeaderValue::from_str(&content_type) {
        headers.insert(CONTENT_TYPE, hv);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(content_len));

    ProtocolResponse {
        status,
        headers,
        body,
    }
}
