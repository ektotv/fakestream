//! Live streams, generated as they are watched.
//!
//! Nothing is written to disk and nothing loops. Frames are produced, encoded
//! and handed to the caller in real time, so a stream runs for as long as
//! somebody is watching and carries a clock that genuinely tracks the wall.
//!
//! Each viewer gets their own encoder, starting at the moment they connect.
//! That is not how a broadcast works, where everyone shares one timeline, but
//! it means a viewer never joins mid-keyframe and every stream begins cleanly.
//! It also means the cost scales with viewers, so this is a test tool rather
//! than a streaming server.

use super::hls::HlsOptions;
use super::mux::ClipSpec;
use super::pmt::PmtAnnouncer;
use super::{
    MediaError, clock, context, encode, ffi, overlay, source::Beeps, source::paint_pattern,
};
use crate::captions::feed::CaptionFeed;
use rsmpeg::avcodec::AVCodecContext;
use rsmpeg::avformat::AVIOContextContainer;
use rsmpeg::avformat::{AVFormatContextOutput, AVIOContextCustom};
use rsmpeg::avutil::{AVFrame, AVMem};
use rsmpeg::ffi as sys;
use std::ffi::CStr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Produces an endless MPEG-TS, paced to real time.
pub struct LiveStream {
    spec: ClipSpec,
    video: AVCodecContext,
    audio: AVCodecContext,
    picture: AVFrame,
    sound: AVFrame,
    beeps: Beeps,
    /// The SEI captions this stream carries, built from the spec's plan.
    captions: CaptionFeed,
    muxer: LiveMuxer,

    /// Seconds since the epoch when this stream began, which the clock counts
    /// from.
    unix_start: f64,
    /// Used to decide how long to wait, rather than accumulating sleeps, so
    /// pacing cannot drift.
    started: Instant,

    frame: u64,
    samples_written: i64,
    samples_per_frame: i32,
}

impl LiveStream {
    /// A stream muxed into memory, for handing straight to one HTTP response.
    pub fn new(spec: ClipSpec) -> Result<Self, MediaError> {
        Self::with_muxer(spec, LiveMuxer::memory_ts)
    }

    /// A stream segmented onto disk as HLS, for many viewers to fetch from.
    ///
    /// The playlist keeps a rolling window and expired segments are deleted, so
    /// a stream left running does not fill the disk.
    pub fn hls(spec: ClipSpec, playlist: &CStr, options: &HlsOptions) -> Result<Self, MediaError> {
        // One group of pictures per segment. Otherwise a segment can begin part
        // way through a group and only decode because the previous one happened
        // to be fetched, which is what makes players stutter at boundaries.
        let spec = ClipSpec {
            keyframe_seconds: options.segment_seconds,
            ..spec
        };
        let options = options.clone();
        let playlist = playlist.to_owned();
        Self::with_muxer(spec, move |spec, video, audio| {
            LiveMuxer::hls(spec, video, audio, &playlist, &options)
        })
    }

    fn with_muxer(
        spec: ClipSpec,
        make_muxer: impl FnOnce(
            &ClipSpec,
            &AVCodecContext,
            &AVCodecContext,
        ) -> Result<LiveMuxer, MediaError>,
    ) -> Result<Self, MediaError> {
        let unix_start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or(0.0);

        // Live holds no B-frames, since a held-back frame is delay a viewer
        // feels, and never emits a global header, since MPEG-TS carries codec
        // configuration in-band.
        let policy = encode::EncoderPolicy {
            global_header: false,
            cap_b_frames: true,
        };
        let video = encode::open_video_encoder(&spec, policy)?;
        let audio = encode::open_audio_encoder(&spec, policy)?;
        let muxer = make_muxer(&spec, &video, &audio)?;

        let picture = encode::new_video_frame(&spec)?;
        let samples_per_frame = audio.frame_size.max(1024);
        let sound = encode::new_audio_frame(&spec, samples_per_frame)?;

        let beeps = Beeps::every_second(spec.sample_rate as u32);
        // Captions come from the spec's plan, so the plain live streams carry
        // none and dedicated fixtures ask for 608, 708, or both. A live stream
        // has no known length, so total_frames is nought here.
        let captions = context(
            "scheduling live captions",
            CaptionFeed::build(&spec.captions, spec.fps, 0, unix_start),
        )?;

        Ok(Self {
            spec,
            video,
            audio,
            picture,
            sound,
            beeps,
            captions,
            muxer,
            unix_start,
            started: Instant::now(),
            frame: 0,
            samples_written: 0,
            samples_per_frame,
        })
    }

    /// Whatever the container writes before any media.
    ///
    /// MPEG-TS produces nothing here, since its PAT and PMT tables are written
    /// alongside the media rather than up front. The call is kept because it
    /// belongs in the sequence, and a format with a real header would need it.
    pub fn header(&mut self) -> Result<Vec<u8>, MediaError> {
        self.muxer.take_output()
    }

    /// How long to wait before producing the next frame.
    ///
    /// Computed against the stream's start rather than the previous frame, so a
    /// slow encode is absorbed instead of pushing every later frame back.
    pub fn wait_before_next(&self) -> Duration {
        let due = Duration::from_secs_f64(self.frame as f64 / f64::from(self.spec.fps.max(1)));
        due.saturating_sub(self.started.elapsed())
    }

    /// Produce the next frame and return whatever bytes that yielded.
    ///
    /// A frame does not always produce output, since encoders buffer, so an
    /// empty result is normal rather than an end of stream.
    pub fn next_chunk(&mut self) -> Result<Vec<u8>, MediaError> {
        let video_time = self.frame as f64 / f64::from(self.spec.fps.max(1));

        while (self.samples_written as f64) / f64::from(self.spec.sample_rate) < video_time {
            self.beeps.fill(
                &mut self.sound,
                self.samples_written as usize,
                self.samples_per_frame as usize,
                self.spec.channels as usize,
            )?;
            self.sound.set_pts(self.samples_written);
            context(
                "encoding live audio",
                self.audio.send_frame(Some(&self.sound)),
            )?;
            self.muxer
                .drain(&mut self.audio, 1, self.spec.audio_time_base())?;
            self.samples_written += i64::from(self.samples_per_frame);
        }

        let frame_first_sample = (self.frame as i64 * i64::from(self.spec.sample_rate)
            / i64::from(self.spec.fps.max(1))) as usize;

        paint_pattern(
            &mut self.picture,
            self.spec.width as usize,
            self.spec.height as usize,
            (self.frame * 2) as usize,
            self.beeps.beeping_at(frame_first_sample),
        )?;

        let reading = clock::reading(self.unix_start, self.frame, self.spec.fps);
        let (canvas, x, y) = clock::render(&reading, self.spec.width, self.spec.height);
        overlay::draw_canvas(
            &mut self.picture,
            &canvas,
            x,
            y,
            self.spec.width as usize,
            self.spec.height as usize,
        )?;

        // Captions ride in the video's own SEI, so a live stream needs no extra
        // track for them, which is how live IPTV carries captions too.
        ffi::clear_captions(&mut self.picture);
        let cc_data = context("encoding a live caption", self.captions.cc_data(self.frame))?;
        if !cc_data.is_empty() {
            ffi::attach_captions(&mut self.picture, &cc_data)?;
        }

        self.picture.set_pts(self.frame as i64);
        context(
            "encoding live video",
            self.video.send_frame(Some(&self.picture)),
        )?;
        self.muxer
            .drain(&mut self.video, 0, self.spec.video_time_base())?;

        self.frame += 1;
        self.muxer.take_output()
    }
}

/// Where a live stream's packets go.
///
/// Either into memory for one HTTP response, or onto disk as HLS segments for
/// many viewers. MPEG-TS never seeks, which is what makes the memory case
/// possible at all: a format that rewrites its header on close, such as plain
/// MP4, would have to buffer the whole stream, which is the opposite of live.
struct LiveMuxer {
    output: AVFormatContextOutput,
    /// Filled by the write callback when muxing into memory. Absent for HLS,
    /// where the muxer writes files itself.
    sink: Option<Arc<Mutex<Vec<u8>>>>,
    /// Splices the caption descriptor into the PMT of the outgoing bytes, when
    /// the spec asks for it. Only the memory TS path can carry one, since HLS
    /// writes its own segment files.
    announcer: Option<PmtAnnouncer>,
    video_stream_tb: sys::AVRational,
    audio_stream_tb: sys::AVRational,
}

impl LiveMuxer {
    /// Segment onto disk, letting the HLS muxer manage the rolling playlist.
    fn hls(
        spec: &ClipSpec,
        video: &AVCodecContext,
        audio: &AVCodecContext,
        playlist: &CStr,
        options: &HlsOptions,
    ) -> Result<Self, MediaError> {
        // Segments live beside the playlist, which ffmpeg needs told explicitly.
        let directory = Path::new(playlist.to_str().unwrap_or("."))
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let mut settings = encode::hls_settings(options, &spec.subtitles, &directory)?;

        let mut output = context(
            "creating the live hls muxer",
            AVFormatContextOutput::builder()
                .format_name(c"hls")
                .filename(playlist)
                .build(),
        )?;

        encode::add_av_streams(&mut output, spec, video, audio);
        context(
            "writing the live hls header",
            output.write_header(&mut settings),
        )?;

        if settings.is_some() {
            return Err(MediaError::Ffmpeg {
                doing: "configuring live hls",
                detail: "one or more options were not recognised".to_string(),
            });
        }

        let video_stream_tb = output.streams()[0].time_base;
        let audio_stream_tb = output.streams()[1].time_base;

        Ok(Self {
            output,
            sink: None,
            announcer: None,
            video_stream_tb,
            audio_stream_tb,
        })
    }

    fn memory_ts(
        spec: &ClipSpec,
        video: &AVCodecContext,
        audio: &AVCodecContext,
    ) -> Result<Self, MediaError> {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&sink);

        let io = AVIOContextCustom::alloc_context(
            AVMem::new(BUFFER_BYTES),
            true,
            Vec::new(),
            None,
            Some(Box::new(move |_: &mut Vec<u8>, bytes: &[u8]| {
                match writer.lock() {
                    Ok(mut sink) => {
                        sink.extend_from_slice(bytes);
                        bytes.len() as i32
                    }
                    // A poisoned lock means the reader panicked, so there is
                    // nobody left to hand bytes to.
                    Err(_) => -1,
                }
            })),
            // No seek callback, since MPEG-TS is written straight through.
            None,
        );

        let mut output = context(
            "creating the live muxer",
            AVFormatContextOutput::builder()
                .format_name(c"mpegts")
                .io_context(AVIOContextContainer::Custom(io))
                .build(),
        )?;

        encode::add_av_streams(&mut output, spec, video, audio);

        context("writing the live header", output.write_header(&mut None))?;
        // Otherwise the header sits in ffmpeg's buffer until enough media
        // accumulates to fill it, and a viewer waits for a stream that has
        // technically already started.
        ffi::flush_output(&mut output);

        let video_stream_tb = output.streams()[0].time_base;
        let audio_stream_tb = output.streams()[1].time_base;

        // Announce the SEI captions in the PMT only when asked and only when
        // there is something to announce.
        let announcer = spec
            .announce_captions_in_pmt
            .then(|| spec.caption_services())
            .filter(|services| !services.is_empty())
            .map(|services| PmtAnnouncer::new(super::pmt::caption_service_descriptor(&services)));

        Ok(Self {
            output,
            sink: Some(sink),
            announcer,
            video_stream_tb,
            audio_stream_tb,
        })
    }

    fn drain(
        &mut self,
        encoder: &mut AVCodecContext,
        stream_index: i32,
        encoder_tb: sys::AVRational,
    ) -> Result<(), MediaError> {
        let stream_tb = if stream_index == 0 {
            self.video_stream_tb
        } else {
            self.audio_stream_tb
        };

        encode::drain(
            encoder,
            &mut self.output,
            stream_index,
            encoder_tb,
            stream_tb,
        )?;

        // Flush per drain rather than per buffer, since holding packets back to
        // fill a buffer is latency a live viewer feels directly.
        ffi::flush_output(&mut self.output);

        Ok(())
    }

    /// Hand over whatever has been written since the last call.
    ///
    /// Empty for HLS, where the muxer writes its own files and there is nothing
    /// for a caller to forward.
    fn take_output(&mut self) -> Result<Vec<u8>, MediaError> {
        let Some(sink) = &self.sink else {
            return Ok(Vec::new());
        };

        let bytes = match sink.lock() {
            Ok(mut sink) => std::mem::take(&mut *sink),
            Err(_) => {
                return Err(MediaError::Ffmpeg {
                    doing: "reading live output",
                    detail: "the output buffer was poisoned".to_string(),
                });
            }
        };

        // Splice the caption descriptor into any PMT the bytes carry, holding a
        // trailing partial packet until the rest arrives.
        Ok(match &mut self.announcer {
            Some(announcer) => announcer.feed(&bytes),
            None => bytes,
        })
    }
}

/// The muxer's own scratch buffer. Small enough that packets reach a viewer
/// promptly rather than sitting in ffmpeg waiting for the buffer to fill.
const BUFFER_BYTES: usize = 4096;
