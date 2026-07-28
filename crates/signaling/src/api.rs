use crate::session::SessionStore;
use axum::{extract::State, Json};
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct ApiState {
    pub store: Arc<SessionStore>,
    pub audit: Arc<crate::audit::AuditLog>,
    pub version: &'static str,
}

#[derive(Serialize)]
pub struct Status {
    pub status: &'static str,
    pub version: &'static str,
    pub ws_connections: usize,
    pub sessions_active: usize,
}

pub async fn health() -> &'static str {
    "ok"
}

pub async fn api_status(State(st): State<ApiState>) -> Json<Status> {
    Json(Status {
        status: "ok",
        version: st.version,
        ws_connections: st.store.connection_count(),
        sessions_active: st.store.session_count(),
    })
}

pub async fn api_audit(State(st): State<ApiState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(st.audit.snapshot()).unwrap_or_default())
}

pub async fn metrics_handler(State(st): State<ApiState>) -> String {
    st.store.metrics().prometheus()
}
