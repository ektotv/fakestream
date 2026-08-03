//! The HTTP surface. One server exposes every fixture at once, so a player can
//! be pointed at a single host and switched between formats without restarting
//! anything.

mod demand;
mod index;
mod live;
mod live_hls;

use crate::fixtures::{Delivery, Fixture, catalogue};
use axum::Router;
use axum::extract::{Request, State};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use demand::Demand;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
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
    /// Not on disk yet. Still linked, since asking for it is what builds it.
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
        .map(|fixture| {
            // Live streams need no building, so they are ready immediately
            // rather than sitting in a queue that will never reach them.
            let state = if fixture.delivery.is_generated_ahead() {
                Readiness::Waiting
            } else {
                Readiness::Ready
            };
            (fixture.id.to_string(), state)
        })
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
    quiet: bool,
) -> Result<(), ServeError> {
    let fixtures = catalogue();
    let base = format!("http://{address}");
    let demand = Demand::new(root.clone(), quiet);
    let streams = live_hls::LiveHls::new();

    let progress_for_handler = Arc::clone(&progress);

    let mut app = Router::new().route(
        "/",
        get({
            let fixtures = fixtures.clone();
            move || index_page(fixtures, base, Arc::clone(&progress))
        }),
    );

    // Live routes are handled rather than served from disk, since nothing is
    // written to disk for them.
    for fixture in fixtures
        .iter()
        .filter(|fixture| fixture.delivery == Delivery::Live)
    {
        let spec = fixture.spec.clone();
        app = app.route(
            &format!("/{}", fixture.route),
            get(move || live::stream(spec.clone())),
        );
    }

    // Everything else falls through here, which is where a fixture is
    // generated if this is the first time anyone has asked for it.
    let app = app.fallback(axum::routing::any(on_demand).with_state(Fixtures {
        demand,
        streams,
        progress: progress_for_handler,
        root,
    }));

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

/// Shared with the fallback handler.
#[derive(Clone)]
struct Fixtures {
    demand: Arc<Demand>,
    streams: Arc<live_hls::LiveHls>,
    progress: Progress,
    root: PathBuf,
}

/// Serve a file, generating it first if this is the first request for it.
///
/// The request blocks while generation runs, which for a fresh fixture is tens
/// of seconds. That is the trade: slow once, instant afterwards, and nothing is
/// built that nobody asked for.
async fn on_demand(State(state): State<Fixtures>, request: Request) -> Response {
    let path = request.uri().path().trim_start_matches('/').to_string();

    if let Some(fixture) = catalogue()
        .into_iter()
        .find(|fixture| fixture.route == path)
    {
        state.demand.log_request(&path);

        let outcome = match fixture.delivery {
            Delivery::Vod => state.demand.ensure_built(&fixture, &state.progress).await,
            Delivery::LiveHls => match &fixture.hls {
                Some(options) => {
                    state
                        .streams
                        .ensure_running(
                            fixture.id,
                            &state.root,
                            fixture.route,
                            &fixture.spec,
                            options,
                        )
                        .await
                }
                None => Err("a live hls fixture with no hls options".to_string()),
            },
            // Progressive live is handled by its own route.
            Delivery::Live => Ok(()),
        };

        if let Err(error) = outcome {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not serve {path}: {error}\n"),
            )
                .into_response();
        }
    } else if let Some(fixture) = live_fixture_owning(&catalogue(), &path) {
        // A player fetches the master playlist once and then polls only the
        // variant playlists and segments beside it, which carry no fixture
        // route of their own. They must still count as watching, or the
        // writer decides nobody is and stops the stream a minute in.
        state.streams.touch(fixture.id);
    }

    // Segments and playlists inside an HLS directory land here too, and by the
    // time a player asks for one its fixture has already been generated.
    match ServeDir::new(&state.root).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error}\n"),
        )
            .into_response(),
    }
}

/// The live HLS fixture whose directory contains this request path, if any.
///
/// A live stream is one fixture but many files. Only the master playlist
/// carries the fixture's route; the variant playlists and segments live beside
/// it and belong to the same stream.
fn live_fixture_owning<'a>(fixtures: &'a [Fixture], path: &str) -> Option<&'a Fixture> {
    fixtures.iter().find(|fixture| {
        fixture.delivery == Delivery::LiveHls
            && fixture
                .route
                .rsplit_once('/')
                .is_some_and(|(directory, _)| {
                    path.strip_prefix(directory)
                        .is_some_and(|rest| rest.starts_with('/'))
                })
    })
}

/// Mirror a generation report into the state the index page reads.
pub(crate) fn record_progress(progress: &Progress, report: &crate::fixtures::Report<'_>) {
    let Ok(mut state) = progress.lock() else {
        return;
    };

    match report {
        crate::fixtures::Report::Started { fixture, .. } => {
            state.insert(fixture.id.to_string(), Readiness::Building(0.0));
        }
        crate::fixtures::Report::Progress { fixture, fraction } => {
            state.insert(fixture.id.to_string(), Readiness::Building(*fraction));
        }
        crate::fixtures::Report::Finished { fixture, .. } => {
            state.insert(fixture.id.to_string(), Readiness::Ready);
        }
        crate::fixtures::Report::SweptPartials(_) => {}
    }
}

async fn index_page(fixtures: Vec<Fixture>, base: String, progress: Progress) -> Html<String> {
    let readiness = progress
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    Html(index::render(&fixtures, &base, &readiness))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_and_segment_requests_belong_to_the_live_stream() {
        // These are the requests a player actually keeps making, so they are
        // the ones that must register as watching.
        let fixtures = catalogue();

        let variant = live_fixture_owning(&fixtures, "live/hls/stream0.m3u8").expect("variant");
        assert_eq!(variant.delivery, Delivery::LiveHls);

        assert!(live_fixture_owning(&fixtures, "live/hls/segment0-00042.ts").is_some());
    }

    #[test]
    fn paths_outside_the_stream_directory_own_nothing() {
        let fixtures = catalogue();

        assert!(live_fixture_owning(&fixtures, "vod/basic.mp4").is_none());
        // A sibling directory sharing the prefix as a string is not inside it.
        assert!(live_fixture_owning(&fixtures, "live/hlsx/stream0.m3u8").is_none());
    }
}
