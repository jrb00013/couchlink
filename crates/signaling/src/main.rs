mod api;
mod audit;
mod metrics;
mod session;
mod ws;

use anyhow::Context;
use api::{api_audit, api_status, health, metrics_handler, ApiState};
use audit::AuditLog;
use axum::{
    extract::ws::WebSocketUpgrade,
    routing::get,
    Router,
};
use clap::Parser;
use metrics::Metrics;
use session::SessionStore;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "couchlink-signaling", about = "Couchlink signaling server", version)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:8443", env = "COUCHLINK_BIND")]
    bind: SocketAddr,
    #[arg(long, default_value = "../../web/dist")]
    web_root: PathBuf,
    #[arg(long, env = "COUCHLINK_CERT")]
    cert: Option<PathBuf>,
    #[arg(long, env = "COUCHLINK_KEY")]
    key: Option<PathBuf>,
    #[arg(long, default_value = "3600", env = "COUCHLINK_SESSION_TTL_SECS")]
    session_ttl_secs: u64,
    #[arg(long, default_value = "5", env = "COUCHLINK_MAX_PIN_FAILURES")]
    max_pin_failures: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,couchlink_signaling=warn,tower_http=error".into()),
        )
        .init();

    let args = Args::parse();
    let audit = Arc::new(AuditLog::new());
    let metrics = Arc::new(Metrics::new());
    let store = Arc::new(SessionStore::with_limits(
        Arc::clone(&audit),
        Arc::clone(&metrics),
        args.max_pin_failures,
        args.session_ttl_secs,
    ));

    let api_state = ApiState {
        store: Arc::clone(&store),
        audit: Arc::clone(&audit),
        version: env!("CARGO_PKG_VERSION"),
    };

    let sweep = Arc::clone(&store);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            sweep.sweep_expired();
        }
    });

    let web_root = args
        .web_root
        .canonicalize()
        .unwrap_or(args.web_root.clone());
    let index = web_root.join("index.html");
    let serve_dir = ServeDir::new(&web_root).not_found_service(ServeFile::new(index));

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/status", get(api_status))
        .route("/api/audit", get(api_audit))
        .route("/metrics", get(metrics_handler))
        .route(
            "/ws",
            get({
                let store = Arc::clone(&store);
                move |ws: WebSocketUpgrade| {
                    let store = Arc::clone(&store);
                    async move { ws.on_upgrade(move |s| ws::handle_socket(s, store)) }
                }
            }),
        )
        .with_state(api_state)
        .fallback_service(serve_dir)
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http());

    info!("couchlink signaling on {}", args.bind);

    if let (Some(cert), Some(key)) = (args.cert, args.key) {
        let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
            .await
            .context("load TLS")?;
        axum_server::bind_rustls(args.bind, config)
            .serve(app.into_make_service())
            .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(args.bind).await?;
        axum::serve(listener, app).await?;
    }
    Ok(())
}
