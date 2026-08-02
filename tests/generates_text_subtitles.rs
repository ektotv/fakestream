//! Text subtitles are rendered by the player, so what matters is that the right
//! characters reach it, in the right track, at the right time.

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
        path.push(format!("fakestream-text-{}-{name}", std::process::id()));
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

fn spec_with(format: SubtitleFormat) -> ClipSpec {
    ClipSpec {
        width: 640,
        height: 480,
        duration_seconds: 12.0,
        video_bitrate: 400_000,
        subtitles: vec![SubtitleTrack::new(
            format,
            "eng",
            "English",
            lorem_cues(12.0, 3.0, 2.0),
        )],
        ..ClipSpec::default()
    }
}

/// Formats ffmpeg can also decode, so a round trip is possible. TTML is absent
/// deliberately: ffmpeg ships an encoder but no decoder, so a TTML fixture can
/// only be checked by reading the document, not by decoding it.
const ROUND_TRIP: [(SubtitleFormat, &str); 4] = [
    (SubtitleFormat::Tx3g, "mp4"),
    (SubtitleFormat::SubRip, "mkv"),
    (SubtitleFormat::Ass, "mkv"),
    (SubtitleFormat::WebVtt, "mkv"),
];

#[test]
fn every_text_format_carries_its_cues() {
    for (format, container) in ROUND_TRIP {
        let file = TempFile::new(&format!("{format:?}.{container}"));
        let spec = spec_with(format);
        write_clip(&file.c_path(), &spec).expect("clip should generate");

        let events = subtitle_events(&file.c_path(), 0).expect("subtitles should decode");
        assert_eq!(
            events.len(),
            spec.subtitles[0].cues.len(),
            "{format:?} lost cues"
        );

        let first = events[0].text.join(" ");
        assert!(
            first.contains("Lorem ipsum"),
            "{format:?} produced {first:?} rather than the cue text"
        );
    }
}

#[test]
fn cues_keep_their_numbering_so_drops_are_visible() {
    for (format, container) in ROUND_TRIP {
        let file = TempFile::new(&format!("order-{format:?}.{container}"));
        write_clip(&file.c_path(), &spec_with(format)).expect("clip should generate");

        let events = subtitle_events(&file.c_path(), 0).expect("subtitles should decode");
        for (index, event) in events.iter().enumerate() {
            let expected = format!("{}.", index + 1);
            assert!(
                event.text.join(" ").starts_with(&expected),
                "{format:?} cue {index} did not start with {expected:?}"
            );
        }
    }
}

#[test]
fn text_formats_land_when_the_cues_asked() {
    for (format, container) in ROUND_TRIP {
        let file = TempFile::new(&format!("timing-{format:?}.{container}"));
        let spec = spec_with(format);
        write_clip(&file.c_path(), &spec).expect("clip should generate");

        let events = subtitle_events(&file.c_path(), 0).expect("subtitles should decode");
        for (event, cue) in events.iter().zip(spec.subtitles[0].cues.iter()) {
            assert!(
                (event.at - cue.start).abs() < 0.1,
                "{format:?} cue landed at {:.3}s, expected {:.3}s",
                event.at,
                cue.start
            );
        }
    }
}

#[test]
fn ttml_is_written_even_though_ffmpeg_cannot_read_it_back() {
    let file = TempFile::new("ttml.mp4");
    write_clip(&file.c_path(), &spec_with(SubtitleFormat::Ttml)).expect("clip should generate");

    let streams = inspect(&file.c_path()).expect("generated file should open");
    let subtitle = streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Subtitle)
        .expect("no subtitle stream was written");

    assert_eq!(subtitle.codec, "ttml");
}

#[test]
fn several_languages_become_several_tracks() {
    let file = TempFile::new("multi.mkv");
    let languages = ["eng", "fra", "spa", "jpn"];
    let spec = ClipSpec {
        width: 640,
        height: 480,
        duration_seconds: 12.0,
        video_bitrate: 400_000,
        subtitles: languages
            .iter()
            .map(|tag| {
                SubtitleTrack::new(SubtitleFormat::SubRip, tag, tag, lorem_cues(12.0, 3.0, 2.0))
            })
            .collect(),
        ..ClipSpec::default()
    };

    write_clip(&file.c_path(), &spec).expect("clip should generate");

    let streams = inspect(&file.c_path()).expect("generated file should open");
    let subtitles = streams
        .iter()
        .filter(|stream| stream.kind == StreamKind::Subtitle)
        .count();
    assert_eq!(subtitles, languages.len());

    // Every track must be independently readable, not just the first.
    for track in 0..languages.len() {
        let events = subtitle_events(&file.c_path(), track).expect("subtitles should decode");
        assert!(!events.is_empty(), "track {track} decoded nothing");
    }
}
