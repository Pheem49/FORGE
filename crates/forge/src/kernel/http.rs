use http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderValue,
    X_CONTENT_TYPE_OPTIONS,
};
use http::{HeaderMap, StatusCode};
use serde_json::Value;

use crate::error::ForgeError;
use crate::runtime::ProtocolResponse;

pub(crate) const STRICT_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'";
#[cfg(feature = "dev-tools")]
pub(crate) const DEV_PROXY_CSP: &str = "default-src 'self' http://localhost:* http://127.0.0.1:*; script-src 'self' 'unsafe-inline' 'unsafe-eval' http://localhost:* http://127.0.0.1:*; style-src 'self' 'unsafe-inline' http://localhost:* http://127.0.0.1:*; img-src 'self' data: blob: http://localhost:* http://127.0.0.1:*; font-src 'self' data: http://localhost:* http://127.0.0.1:*; connect-src 'self' http://localhost:* http://127.0.0.1:* ws://localhost:* ws://127.0.0.1:*";

pub(crate) fn security_headers(csp: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(csp));
    headers
}

pub(crate) fn build_response(
    status: StatusCode,
    content_type: &'static str,
    body: Vec<u8>,
    content_len: usize,
    no_store: bool,
    csp: &'static str,
) -> ProtocolResponse {
    let mut headers = security_headers(csp);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    if no_store {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers.insert(CONTENT_LENGTH, HeaderValue::from(content_len));
    ProtocolResponse {
        status,
        headers,
        body,
    }
}

pub(crate) fn plain_response(
    status: StatusCode,
    body: Vec<u8>,
    csp: &'static str,
) -> ProtocolResponse {
    let len = body.len();
    build_response(status, "text/plain; charset=utf-8", body, len, false, csp)
}

pub(crate) fn binary_ok_response(body: Vec<u8>, csp: &'static str) -> ProtocolResponse {
    let len = body.len();
    build_response(
        StatusCode::OK,
        "application/octet-stream",
        body,
        len,
        true,
        csp,
    )
}

pub(crate) fn json_ok_response(value: &Value, csp: &'static str) -> ProtocolResponse {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"null".to_vec());
    let len = body.len();
    build_response(
        StatusCode::OK,
        "application/json; charset=utf-8",
        body,
        len,
        true,
        csp,
    )
}

pub(crate) fn json_error_response(
    status: StatusCode,
    err: &ForgeError,
    csp: &'static str,
) -> ProtocolResponse {
    let body = serde_json::to_vec(err)
        .unwrap_or_else(|_| br#"{"code":"APP_ERROR","message":"serialization failed"}"#.to_vec());
    let len = body.len();
    build_response(
        status,
        "application/json; charset=utf-8",
        body,
        len,
        true,
        csp,
    )
}
