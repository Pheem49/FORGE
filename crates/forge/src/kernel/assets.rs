use std::path::{Component, Path};

use http::{Method, StatusCode};

use super::ForgeKernel;
use super::http::{build_response, plain_response};
use crate::runtime::ProtocolResponse;

const BOOTSTRAP_TAG: &str = r#"<script src="/__forge/bootstrap.js"></script>"#;

impl ForgeKernel {
    pub(super) fn serve_static_asset(&self, method: &Method, raw_path: &str) -> ProtocolResponse {
        let csp = self.active_csp();
        if *method != Method::GET && *method != Method::HEAD {
            return plain_response(
                StatusCode::METHOD_NOT_ALLOWED,
                b"Method Not Allowed".to_vec(),
                csp,
            );
        }

        let Some(clean_rel) = sanitize_asset_path(raw_path) else {
            return plain_response(StatusCode::FORBIDDEN, b"Forbidden".to_vec(), csp);
        };

        let raw_bytes = if let Some(mem_map) = &self.config.in_memory_assets {
            let key = format!("/{clean_rel}");
            match mem_map.get(&key) {
                Some(bytes) => bytes.clone(),
                None => return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp),
            }
        } else if let Some(canon_root) = &self.canonical_assets_root {
            let candidate = canon_root.join(&clean_rel);
            let Ok(canon_target) = candidate.canonicalize() else {
                return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp);
            };
            if !canon_target.starts_with(canon_root) {
                return plain_response(StatusCode::FORBIDDEN, b"Forbidden".to_vec(), csp);
            }
            if !canon_target.is_file() {
                return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp);
            }
            match std::fs::read(&canon_target) {
                Ok(b) => b,
                Err(_) => return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp),
            }
        } else {
            return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp);
        };

        let mime = detect_mime(&clean_rel);
        let final_bytes = if mime.starts_with("text/html") {
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

        build_response(StatusCode::OK, mime, body, content_len, false, csp)
    }
}

pub(super) fn sanitize_asset_path(raw_path: &str) -> Option<String> {
    let decoded = percent_decode(raw_path);
    if decoded.contains('\0') || decoded.contains('\\') {
        return None;
    }
    let trimmed = decoded.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some("index.html".to_string());
    }

    let path = Path::new(trimmed);
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_str()?;
                parts.push(s);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    if parts.is_empty() {
        Some("index.html".to_string())
    } else {
        Some(parts.join("/"))
    }
}

pub(super) fn inject_bootstrap_script(html_bytes: &[u8]) -> Vec<u8> {
    let Ok(html) = std::str::from_utf8(html_bytes) else {
        return html_bytes.to_vec();
    };
    if html.contains("/__forge/bootstrap.js") {
        return html_bytes.to_vec();
    }

    let lower = html.to_ascii_lowercase();
    if let Some(head_idx) = lower.find("<head")
        && let Some(rel_close) = lower[head_idx..].find('>')
    {
        let insert_pos = head_idx + rel_close + 1;
        let mut out = String::with_capacity(html.len() + BOOTSTRAP_TAG.len() + 1);
        out.push_str(&html[..insert_pos]);
        out.push_str(BOOTSTRAP_TAG);
        out.push_str(&html[insert_pos..]);
        return out.into_bytes();
    }

    let mut out = String::with_capacity(html.len() + BOOTSTRAP_TAG.len());
    out.push_str(BOOTSTRAP_TAG);
    out.push_str(html);
    out.into_bytes()
}

pub(super) fn detect_mime(rel_path: &str) -> &'static str {
    match Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub(crate) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
