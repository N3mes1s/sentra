use std::env;

use sentra::{app, build_state_from_env};
use tokio::net::TcpListener;
use tokio::signal;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialise structured logging. Reads RUST_LOG environment variable.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // NOTE: Minimal logging setup; JSON output not implemented due to limited fmt feature set.
    // Future enhancement: switch to tracing-layered JSON serialization crate.
    fmt().with_env_filter(filter).init();

    // Build application state from environment variables and optional config
    let state = build_state_from_env().await?;
    let app = app(state);

    // Determine port to bind on. Default to 8080 if unspecified.
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);
    let addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();

    // Run the server with graceful shutdown on Ctrl+C
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    // Wait for Ctrl+C
    let _ = signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
