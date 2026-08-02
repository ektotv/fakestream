//! The HTTP surface. One server exposes every fixture at once, so a player can
//! be pointed at a single host and switched between formats without restarting
//! anything.

mod index;

use crate::fixtures::{Fixture, catalogue};
use axum::Router;
use axum::response::Html;
use axum::routing::get;
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::services::ServeDir;

#[derive(Debug)]
pub enum ServeError {
    Bind { address: SocketAddr, source: std::io::Error },
    Io(std::io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind { address, source } => write!(formatter, "could not bind {address}: {source}"),
            Self::Io(source) => write!(formatter, "{source}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// Serve the catalogue out of `root` until interrupted.
///
/// File delivery goes through tower-http's `ServeDir`, which handles range and
/// conditional requests. Players lean on ranges to seek, so that behaviour is
/// worth taking rather than reimplementing.
pub async fn run(root: PathBuf, address: SocketAddr) -> Result<(), ServeError> {
    let fixtures = catalogue();
    let base = format!("http://{address}");

    let app = Router::new()
        .route("/", get(move || index_page(fixtures, base)))
        .fallback_service(ServeDir::new(root));

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|source| ServeError::Bind { address, source })?;

    println!("serving on http://{address}");

    axum::serve(listener, app).await.map_err(ServeError::Io)
}

async fn index_page(fixtures: Vec<Fixture>, base: String) -> Html<String> {
    Html(index::render(&fixtures, &base))
}
