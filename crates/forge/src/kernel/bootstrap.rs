use http::{Method, StatusCode};

use super::ForgeKernel;
use super::http::{build_response, plain_response};
use crate::runtime::ProtocolResponse;

const BOOTSTRAP_TEMPLATE: &str = include_str!("bootstrap.js");

impl ForgeKernel {
    pub(super) fn serve_bootstrap(&self, method: &Method) -> ProtocolResponse {
        if *method != Method::GET && *method != Method::HEAD {
            return plain_response(
                StatusCode::METHOD_NOT_ALLOWED,
                b"Method Not Allowed".to_vec(),
                self.active_csp(),
            );
        }

        let token_js = serde_json::to_string(&self.launch_token).unwrap_or_else(|_| "\"\"".into());
        let runtime_js = serde_json::to_string(self.config.runtime.default.as_str())
            .unwrap_or_else(|_| "\"webview\"".into());
        let smoke_js = if std::env::var("FORGE_SMOKE_TEST").ok().as_deref() == Some("1") {
            "true"
        } else {
            "false"
        };

        let script = BOOTSTRAP_TEMPLATE
            .replace("__FORGE_TOKEN__", &token_js)
            .replace("__FORGE_RUNTIME__", &runtime_js)
            .replace("__FORGE_SMOKE__", smoke_js);

        let content_len = script.len();
        let body = if *method == Method::HEAD {
            Vec::new()
        } else {
            script.into_bytes()
        };

        build_response(
            StatusCode::OK,
            "text/javascript; charset=utf-8",
            body,
            content_len,
            true,
            self.active_csp(),
        )
    }
}
