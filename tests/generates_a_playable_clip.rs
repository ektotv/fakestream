//! Generation is checked by reading the artefact back, not by trusting the code
//! that wrote it.

use fakestream::media::mux::{ClipSpec, write_clip};
use fakestream::media::verify::{StreamKind, inspect};
use std::ffi::CString;

/// A short clip keeps the test quick while still exercising every stage.
fn short_spec() -> ClipSpec {
    ClipSpec {
        width: 320,
        height: 240,
        fps: 25,
        duration_seconds: 1.0,
        video_bitrate: 200_000,
        ..ClipSpec::default()
    }
}

struct TempFile {
    path: std::path::PathBuf,
}

impl TempFile {
    fn new(name: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("fakestream-test-{}-{name}", std::process::id()));
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

#[test]
fn writes_an_mp4_with_video_and_audio() {
    let file = TempFile::new("clip.mp4");
    let spec = short_spec();

    write_clip(&file.c_path(), &spec).expect("clip should generate");

    let streams = inspect(&file.c_path()).expect("generated file should open");
    assert_eq!(streams.len(), 2, "expected one video and one audio stream");

    let video = &streams[0];
    assert_eq!(video.kind, StreamKind::Video);
    assert_eq!(video.codec, "h264");
    assert_eq!(video.width, spec.width);
    assert_eq!(video.height, spec.height);

    let audio = &streams[1];
    assert_eq!(audio.kind, StreamKind::Audio);
    assert_eq!(audio.codec, "aac");
    assert_eq!(audio.sample_rate, spec.sample_rate);
    assert_eq!(audio.channels, spec.channels as i32);
}

#[test]
fn duration_matches_what_was_asked_for() {
    let file = TempFile::new("duration.mp4");
    let spec = short_spec();

    write_clip(&file.c_path(), &spec).expect("clip should generate");
    let streams = inspect(&file.c_path()).expect("generated file should open");

    for stream in &streams {
        let drift = (stream.duration - spec.duration_seconds).abs();
        assert!(
            drift < 0.1,
            "{:?} stream ran {:.3}s against the {:.3}s asked for",
            stream.kind,
            stream.duration,
            spec.duration_seconds
        );
    }
}

#[test]
fn the_container_is_chosen_by_extension() {
    let file = TempFile::new("clip.ts");
    write_clip(&file.c_path(), &short_spec()).expect("mpegts should generate");

    let streams = inspect(&file.c_path()).expect("generated file should open");
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0].codec, "h264");
}
