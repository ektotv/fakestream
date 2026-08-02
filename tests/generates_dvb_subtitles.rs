//! DVB subtitles are pictures on their own stream, so they cannot be checked by
//! reading text. These tests decode the artefact and look at what a player
//! would actually receive.

use fakestream::captions::script::lorem_cues;
use fakestream::media::mux::{ClipSpec, write_clip};
use fakestream::media::subtitles::{SubtitleFormat, SubtitleTrack};
use fakestream::media::verify::{StreamKind, inspect, subtitle_events};
use std::ffi::CString;

struct TempFile {
    path: std::path::PathBuf,
}

impl TempFile {
    fn new(name: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("fakestream-dvb-{}-{name}", std::process::id()));
        Self { path }
    }

    fn c_path(&self) -> CString {
        CString::new(self.path.to_string_lossy().as_bytes()).expect("path holds no null byte")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Three cues over twelve seconds, appearing every three and lasting two.
fn spec() -> ClipSpec {
    ClipSpec {
        width: 640,
        height: 480,
        duration_seconds: 12.0,
        video_bitrate: 400_000,
        subtitles: vec![SubtitleTrack::new(
            SubtitleFormat::Dvb,
            "eng",
            "English",
            lorem_cues(12.0, 3.0, 2.0),
        )],
        ..ClipSpec::default()
    }
}

#[test]
fn the_container_announces_a_subtitle_track() {
    let file = TempFile::new("announced.ts");
    write_clip(&file.c_path(), &spec()).expect("clip should generate");

    let streams = inspect(&file.c_path()).expect("generated file should open");
    let subtitle = streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Subtitle)
        .expect("no subtitle stream was announced");

    assert_eq!(subtitle.codec, "dvb_subtitle");
}

#[test]
fn every_caption_is_followed_by_a_clear() {
    // Without a clear the caption stays up until the decoder's own page
    // timeout, which ffmpeg hard codes to thirty seconds and which has nothing
    // to do with how long the cue was meant to last.
    let file = TempFile::new("cleared.ts");
    let spec = spec();
    write_clip(&file.c_path(), &spec).expect("clip should generate");

    let events = subtitle_events(&file.c_path(), 0).expect("subtitles should decode");
    assert!(!events.is_empty(), "no subtitle events were decoded");

    let captions = events.iter().filter(|event| event.rects > 0).count();
    let clears = events.iter().filter(|event| event.rects == 0).count();

    assert_eq!(captions, spec.subtitles[0].cues.len());
    assert_eq!(clears, captions, "every caption needs a clear after it");
}

#[test]
fn captions_and_clears_land_when_the_cues_asked() {
    let file = TempFile::new("timing.ts");
    let spec = spec();
    write_clip(&file.c_path(), &spec).expect("clip should generate");

    let events = subtitle_events(&file.c_path(), 0).expect("subtitles should decode");

    // MPEG-TS applies a small start offset to every stream, so compare against
    // the first event rather than against zero.
    let offset = events[0].at - spec.subtitles[0].cues[0].start;

    for (index, cue) in spec.subtitles[0].cues.iter().enumerate() {
        let caption = &events[index * 2];
        let clear = &events[index * 2 + 1];

        assert!(caption.rects > 0, "event {index} should be a caption");
        assert!(
            clear.rects == 0,
            "event {index} should be followed by a clear"
        );

        assert!(
            (caption.at - offset - cue.start).abs() < 0.05,
            "caption {index} landed at {:.3}s, expected {:.3}s",
            caption.at - offset,
            cue.start
        );
        assert!(
            (clear.at - offset - (cue.start + cue.duration)).abs() < 0.05,
            "clear {index} landed at {:.3}s, expected {:.3}s",
            clear.at - offset,
            cue.start + cue.duration
        );
    }
}

#[test]
fn a_clip_without_cues_has_no_subtitle_stream() {
    let file = TempFile::new("none.ts");
    let spec = ClipSpec {
        subtitles: Vec::new(),
        ..spec()
    };
    write_clip(&file.c_path(), &spec).expect("clip should generate");

    let streams = inspect(&file.c_path()).expect("generated file should open");
    assert!(
        streams
            .iter()
            .all(|stream| stream.kind != StreamKind::Subtitle),
        "a subtitle stream was announced with nothing to put in it"
    );
}
