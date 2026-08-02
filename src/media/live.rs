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
use super::{MediaError, clock, context, ffi, overlay, source::Beeps, source::paint_pattern};
use crate::captions::libcaption::Channel;
use crate::captions::rolling::RollingCaptions;
use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVIOContextContainer;
use rsmpeg::avformat::{AVFormatContextOutput, AVIOContextCustom};
use rsmpeg::avutil::{AVChannelLayout, AVDictionary, AVFrame, AVMem};
use rsmpeg::ffi as sys;
use std::ffi::{CStr, CString};
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
    captions: RollingCaptions,
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

        let video = open_video_encoder(&spec)?;
        let audio = open_audio_encoder(&spec)?;
        let muxer = make_muxer(&spec, &video, &audio)?;

        let mut picture = AVFrame::new();
        picture.set_width(spec.width);
        picture.set_height(spec.height);
        picture.set_format(sys::AV_PIX_FMT_YUV420P);
        context("allocating a live video frame", picture.alloc_buffer())?;

        let samples_per_frame = audio.frame_size.max(1024);
        let mut sound = AVFrame::new();
        sound.set_nb_samples(samples_per_frame);
        sound.set_ch_layout(AVChannelLayout::from_nb_channels(spec.channels as i32).into_inner());
        sound.set_format(sys::AV_SAMPLE_FMT_FLTP);
        sound.set_sample_rate(spec.sample_rate);
        context("allocating a live audio frame", sound.alloc_buffer())?;

        let beeps = Beeps::every_second(spec.sample_rate as u32);
        let captions = RollingCaptions::new(
            spec.fps,
            CAPTION_INTERVAL_SECONDS,
            CAPTION_VISIBLE_SECONDS,
            Channel::One,
        );

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
        let triplet = self
            .captions
            .triplet_for(self.frame, self.unix_start)
            .map_err(|error| MediaError::Ffmpeg {
                doing: "encoding a live caption",
                detail: error.to_string(),
            })?;
        ffi::attach_captions(&mut self.picture, &triplet)?;

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

fn open_video_encoder(spec: &ClipSpec) -> Result<AVCodecContext, MediaError> {
    let codec =
        AVCodec::find_encoder_by_name(c"libx264").ok_or(MediaError::MissingCodec("libx264"))?;
    let mut encoder = AVCodecContext::new(&codec);
    encoder.set_width(spec.width);
    encoder.set_height(spec.height);
    encoder.set_pix_fmt(sys::AV_PIX_FMT_YUV420P);
    encoder.set_time_base(spec.video_time_base());
    encoder.set_framerate(sys::AVRational {
        num: spec.fps,
        den: 1,
    });
    encoder.set_bit_rate(spec.video_bitrate);
    // A player joining or recovering waits at most this long for a picture,
    // and for segmented delivery it also defines the segment boundary.
    encoder.set_gop_size(spec.keyframe_interval());
    // Latency matters more than compression here. B-frames make the encoder
    // hold frames back, which shows up directly as delay on a live stream.
    encoder.set_max_b_frames(0);
    context("opening libx264 for live", encoder.open(None))?;
    Ok(encoder)
}

fn open_audio_encoder(spec: &ClipSpec) -> Result<AVCodecContext, MediaError> {
    let codec = AVCodec::find_encoder_by_name(c"aac").ok_or(MediaError::MissingCodec("aac"))?;
    let mut encoder = AVCodecContext::new(&codec);
    encoder.set_sample_rate(spec.sample_rate);
    encoder.set_ch_layout(AVChannelLayout::from_nb_channels(spec.channels as i32).into_inner());
    encoder.set_sample_fmt(sys::AV_SAMPLE_FMT_FLTP);
    encoder.set_time_base(spec.audio_time_base());
    context("opening aac for live", encoder.open(None))?;
    Ok(encoder)
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

        let mut settings: Option<AVDictionary> = None;
        for (key, value) in options.as_pairs(&spec.subtitles, &directory) {
            let key = CString::new(key)
                .map_err(|_| MediaError::Ffi(ffi::FfiError::Shape("bad option name")))?;
            let value = CString::new(value)
                .map_err(|_| MediaError::Ffi(ffi::FfiError::Shape("bad option value")))?;
            settings = Some(match settings {
                Some(existing) => existing.set(&key, &value, 0),
                None => AVDictionary::new(&key, &value, 0),
            });
        }

        let mut output = context(
            "creating the live hls muxer",
            AVFormatContextOutput::builder()
                .format_name(c"hls")
                .filename(playlist)
                .build(),
        )?;

        Self::add_streams(&mut output, spec, video, audio);
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
            video_stream_tb,
            audio_stream_tb,
        })
    }

    fn add_streams(
        output: &mut AVFormatContextOutput,
        spec: &ClipSpec,
        video: &AVCodecContext,
        audio: &AVCodecContext,
    ) {
        {
            let mut stream = output.new_stream();
            stream.set_codecpar(video.extract_codecpar());
            stream.set_time_base(spec.video_time_base());
        }
        {
            let mut stream = output.new_stream();
            stream.set_codecpar(audio.extract_codecpar());
            stream.set_time_base(spec.audio_time_base());
        }
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

        Self::add_streams(&mut output, spec, video, audio);

        context("writing the live header", output.write_header(&mut None))?;
        // Otherwise the header sits in ffmpeg's buffer until enough media
        // accumulates to fill it, and a viewer waits for a stream that has
        // technically already started.
        ffi::flush_output(&mut output);

        let video_stream_tb = output.streams()[0].time_base;
        let audio_stream_tb = output.streams()[1].time_base;

        Ok(Self {
            output,
            sink: Some(sink),
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

        while let Ok(mut packet) = encoder.receive_packet() {
            ffi::route(&mut packet, stream_index);
            ffi::rescale(&mut packet, encoder_tb, stream_tb);
            context(
                "writing a live packet",
                self.output.interleaved_write_frame(&mut packet),
            )?;
        }

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

        match sink.lock() {
            Ok(mut sink) => Ok(std::mem::take(&mut *sink)),
            Err(_) => Err(MediaError::Ffmpeg {
                doing: "reading live output",
                detail: "the output buffer was poisoned".to_string(),
            }),
        }
    }
}

/// Seconds between captions appearing on a live stream.
const CAPTION_INTERVAL_SECONDS: f64 = 3.0;

/// How long each caption stays up, leaving a visible gap before the next.
const CAPTION_VISIBLE_SECONDS: f64 = 2.5;

/// The muxer's own scratch buffer. Small enough that packets reach a viewer
/// promptly rather than sitting in ffmpeg waiting for the buffer to fill.
const BUFFER_BYTES: usize = 4096;
