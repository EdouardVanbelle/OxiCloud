use axum::extract::State;
use axum::http::header;
use axum::response::Response;
use serde_json::json;
use std::sync::Arc;

use crate::common::di::AppState;

/// Pre-serialized `/status.php` body. The payload is process-invariant
/// (pure config: emulated NC version), yet every NC desktop/mobile client
/// polls it on connect and periodically — the old handler re-built the
/// `json!` tree and re-serialized on every poll (793 ns / 14 allocs;
/// now a `Bytes` refcount bump at ~29 ns / 0 allocs — benches/ROUND11.md).
static STATUS_BODY: std::sync::OnceLock<bytes::Bytes> = std::sync::OnceLock::new();

pub async fn handle_status(State(state): State<Arc<AppState>>) -> Response {
    let body = STATUS_BODY.get_or_init(|| {
        let (major, minor, patch) = state.core.config.nextcloud.emulated_version;
        let version_string = state.core.config.nextcloud.version_string();
        let v = json!({
            "installed": true,
            "maintenance": false,
            "needsDbUpgrade": false,
            "version": format!("{}.{}.{}.1", major, minor, patch),
            "versionstring": version_string,
            "productname": "OxiCloud",
            "edition": ""
        });
        bytes::Bytes::from(serde_json::to_vec(&v).expect("status.php body serializes"))
    });
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.clone()))
        .expect("static status.php response")
}
