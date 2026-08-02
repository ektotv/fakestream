//! The catalogue. One declaration per test asset, used both to generate the
//! files and to render the index a player developer browses, so the two cannot
//! drift apart.

mod cache;

use crate::captions::cea608::ChannelCues;
use crate::captions::libcaption::Channel;
use crate::captions::script::{lorem_cues, offset_cues};
use crate::media::MediaError;
use crate::media::hls::{HlsOptions, PlaylistKind, SegmentType};
use crate::media::mux::{ClipSpec, Target, write_clip_reporting};
use crate::media::subtitles::{SubtitleFormat, SubtitleTrack};
use std::ffi::CString;
use std::path::{Path, PathBuf};

/// How a fixture reaches the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// A complete file, fetched with range requests.
    Vod,
    /// An endless stream, generated as it is watched. Nothing is written to
    /// disk and there is nothing to build in advance.
    Live,
}

impl Delivery {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vod => "VOD",
            Self::Live => "LIVE",
        }
    }

    /// Live streams are produced on demand, so there is no file to cache.
    pub fn is_generated_ahead(self) -> bool {
        matches!(self, Self::Vod)
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
    /// Set when the fixture is packaged as HLS rather than a single file.
    pub hls: Option<HlsOptions>,
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
            hls: None,
        },
        Fixture {
            id: "live-ts",
            title: "Live MPEG-TS, generated as you watch",
            purpose: "An endless progressive stream, the shape most IPTV \
                      providers serve. The picture carries a UTC clock, elapsed \
                      time and a frame counter, so holding a real clock next to \
                      the screen measures end to end latency directly, and a \
                      stalled or looping player is obvious.",
            route: "live/stream.ts",
            delivery: Delivery::Live,
            spec: ClipSpec {
                // Unbounded in practice; the stream runs until the viewer stops.
                duration_seconds: 0.0,
                ..ClipSpec::default()
            },
            hls: None,
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
            hls: None,
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
            hls: None,
        },
        Fixture {
            id: "vod-ts-dvbsub",
            title: "MPEG-TS with DVB bitmap subtitles",
            purpose: "Subtitles as pictures on their own announced stream, the \
                      opposite case to CEA-608. Common on European IPTV. Tests \
                      whether a player lists the track, decodes the bitmaps and \
                      places them correctly against the video.",
            route: "vod/dvbsub.ts",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: vec![SubtitleTrack::new(
                    SubtitleFormat::Dvb,
                    "eng",
                    "English (DVB bitmap)",
                    lorem_cues(30.0, 3.0, 2.5),
                )],
                ..ClipSpec::default()
            },
            hls: None,
        },
        Fixture {
            id: "vod-mp4-tx3g",
            title: "MP4 with tx3g timed text",
            purpose: "The usual subtitle format inside MP4. The player renders \
                      the text itself, so this tests its font handling, line \
                      breaking and placement rather than ours.",
            route: "vod/tx3g.mp4",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: vec![SubtitleTrack::new(
                    SubtitleFormat::Tx3g,
                    "eng",
                    "English",
                    lorem_cues(30.0, 3.0, 2.5),
                )],
                ..ClipSpec::default()
            },
            hls: None,
        },
        Fixture {
            id: "vod-mp4-ttml",
            title: "MP4 with TTML timed text",
            purpose: "Carried in MP4 as stpp, and what DASH and CMAF use. A \
                      player that handles tx3g may still not handle this.",
            route: "vod/ttml.mp4",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: vec![SubtitleTrack::new(
                    SubtitleFormat::Ttml,
                    "eng",
                    "English",
                    lorem_cues(30.0, 3.0, 2.5),
                )],
                ..ClipSpec::default()
            },
            hls: None,
        },
        Fixture {
            id: "vod-mkv-subrip",
            title: "Matroska with SubRip subtitles",
            purpose: "Plain text subtitles with no styling, the simplest case a \
                      player can be asked to render.",
            route: "vod/subrip.mkv",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: vec![SubtitleTrack::new(
                    SubtitleFormat::SubRip,
                    "eng",
                    "English",
                    lorem_cues(30.0, 3.0, 2.5),
                )],
                ..ClipSpec::default()
            },
            hls: None,
        },
        Fixture {
            id: "vod-mkv-ass",
            title: "Matroska with ASS subtitles",
            purpose: "ASS carries styling and positioning, so it exercises far \
                      more of a renderer than SubRip does.",
            route: "vod/ass.mkv",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: vec![SubtitleTrack::new(
                    SubtitleFormat::Ass,
                    "eng",
                    "English",
                    lorem_cues(30.0, 3.0, 2.5),
                )],
                ..ClipSpec::default()
            },
            hls: None,
        },
        Fixture {
            id: "vod-mkv-webvtt",
            title: "Matroska with WebVTT subtitles",
            purpose: "WebVTT away from HLS. Its usual home is a separate HLS \
                      rendition, which is not built yet.",
            route: "vod/webvtt.mkv",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: vec![SubtitleTrack::new(
                    SubtitleFormat::WebVtt,
                    "eng",
                    "English",
                    lorem_cues(30.0, 3.0, 2.5),
                )],
                ..ClipSpec::default()
            },
            hls: None,
        },
        Fixture {
            id: "hls-ts",
            title: "HLS with MPEG-TS segments",
            purpose: "Segmented delivery with WebVTT subtitle renditions in \
                      four languages, announced in the master playlist. This is \
                      what most IPTV services serve, and the subtitles arrive as \
                      separate renditions rather than inside the media.",
            route: "hls/ts/master.m3u8",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: hls_subtitle_tracks(),
                ..ClipSpec::default()
            },
            hls: Some(HlsOptions {
                segment_type: SegmentType::MpegTs,
                segment_seconds: 4.0,
                kind: PlaylistKind::Vod,
                master_name: "master.m3u8".to_string(),
            }),
        },
        Fixture {
            id: "hls-fmp4",
            title: "HLS with fragmented MP4 segments",
            purpose: "The same content packaged as fMP4, which newer services \
                      use and which lifts the playlist to version 7. A player \
                      takes a different path through its extractor for these, \
                      so handling TS says nothing about handling these.",
            route: "hls/fmp4/master.m3u8",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: hls_subtitle_tracks(),
                ..ClipSpec::default()
            },
            hls: Some(HlsOptions {
                segment_type: SegmentType::FragmentedMp4,
                segment_seconds: 4.0,
                kind: PlaylistKind::Vod,
                master_name: "master.m3u8".to_string(),
            }),
        },
        Fixture {
            id: "vod-mkv-multilingual",
            title: "Matroska with subtitles in four languages",
            purpose: "Four tracks tagged eng, fra, spa and jpn, each with \
                      visibly different text. Tests track listing, language \
                      labelling, switching, and whether a player can honour a \
                      stored language preference.",
            route: "vod/multilingual.mkv",
            delivery: Delivery::Vod,
            spec: ClipSpec {
                duration_seconds: 30.0,
                subtitles: LANGUAGES
                    .iter()
                    .map(|(tag, label, prefix)| {
                        SubtitleTrack::new(
                            SubtitleFormat::SubRip,
                            tag,
                            label,
                            offset_cues(lorem_cues(30.0, 3.0, 2.5), 0.0, prefix),
                        )
                    })
                    .collect(),
                ..ClipSpec::default()
            },
            hls: None,
        },
    ]
}

/// HLS carries subtitles as WebVTT renditions whatever the segment format, so
/// this is WebVTT regardless of what the media segments are.
///
/// Only one language, which is a limit of ffmpeg's HLS muxer rather than a
/// choice. It writes a subtitle rendition only for a variant that also carries
/// video, and gives each variant exactly one WebVTT stream, so several
/// languages would mean duplicating the video once per language. Feeding it
/// more than one subtitle stream in a variant fails outright with "webvtt muxer
/// does not support more than one stream of type subtitle".
///
/// The multilingual Matroska fixture covers language selection meanwhile.
fn hls_subtitle_tracks() -> Vec<SubtitleTrack> {
    vec![SubtitleTrack::new(
        SubtitleFormat::WebVtt,
        "eng",
        "English",
        lorem_cues(30.0, 3.0, 2.5),
    )]
}

/// Languages for the multilingual fixture. The prefix goes in front of every
/// cue so the track on screen is identifiable without reading the track list.
const LANGUAGES: [(&str, &str, &str); 4] = [
    ("eng", "English", "[EN]"),
    ("fra", "Français", "[FR] Français"),
    ("spa", "Español", "[ES] Español"),
    ("jpn", "日本語", "[JA] 日本語"),
];

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

/// Generate a fixture into the cache, unless a current one is already there.
///
/// Returns whether it did any work, which lets a caller report progress
/// without a rebuild looking like a first run.
pub fn build(fixture: &Fixture, root: &Path) -> Result<bool, BuildError> {
    build_reporting(fixture, root, &mut |_| {})
}

/// As [`build`], reporting how far through generation is.
pub fn build_reporting(
    fixture: &Fixture,
    root: &Path,
    progress: &mut dyn FnMut(f64),
) -> Result<bool, BuildError> {
    let target = fixture.cache_path(root);
    let signature = cache::signature(&fixture.spec);

    if cache::is_current(&target, &signature) {
        return Ok(false);
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| BuildError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    // HLS writes a directory of playlists and segments rather than one file,
    // so it is staged a directory at a time. Renaming only the playlist would
    // leave every segment named after the temporary.
    match &fixture.hls {
        Some(options) => build_hls(fixture, &target, options, progress)?,
        None => build_file(fixture, &target, progress)?,
    }

    cache::record(&target, &signature).map_err(|source| BuildError::Io {
        path: target.clone(),
        source,
    })?;

    Ok(true)
}

/// Generate a single file fixture, staged under a temporary name so an
/// interrupted run cannot leave a half written file that later looks complete.
fn build_file(
    fixture: &Fixture,
    target: &Path,
    progress: &mut dyn FnMut(f64),
) -> Result<(), BuildError> {
    let partial = cache::partial_name(target);
    let c_path = CString::new(partial.to_string_lossy().as_bytes())
        .map_err(|_| BuildError::UnusablePath(partial.clone()))?;

    write_clip_reporting(&Target::File(&c_path), &fixture.spec, progress)?;

    std::fs::rename(&partial, target).map_err(|source| BuildError::Io {
        path: target.to_path_buf(),
        source,
    })
}

/// Generate an HLS fixture into a staging directory, then move the whole
/// directory into place so a reader never sees a partial playlist.
fn build_hls(
    fixture: &Fixture,
    target: &Path,
    options: &HlsOptions,
    progress: &mut dyn FnMut(f64),
) -> Result<(), BuildError> {
    let final_dir = target.parent().unwrap_or(Path::new(".")).to_path_buf();
    let staging = cache::partial_name(&final_dir);

    // Anything left from a previous attempt would be mixed in with this one.
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|source| BuildError::Io {
        path: staging.clone(),
        source,
    })?;

    // The muxer derives segment and rendition names from the variant playlist
    // path, and writes the master playlist beside it.
    let variant = staging.join("variant%v.m3u8");
    let c_path = CString::new(variant.to_string_lossy().as_bytes())
        .map_err(|_| BuildError::UnusablePath(variant.clone()))?;

    write_clip_reporting(
        &Target::Hls {
            playlist: &c_path,
            options,
        },
        &fixture.spec,
        progress,
    )?;

    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&staging, &final_dir).map_err(|source| BuildError::Io {
        path: final_dir,
        source,
    })
}

/// Build everything in the catalogue, reporting as it goes.
///
/// Sweeps away anything an interrupted run left behind first. Those files are
/// hidden, so they never appear in a listing and would otherwise pile up.
pub fn build_all(root: &Path, watcher: &mut dyn FnMut(Report<'_>)) -> Result<(), BuildError> {
    let swept = cache::sweep_partials(root);
    if swept > 0 {
        watcher(Report::SweptPartials(swept));
    }

    let catalogue: Vec<Fixture> = catalogue()
        .into_iter()
        .filter(|fixture| fixture.delivery.is_generated_ahead())
        .collect();

    for (index, fixture) in catalogue.iter().enumerate() {
        watcher(Report::Started {
            fixture,
            index,
            total: catalogue.len(),
        });

        let built = build_reporting(fixture, root, &mut |fraction| {
            watcher(Report::Progress { fixture, fraction });
        })?;

        watcher(Report::Finished { fixture, built });
    }

    Ok(())
}

/// What a caller is told while the catalogue is being built.
pub enum Report<'a> {
    /// Files left by an interrupted run were removed.
    SweptPartials(usize),
    Started {
        fixture: &'a Fixture,
        index: usize,
        total: usize,
    },
    Progress {
        fixture: &'a Fixture,
        /// Zero to one.
        fraction: f64,
    },
    Finished {
        fixture: &'a Fixture,
        /// False when a current file was already on disk.
        built: bool,
    },
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
