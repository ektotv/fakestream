//! The catalogue. One declaration per test asset, used both to generate the
//! files and to render the index a player developer browses, so the two cannot
//! drift apart.

use crate::captions::cea608::ChannelCues;
use crate::captions::libcaption::Channel;
use crate::captions::script::{lorem_cues, offset_cues};
use crate::media::MediaError;
use crate::media::mux::{ClipSpec, write_clip};
use std::ffi::CString;
use std::path::{Path, PathBuf};

/// How a fixture reaches the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// A complete file, fetched with range requests.
    Vod,
}

impl Delivery {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vod => "VOD",
        }
    }
}

/// One test asset, declared rather than built imperatively.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// Stable identifier, also used as the cache filename stem.
    pub id: &'static str,
    pub title: &'static str,
    /// What a player developer would use this to test.
    pub purpose: &'static str,
    /// Path under the server root, which is also the path under the cache.
    pub route: &'static str,
    pub delivery: Delivery,
    pub spec: ClipSpec,
}

impl Fixture {
    /// Where this fixture's file lives inside the cache directory.
    pub fn cache_path(&self, root: &Path) -> PathBuf {
        root.join(self.route)
    }
}

/// Every fixture fakestream can produce.
pub fn catalogue() -> Vec<Fixture> {
    vec![
        Fixture {
            id: "vod-mp4-basic",
            title: "MP4, H.264 and AAC",
            purpose: "Baseline playback, seeking and AV sync. A beep sounds on every \
                  second boundary and the picture flashes a white marker on the \
                  same frame, so drift between audio and video is visible and \
                  audible at once.",
            route: "vod/basic.mp4",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                ..ClipSpec::default()
            },
        },
        Fixture {
            id: "vod-ts-cea608",
            title: "MPEG-TS with CEA-608 captions",
            purpose: "In-band closed captions carried in the video's SEI rather than \
                  as a separate track. Cues are numbered, so a dropped or \
                  mistimed caption is obvious. Tests whether a player finds \
                  captions that are not announced as a stream.",
            route: "vod/cea608.ts",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                cea608: vec![ChannelCues {
                    channel: Channel::One,
                    cues: lorem_cues(30.0, 3.0, 2.5),
                }],
                ..ClipSpec::default()
            },
        },
        Fixture {
            id: "vod-ts-cea608-dual",
            title: "MPEG-TS with two CEA-608 caption channels",
            purpose: "CC1 and CC2 carry different text, the way a broadcaster \
                      puts a second language on channel two. Tests whether a \
                      player lists both channels and actually renders the one \
                      the viewer picks, rather than always showing CC1.",
            route: "vod/cea608-dual.ts",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                cea608: vec![
                    ChannelCues {
                        channel: Channel::One,
                        cues: lorem_cues(30.0, 3.0, 2.5),
                    },
                    ChannelCues {
                        channel: Channel::Two,
                        // Offset so the two channels are never transmitting at
                        // the same moment, and visibly different so picking the
                        // wrong one is obvious.
                        cues: offset_cues(lorem_cues(30.0, 3.0, 2.5), 1.5, "CC2:"),
                    },
                ],
                ..ClipSpec::default()
            },
        },
    ]
}

#[derive(Debug)]
pub enum BuildError {
    Media(MediaError),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A path that cannot cross the FFI boundary, which means it holds a null.
    UnusablePath(PathBuf),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Media(error) => write!(formatter, "{error}"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::UnusablePath(path) => {
                write!(formatter, "{} cannot be passed to ffmpeg", path.display())
            }
        }
    }
}

impl std::error::Error for BuildError {}

impl From<MediaError> for BuildError {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

/// Generate a fixture into the cache, unless it is already there.
///
/// Returns whether it did any work, which lets the caller report progress
/// without a rebuild looking like a first run.
pub fn build(fixture: &Fixture, root: &Path) -> Result<bool, BuildError> {
    let target = fixture.cache_path(root);
    if target.exists() {
        return Ok(false);
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| BuildError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    // Write to a neighbouring temporary name first, so an interrupted run
    // cannot leave a half written file that later looks complete.
    //
    // The extension has to survive that rename, because libavformat picks its
    // muxer from the filename. A temporary called `.partial` gets no muxer at
    // all and the write fails before it starts.
    let partial = temporary_name(&target);
    let c_path = CString::new(partial.to_string_lossy().as_bytes())
        .map_err(|_| BuildError::UnusablePath(partial.clone()))?;

    write_clip(&c_path, &fixture.spec)?;

    std::fs::rename(&partial, &target).map_err(|source| BuildError::Io {
        path: target.clone(),
        source,
    })?;

    Ok(true)
}

/// A hidden sibling of `target` that keeps the same extension, so the muxer is
/// still inferred correctly while the file is being written.
fn temporary_name(target: &Path) -> PathBuf {
    match target.file_name().and_then(|name| name.to_str()) {
        Some(name) => target.with_file_name(format!(".partial-{name}")),
        None => target.with_file_name(".partial"),
    }
}

/// Build everything in the catalogue.
pub fn build_all(root: &Path) -> Result<Vec<(Fixture, bool)>, BuildError> {
    catalogue()
        .into_iter()
        .map(|fixture| build(&fixture, root).map(|built| (fixture, built)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_is_unique() {
        let mut routes: Vec<&str> = catalogue().iter().map(|fixture| fixture.route).collect();
        routes.sort_unstable();
        let count = routes.len();
        routes.dedup();
        assert_eq!(routes.len(), count, "two fixtures share a route");
    }

    #[test]
    fn every_id_is_unique() {
        let mut ids: Vec<&str> = catalogue().iter().map(|fixture| fixture.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "two fixtures share an id");
    }

    #[test]
    fn the_temporary_name_keeps_the_extension() {
        // libavformat picks its muxer from the extension, so losing it here
        // breaks generation before a byte is written.
        let target = Path::new("fixtures/vod/basic.mp4");
        let partial = temporary_name(target);

        assert_eq!(partial.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert_eq!(partial.parent(), target.parent());
        assert_ne!(partial, target);
    }

    #[test]
    fn routes_stay_inside_the_cache() {
        for fixture in catalogue() {
            assert!(
                !fixture.route.starts_with('/') && !fixture.route.contains(".."),
                "{} has a route that could escape the cache directory",
                fixture.id
            );
        }
    }
}
