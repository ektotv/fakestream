//! Writing a clip. Encoders in, container out.

use super::hls::HlsOptions;
use super::subtitles::SubtitleTrack;
use super::{MediaError, context, encode, ffi, source::Beeps, source::paint_pattern};
use crate::captions::ass;
use crate::captions::dvb::{self, Layout};
use crate::captions::feed::{CaptionFeed, CaptionPlan};
use crate::captions::script::Cue;
use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVFormatContextOutput;
use rsmpeg::avutil::AVDictionary;
use rsmpeg::ffi as sys;
use std::ffi::{CStr, CString};
use std::path::Path;

/// What to generate. Everything is synthetic, so this is the whole input.
#[derive(Debug, Clone)]
pub struct ClipSpec {
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub duration_seconds: f64,
    pub video_bitrate: i64,
    /// How often a keyframe is emitted, in seconds.
    ///
    /// For segmented delivery this should match the segment length, so each
    /// segment is exactly one group of pictures beginning with a keyframe. A
    /// segment that starts mid-group only decodes because the previous segment
    /// happened to be fetched, which is what makes players stutter at
    /// boundaries.
    pub keyframe_seconds: f64,
    pub sample_rate: i32,
    pub channels: u32,
    /// Captions carried in the video's SEI. One plan says what to carry and in
    /// which timing model, finite for VOD or rolling for live.
    pub captions: CaptionPlan,
    /// Subtitle tracks, each its own stream in the container.
    pub subtitles: Vec<SubtitleTrack>,
    /// Announce the SEI captions in the PMT with a caption_service_descriptor,
    /// once the MPEG-TS is muxed. Only meaningful for a TS container carrying
    /// CEA captions. Off by default, so the plain CEA fixtures stay unannounced.
    pub announce_captions_in_pmt: bool,
}

impl Default for ClipSpec {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 25,
            duration_seconds: 10.0,
            video_bitrate: 2_000_000,
            keyframe_seconds: 1.0,
            sample_rate: 48_000,
            channels: 2,
            captions: CaptionPlan::None,
            subtitles: Vec::new(),
            announce_captions_in_pmt: false,
        }
    }
}

/// Milliseconds, which is a natural base for cue timing and is rescaled into
/// whatever the muxer settles on.
const SUBTITLE_TIMEBASE: i32 = 1000;

impl ClipSpec {
    /// Keyframe interval in frames, which is what the encoder wants.
    pub(crate) fn keyframe_interval(&self) -> i32 {
        ((self.keyframe_seconds * f64::from(self.fps)).round() as i32).max(1)
    }

    fn total_video_frames(&self) -> i64 {
        (self.duration_seconds * f64::from(self.fps)).round() as i64
    }

    pub(crate) fn video_time_base(&self) -> ffi::TimeBase {
        ffi::TimeBase::new(1, self.fps)
    }

    pub(crate) fn audio_time_base(&self) -> ffi::TimeBase {
        ffi::TimeBase::new(1, self.sample_rate)
    }

    pub(crate) fn subtitle_time_base(&self) -> ffi::TimeBase {
        ffi::TimeBase::new(1, SUBTITLE_TIMEBASE)
    }

    /// The caption services to announce in the PMT, derived from the SEI
    /// captions this clip carries. English throughout, since that is all the
    /// synthetic cues say.
    pub(crate) fn caption_services(&self) -> Vec<super::pmt::CaptionService> {
        use super::pmt::{CaptionService, ServiceKind};

        let mut services = Vec::new();
        // The channels in use here (one and two) both ride field 1, so a single
        // line 21 field 1 service announces whichever carries text.
        if self.captions.has_608() {
            services.push(CaptionService {
                language: *b"eng",
                kind: ServiceKind::Line21 { field2: false },
                easy_reader: false,
                wide_aspect: true,
            });
        }
        if self.captions.has_708() {
            // Service 1 is the primary caption service.
            services.push(CaptionService {
                language: *b"eng",
                kind: ServiceKind::Digital { service_number: 1 },
                easy_reader: false,
                wide_aspect: true,
            });
        }
        services
    }
}

/// Generate a clip and write it to `path`. The container is inferred from the
/// file extension, which is how libavformat picks a muxer.
pub fn write_clip(path: &CStr, spec: &ClipSpec) -> Result<(), MediaError> {
    write_clip_reporting(&Target::File(path), spec, &mut |_| {})
}

/// Where a clip is written.
pub enum Target<'a> {
    /// One file, with the muxer chosen from its extension.
    File(&'a CStr),
    /// HLS, writing playlists and segments into a directory. The path names the
    /// variant playlist, and everything else lands beside it.
    Hls {
        playlist: &'a CStr,
        options: &'a HlsOptions,
    },
}

/// Open the container a clip will be written into, along with any settings the
/// muxer wants when the header is written.
///
/// Muxer private options, which is what all the HLS ones are, are consumed by
/// `write_header` rather than at allocation. Passing them earlier leaves them
/// untouched and the muxer runs on its defaults.
fn open_output(
    target: &Target,
    spec: &ClipSpec,
) -> Result<(AVFormatContextOutput, Option<AVDictionary>), MediaError> {
    match target {
        Target::File(path) => {
            let output = context("creating output", AVFormatContextOutput::create(path))?;
            Ok((output, None))
        }
        Target::Hls { playlist, options } => {
            let directory = Path::new(playlist.to_str().unwrap_or("."))
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf();

            let settings = encode::hls_settings(options, &spec.subtitles, &directory)?;

            let output = context(
                "creating hls output",
                AVFormatContextOutput::builder()
                    .format_name(c"hls")
                    .filename(playlist)
                    .build(),
            )?;

            Ok((output, settings))
        }
    }
}

/// As [`write_clip`], reporting how far through it is.
///
/// Generating a clip takes tens of seconds and used to print nothing, which is
/// indistinguishable from being stuck. The fraction runs from zero to one and
/// is reported at most once per percent, so a caller can redraw cheaply.
pub fn write_clip_reporting(
    target: &Target,
    spec: &ClipSpec,
    progress: &mut dyn FnMut(f64),
) -> Result<(), MediaError> {
    let (mut output, mut settings) = open_output(target, spec)?;

    // MP4 and other formats keep codec configuration in the container header
    // rather than in the stream, and the encoder has to know that before it is
    // opened or its extradata comes out in the wrong place.
    let global_header = output.oformat().flags & sys::AVFMT_GLOBALHEADER as i32 != 0;

    let policy = encode::EncoderPolicy {
        global_header,
        cap_b_frames: false,
    };
    let mut video = encode::open_video_encoder(spec, policy)?;
    let mut audio = encode::open_audio_encoder(spec, policy)?;

    encode::add_av_streams(&mut output, spec, &video, &audio);

    let mut subtitle_encoders = Vec::with_capacity(spec.subtitles.len());
    for track in &spec.subtitles {
        let encoder = open_subtitle_encoder(spec, track)?;
        let mut stream = output.new_stream();
        stream.set_codecpar(encoder.extract_codecpar());
        stream.set_time_base(spec.subtitle_time_base().raw());
        stream.set_metadata(Some(track_metadata(track)?));
        subtitle_encoders.push(encoder);
    }

    context("writing header", output.write_header(&mut settings))?;

    // Options the muxer did not recognise are left behind rather than
    // reported, so an unknown or misspelled one would silently do nothing and
    // the fixture would look right while being wrong.
    if settings.is_some() {
        return Err(MediaError::Ffmpeg {
            doing: "configuring the muxer",
            detail: "one or more options were not recognised".to_string(),
        });
    }

    // write_header is free to replace the time bases just set, and MPEG-TS
    // always does, forcing 90kHz. Read back what the muxer settled on.
    let video_stream_tb = ffi::TimeBase::from_raw(output.streams()[0].time_base);
    let audio_stream_tb = ffi::TimeBase::from_raw(output.streams()[1].time_base);
    let subtitle_stream_tbs: Vec<ffi::TimeBase> = (0..spec.subtitles.len())
        .map(|index| {
            ffi::TimeBase::from_raw(output.streams()[FIRST_SUBTITLE_STREAM + index].time_base)
        })
        .collect();

    let mut picture = encode::new_video_frame(spec)?;
    let samples_per_frame = audio.frame_size.max(1024);
    let mut sound = encode::new_audio_frame(spec, samples_per_frame)?;
    let beeps = Beeps::every_second(spec.sample_rate as u32);

    let total_frames = spec.total_video_frames();
    let mut samples_written: i64 = 0;
    let mut subtitle_events = subtitle_events(spec);
    subtitle_events.reverse();

    let mut caption_feed = context(
        "scheduling captions",
        CaptionFeed::build(&spec.captions, spec.fps, total_frames, 0.0),
    )?;

    let mut last_reported = -1i64;

    for index in 0..total_frames {
        // At most one report per percent, so a caller can redraw cheaply.
        let percent = index * 100 / total_frames.max(1);
        if percent != last_reported {
            last_reported = percent;
            progress(index as f64 / total_frames.max(1) as f64);
        }

        // Keep audio level with video rather than writing one stream then the
        // other, so the muxer never has to buffer a whole track.
        let video_time = index as f64 / f64::from(spec.fps);
        while (samples_written as f64) / f64::from(spec.sample_rate) < video_time {
            beeps.fill(
                &mut sound,
                samples_written as usize,
                samples_per_frame as usize,
                spec.channels as usize,
            )?;
            sound.set_pts(samples_written);
            context("encoding audio", audio.send_frame(Some(&sound)))?;
            encode::drain(
                &mut audio,
                &mut output,
                1,
                spec.audio_time_base(),
                audio_stream_tb,
            )?;
            samples_written += i64::from(samples_per_frame);
        }

        // Subtitle packets go in as their moment arrives, so interleaving
        // stays monotonic and the muxer never buffers a whole track.
        while subtitle_events
            .last()
            .is_some_and(|event| event.at_seconds <= video_time)
        {
            let event = subtitle_events.pop().expect("checked by the guard above");
            let encoder = &mut subtitle_encoders[event.track];
            let stream_tb = subtitle_stream_tbs[event.track];
            write_subtitle(&mut output, encoder, spec, &event, stream_tb)?;
        }

        // The marker shows on the frames a beep is sounding, which makes audio
        // and video sync checkable by eye and ear at the same moment.
        let frame_first_sample =
            (index * i64::from(spec.sample_rate) / i64::from(spec.fps)) as usize;
        paint_pattern(
            &mut picture,
            spec.width as usize,
            spec.height as usize,
            (index * 2) as usize,
            beeps.beeping_at(frame_first_sample),
        )?;
        picture.set_pts(index);

        // Side data does not survive a frame being reused, but clearing is
        // still explicit, since a stale caption riding a later frame would be
        // near impossible to spot in the output.
        ffi::clear_captions(&mut picture);
        let cc_data = context("encoding a caption", caption_feed.cc_data(index as u64))?;
        if !cc_data.is_empty() {
            ffi::attach_captions(&mut picture, &cc_data)?;
        }

        context("encoding video", video.send_frame(Some(&picture)))?;
        encode::drain(
            &mut video,
            &mut output,
            0,
            spec.video_time_base(),
            video_stream_tb,
        )?;
    }

    context("flushing video", video.send_frame(None))?;
    encode::drain(
        &mut video,
        &mut output,
        0,
        spec.video_time_base(),
        video_stream_tb,
    )?;
    context("flushing audio", audio.send_frame(None))?;
    encode::drain(
        &mut audio,
        &mut output,
        1,
        spec.audio_time_base(),
        audio_stream_tb,
    )?;

    context("writing trailer", output.write_trailer())?;
    progress(1.0);
    Ok(())
}

/// One thing to put on a subtitle stream: a caption appearing, or the empty
/// event that removes it.
struct SubtitleEvent {
    /// Which track in the spec this belongs to.
    track: usize,
    at_seconds: f64,
    /// How long the packet claims to last, which is what a player uses. None
    /// for a clear.
    duration_seconds: Option<f64>,
    cue: Option<Cue>,
    /// Position within its own track, which ASS carries as read order.
    read_order: usize,
}

/// Flatten every track's cues into one ordered event list.
///
/// Bitmap formats get an explicit removal event, because a DVB decoder
/// otherwise leaves the caption up until its own page timeout, which ffmpeg
/// hard codes to thirty seconds. Text formats carry their duration on the
/// packet, so they need no such thing.
fn subtitle_events(spec: &ClipSpec) -> Vec<SubtitleEvent> {
    let mut events = Vec::new();

    for (track, subtitle) in spec.subtitles.iter().enumerate() {
        for (read_order, cue) in subtitle.cues.iter().enumerate() {
            events.push(SubtitleEvent {
                track,
                at_seconds: cue.start,
                duration_seconds: Some(cue.duration),
                cue: Some(cue.clone()),
                read_order,
            });

            if subtitle.format.is_bitmap() {
                events.push(SubtitleEvent {
                    track,
                    at_seconds: cue.start + cue.duration,
                    duration_seconds: None,
                    cue: None,
                    read_order,
                });
            }
        }
    }

    events.sort_by(|left, right| {
        left.at_seconds
            .partial_cmp(&right.at_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    events
}

/// Language and title, which is how a player labels a track and how a stored
/// language preference gets matched.
fn track_metadata(track: &SubtitleTrack) -> Result<AVDictionary, MediaError> {
    let language = CString::new(track.language.as_str())
        .map_err(|_| MediaError::Ffi(ffi::FfiError::Shape("language contains a null byte")))?;
    let title = CString::new(track.title.as_str())
        .map_err(|_| MediaError::Ffi(ffi::FfiError::Shape("title contains a null byte")))?;

    let dictionary = AVDictionary::new(c"language", &language, 0).set(c"title", &title, 0);
    Ok(dictionary)
}

fn open_subtitle_encoder(
    spec: &ClipSpec,
    track: &SubtitleTrack,
) -> Result<AVCodecContext, MediaError> {
    let name = track.format.encoder_name();
    let codec = AVCodec::find_encoder_by_name(name)
        .ok_or(MediaError::MissingCodec("a subtitle encoder"))?;

    let mut encoder = AVCodecContext::new(&codec);
    // The display a subtitle positions itself against, which is the video.
    encoder.set_width(spec.width);
    encoder.set_height(spec.height);
    encoder.set_time_base(spec.subtitle_time_base().raw());

    if track.format.needs_ass_header() {
        let header = ass::header();
        ffi::set_subtitle_header(&mut encoder, &header)?;
    }

    context("opening a subtitle encoder", encoder.open(None))?;
    Ok(encoder)
}

/// Encode one event and write it to its stream.
fn write_subtitle(
    output: &mut AVFormatContextOutput,
    encoder: &mut AVCodecContext,
    spec: &ClipSpec,
    event: &SubtitleEvent,
    stream_tb: ffi::TimeBase,
) -> Result<(), MediaError> {
    let track = &spec.subtitles[event.track];
    let duration_ms = (event.duration_seconds.unwrap_or(0.0) * 1000.0) as u32;

    let subtitle = match (&event.cue, track.format.is_bitmap()) {
        (Some(cue), true) => {
            let layout = Layout::for_display(spec.width, spec.height);
            let rendered = dvb::render(cue, &layout);
            ffi::bitmap_subtitle(
                rendered.x,
                rendered.y,
                rendered.canvas.width,
                rendered.canvas.height,
                &rendered.canvas.pixels,
                &crate::captions::text::palette(),
                duration_ms,
            )?
        }
        (Some(cue), false) => {
            ffi::text_subtitle(&ass::dialogue(cue, event.read_order), duration_ms)?
        }
        (None, _) => ffi::empty_subtitle(),
    };

    // Generous, since a caption's encoded size grows with its bitmap and a
    // short buffer would truncate rather than fail loudly.
    let mut buffer = vec![0u8; 256 * 1024];
    let used = ffi::encode_subtitle(encoder, &subtitle, &mut buffer)?;
    if used == 0 {
        return Ok(());
    }

    let mut packet = ffi::from_bytes(&buffer[..used])?;
    let pts = (event.at_seconds * f64::from(SUBTITLE_TIMEBASE)) as i64;
    let duration = (event.duration_seconds.unwrap_or(0.0) * f64::from(SUBTITLE_TIMEBASE)) as i64;
    let stream_index = (FIRST_SUBTITLE_STREAM + event.track) as i32;

    ffi::stamp(&mut packet, stream_index, pts, duration);
    ffi::rescale(&mut packet, spec.subtitle_time_base(), stream_tb);

    context(
        "writing subtitle packet",
        output.interleaved_write_frame(&mut packet),
    )?;
    Ok(())
}

/// Video is stream zero, audio one, so subtitle tracks follow.
const FIRST_SUBTITLE_STREAM: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keyframe_interval_is_expressed_in_frames() {
        let spec = ClipSpec {
            fps: 25,
            keyframe_seconds: 2.0,
            ..ClipSpec::default()
        };
        assert_eq!(spec.keyframe_interval(), 50);
    }

    #[test]
    fn a_keyframe_interval_is_never_zero() {
        // Zero would tell the encoder to emit only one keyframe ever, and a
        // segmented stream would be undecodable from any point but the start.
        let spec = ClipSpec {
            fps: 25,
            keyframe_seconds: 0.0,
            ..ClipSpec::default()
        };
        assert_eq!(spec.keyframe_interval(), 1);
    }
}
