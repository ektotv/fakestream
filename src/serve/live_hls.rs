//! Keeping a live HLS stream running while somebody is watching.
//!
//! Unlike progressive live, where each viewer gets their own stream down their
//! own connection, HLS viewers share one timeline written to disk. So there is
//! one writer per fixture rather than one per viewer, started when the first
//! request arrives and stopped once nobody has asked for anything in a while.

use crate::media::hls::HlsOptions;
use crate::media::live::LiveStream;
use crate::media::mux::ClipSpec;
use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long after the last request a stream keeps running.
///
/// Long enough that a player pausing between segment fetches does not kill it,
/// short enough that a forgotten tab does not leave an encoder burning a core.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to wait for the stream to be worth handing to a player.
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(60);

/// How many segments must exist before a viewer is let in.
///
/// Publishing as soon as the first segment lands puts a viewer right at the
/// live edge with nothing buffered, racing the encoder frame for frame, and any
/// hesitation shows as a jump. Apple's guidance is that a client should have
/// around three target durations available before it starts, which is what this
/// waits for. It only costs the first viewer, since the window stays full
/// afterwards.
const MINIMUM_SEGMENTS: usize = 3;

/// One running live HLS stream.
struct Running {
    /// Bumped on every request, so the writer can tell when it is unwatched.
    last_request: Arc<AtomicU64>,
    started: Instant,
}

impl Running {
    fn touch(&self) {
        self.last_request
            .store(self.started.elapsed().as_secs(), Ordering::Relaxed);
    }
}

/// Supervises the live HLS writers, one per fixture.
#[derive(Default)]
pub struct LiveHls {
    running: Mutex<std::collections::HashMap<String, Running>>,
}

impl LiveHls {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Make sure a stream is running, starting it if not, and wait until its
    /// playlist exists so the response is not a 404 on a stream that is about
    /// to work.
    pub async fn ensure_running(
        self: &Arc<Self>,
        id: &str,
        root: &Path,
        route: &str,
        spec: &ClipSpec,
        options: &HlsOptions,
    ) -> Result<(), String> {
        let playlist = root.join(route);
        let directory = playlist.parent().unwrap_or(root).to_path_buf();

        let already_running = {
            let mut running = self
                .running
                .lock()
                .map_err(|_| "supervisor lock poisoned")?;
            match running.get(id) {
                Some(stream) => {
                    stream.touch();
                    true
                }
                None => {
                    let last_request = Arc::new(AtomicU64::new(0));
                    running.insert(
                        id.to_string(),
                        Running {
                            last_request: Arc::clone(&last_request),
                            started: Instant::now(),
                        },
                    );

                    let supervisor = Arc::clone(self);
                    let id = id.to_string();
                    let spec = spec.clone();
                    let options = options.clone();
                    let writer_directory = directory.clone();

                    std::thread::spawn(move || {
                        let outcome = run_writer(&writer_directory, &spec, &options, &last_request);
                        if let Err(error) = outcome {
                            eprintln!("live hls stopped: {error}");
                        }
                        if let Ok(mut running) = supervisor.running.lock() {
                            running.remove(&id);
                        }
                    });

                    false
                }
            }
        };

        let extension = options.segment_type.extension();
        if already_running && ready(&playlist, &directory, extension) {
            return Ok(());
        }

        wait_until_ready(&playlist, &directory, extension).await
    }
}

/// Is there a playlist, and enough behind it to start playing?
fn ready(playlist: &Path, directory: &Path, extension: &str) -> bool {
    playlist.exists() && finished_segments(directory, extension) >= MINIMUM_SEGMENTS
}

/// Count segments that are complete.
///
/// The muxer writes each segment under a temporary name and renames it when
/// finished, so anything carrying the real extension is whole.
fn finished_segments(directory: &Path, extension: &str) -> usize {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };

    entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|found| found.to_str())
                .is_some_and(|found| found == extension)
        })
        .count()
}

/// Wait until the stream has enough behind it to hand to a player.
async fn wait_until_ready(
    playlist: &Path,
    directory: &Path,
    extension: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + PLAYLIST_TIMEOUT;

    while Instant::now() < deadline {
        if ready(playlist, directory, extension) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err("the live stream never produced enough segments to start".to_string())
}

/// Generate frames in real time until nobody is watching.
fn run_writer(
    directory: &Path,
    spec: &ClipSpec,
    options: &HlsOptions,
    last_request: &AtomicU64,
) -> Result<(), String> {
    // Start from an empty directory, or a viewer joining could be handed
    // segments from a previous run that no longer match the playlist.
    let _ = std::fs::remove_dir_all(directory);
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;

    let variant = directory.join(HlsOptions::VARIANT_TEMPLATE);
    let path = CString::new(variant.to_string_lossy().as_bytes())
        .map_err(|_| "the stream directory cannot be passed to ffmpeg".to_string())?;

    let mut stream = LiveStream::hls(spec.clone(), &path, options).map_err(|e| e.to_string())?;
    let started = Instant::now();

    loop {
        let wait = stream.wait_before_next();
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }

        stream.next_chunk().map_err(|error| error.to_string())?;

        let idle = started
            .elapsed()
            .as_secs()
            .saturating_sub(last_request.load(Ordering::Relaxed));
        if idle > IDLE_TIMEOUT.as_secs() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("fakestream-live-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn touch(&self, name: &str) {
            std::fs::write(self.0.join(name), b"x").expect("write");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_playlist_alone_is_not_enough_to_start() {
        // Handing a viewer a playlist with one segment puts them at the live
        // edge with nothing buffered, racing the encoder frame for frame.
        let dir = TempDir::new("bare");
        dir.touch("master.m3u8");
        dir.touch("segment0-00000.ts");

        assert!(!ready(&dir.0.join("master.m3u8"), &dir.0, "ts"));
    }

    #[test]
    fn enough_segments_makes_a_stream_ready() {
        let dir = TempDir::new("ready");
        dir.touch("master.m3u8");
        for index in 0..MINIMUM_SEGMENTS {
            dir.touch(&format!("segment0-0000{index}.ts"));
        }

        assert!(ready(&dir.0.join("master.m3u8"), &dir.0, "ts"));
    }

    #[test]
    fn segments_still_being_written_do_not_count() {
        // The muxer writes under a temporary name and renames on completion, so
        // only files carrying the real extension are whole.
        let dir = TempDir::new("partial");
        dir.touch("master.m3u8");
        dir.touch("segment0-00000.ts");
        dir.touch("segment0-00001.ts");
        dir.touch("segment0-00002.ts.tmp");

        assert_eq!(finished_segments(&dir.0, "ts"), 2);
        assert!(!ready(&dir.0.join("master.m3u8"), &dir.0, "ts"));
    }

    #[test]
    fn fragmented_segments_are_counted_by_their_own_extension() {
        let dir = TempDir::new("fmp4");
        dir.touch("init.mp4");
        for index in 0..MINIMUM_SEGMENTS {
            dir.touch(&format!("segment0-0000{index}.m4s"));
        }

        assert_eq!(finished_segments(&dir.0, "m4s"), MINIMUM_SEGMENTS);
        // The init segment is an mp4 and must not be mistaken for media.
        assert_eq!(finished_segments(&dir.0, "ts"), 0);
    }

    #[test]
    fn a_missing_directory_reports_nothing_rather_than_failing() {
        assert_eq!(finished_segments(Path::new("nowhere-at-all"), "ts"), 0);
    }
}
