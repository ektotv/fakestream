//! CEA captions ride in the video SEI, which ffmpeg never mentions in the PMT.
//! This fixture adds the caption_service_descriptor a player looks for when it
//! only shows captions the container advertises. These tests build the real
//! fixture and read the PMT back out of the muxed file.

use fakestream::captions::cea608::ChannelCues;
use fakestream::captions::libcaption::Channel;
use fakestream::captions::script::lorem_cues;
use fakestream::fixtures::{self, Delivery, Fixture};
use fakestream::media::mux::ClipSpec;
use fakestream::media::pmt::{self, CaptionService, ServiceKind};
use fakestream::media::verify::{StreamKind, inspect};
use std::ffi::CString;
use std::path::PathBuf;

/// A temporary directory that a fixture is built into, swept on drop.
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(name: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("fakestream-pmt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        Self { path }
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A short TS carrying both CEA-608 and 708, announced in the PMT when asked.
/// Kept brief so the encode is quick.
fn fixture(route: &'static str, announce: bool) -> Fixture {
    Fixture {
        id: "test-pmt",
        title: "test",
        purpose: "test",
        route,
        delivery: Delivery::Vod,
        spec: ClipSpec {
            width: 320,
            height: 240,
            duration_seconds: 2.0,
            video_bitrate: 300_000,
            cea608: vec![ChannelCues {
                channel: Channel::One,
                cues: lorem_cues(2.0, 0.5, 0.4),
            }],
            cea708: lorem_cues(2.0, 0.5, 0.4),
            announce_captions_in_pmt: announce,
            ..ClipSpec::default()
        },
        hls: None,
    }
}

/// The descriptor the fixture should carry: a 608 field 1 service and 708
/// service 1, both English and wide aspect.
fn expected_descriptor() -> Vec<u8> {
    pmt::caption_service_descriptor(&[
        CaptionService {
            language: *b"eng",
            kind: ServiceKind::Line21 { field2: false },
            easy_reader: false,
            wide_aspect: true,
        },
        CaptionService {
            language: *b"eng",
            kind: ServiceKind::Digital { service_number: 1 },
            easy_reader: false,
            wide_aspect: true,
        },
    ])
}

#[test]
fn the_pmt_carries_the_caption_descriptor() {
    let root = TempRoot::new("announced");
    let fixture = fixture("vod/announced.ts", true);
    fixtures::build(&fixture, &root.path).expect("fixture should build");

    let ts = std::fs::read(fixture.cache_path(&root.path)).expect("read the muxed file");
    let descriptor =
        pmt::video_caption_descriptor(&ts).expect("the PMT should announce the captions");

    assert_eq!(descriptor, expected_descriptor());
}

#[test]
fn the_stream_still_decodes_after_the_rewrite() {
    // Splicing the descriptor in must leave a PMT ffmpeg can still read, or the
    // fix would break the file it was meant to improve.
    let root = TempRoot::new("valid");
    let fixture = fixture("vod/valid.ts", true);
    fixtures::build(&fixture, &root.path).expect("fixture should build");

    let path = CString::new(fixture.cache_path(&root.path).to_string_lossy().as_bytes())
        .expect("path holds no null byte");
    let streams = inspect(&path).expect("the rewritten file should still open");

    assert!(
        streams
            .iter()
            .any(|stream| stream.kind == StreamKind::Video),
        "the video stream went missing after the rewrite"
    );
}

#[test]
fn without_the_flag_the_pmt_says_nothing() {
    // The plain CEA fixtures rely on this: their captions stay unannounced.
    let root = TempRoot::new("plain");
    let fixture = fixture("vod/plain.ts", false);
    fixtures::build(&fixture, &root.path).expect("fixture should build");

    let ts = std::fs::read(fixture.cache_path(&root.path)).expect("read the muxed file");
    assert_eq!(
        pmt::video_caption_descriptor(&ts),
        None,
        "an unannounced fixture grew a caption descriptor"
    );
}
