//! The HTTP surface. One server exposes every fixture at once, so a player can
//! be pointed at a single host and switched between formats without restarting
//! anything.

mod index;

use crate::fixtures::{Fixture, catalogue};
use axum::Router;
use axum::response::Html;
use axum::routing::get;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::services::ServeDir;

#[derive(Debug)]
pub enum ServeError {
    Bind {
        address: SocketAddr,
        source: std::io::Error,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind { address, source } => {
                write!(formatter, "could not bind {address}: {source}")
            }
            Self::Io(source) => write!(formatter, "{source}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// How far along a fixture is, as the index page reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Readiness {
    Waiting,
    /// Zero to one.
    Building(f64),
    Ready,
}

/// Shared between the thread generating fixtures and the request handlers.
pub type Progress = Arc<Mutex<HashMap<String, Readiness>>>;

/// Start with everything waiting, so the index is honest from the first request
/// rather than claiming fixtures exist before they do.
pub fn pending(fixtures: &[Fixture]) -> Progress {
    let map = fixtures
        .iter()
        .map(|fixture| (fixture.id.to_string(), Readiness::Waiting))
        .collect();
    Arc::new(Mutex::new(map))
}

/// Serve the catalogue out of `root` until interrupted.
///
/// File delivery goes through tower-http's `ServeDir`, which handles range and
/// conditional requests. Players lean on ranges to seek, so that behaviour is
/// worth taking rather than reimplementing.
///
/// The server starts before generation finishes, since waiting two minutes to
/// reach a page that could have told you what was happening is the worst of
/// both. The index reports what is ready.
pub async fn run(
    listener: tokio::net::TcpListener,
    address: SocketAddr,
    root: PathBuf,
    progress: Progress,
) -> Result<(), ServeError> {
    let fixtures = catalogue();
    let base = format!("http://{address}");

    let app = Router::new()
        .route(
            "/",
            get(move || index_page(fixtures, base, Arc::clone(&progress))),
        )
        .fallback_service(ServeDir::new(root));

    axum::serve(listener, app).await.map_err(ServeError::Io)
}

/// Take the port before anything else happens.
///
/// Separate from [`run`] so a caller can report the address, and start work
/// that writes to the terminal, only once the port is genuinely held. Announcing
/// first would be a lie if the port were busy, and starting generation first
/// leaves two threads writing over each other.
pub async fn bind(address: SocketAddr) -> Result<tokio::net::TcpListener, ServeError> {
    tokio::net::TcpListener::bind(address)
        .await
        .map_err(|source| ServeError::Bind { address, source })
}

async fn index_page(fixtures: Vec<Fixture>, base: String, progress: Progress) -> Html<String> {
    let readiness = progress
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    Html(index::render(&fixtures, &base, &readiness))
}
