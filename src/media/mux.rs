//! Writing a clip. Encoders in, container out.

use super::hls::HlsOptions;
use super::subtitles::SubtitleTrack;
use super::{MediaError, context, ffi, source::Beeps, source::paint_pattern};
use crate::captions::ass;
use crate::captions::cea608::{self, ChannelCues};
use crate::captions::cea708;
use crate::captions::dvb::{self, Layout};
use crate::captions::script::Cue;
use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVFormatContextOutput;
use rsmpeg::avutil::{AVChannelLayout, AVDictionary, AVFrame};
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
    /// Captions carried in the video's SEI, one entry per caption channel.
    /// Empty means none.
    pub cea608: Vec<ChannelCues>,
    /// CEA-708 captions, which share the same SEI as the 608 ones. Real
    /// streams commonly carry both.
    pub cea708: Vec<Cue>,
    /// Subtitle tracks, each its own stream in the container.
    pub subtitles: Vec<SubtitleTrack>,
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
            cea608: Vec::new(),
            cea708: Vec::new(),
            subtitles: Vec::new(),
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

    pub(crate) fn video_time_base(&self) -> sys::AVRational {
        sys::AVRational {
            num: 1,
            den: self.fps,
        }
    }

    pub(crate) fn audio_time_base(&self) -> sys::AVRational {
        sys::AVRational {
            num: 1,
            den: self.sample_rate,
        }
    }

    pub(crate) fn subtitle_time_base(&self) -> sys::AVRational {
        sys::AVRational {
            num: 1,
            den: SUBTITLE_TIMEBASE,
        }
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

    let mut video = open_video_encoder(spec, global_header)?;
    let mut audio = open_audio_encoder(spec, global_header)?;

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

    let mut subtitle_encoders = Vec::with_capacity(spec.subtitles.len());
    for track in &spec.subtitles {
        let encoder = open_subtitle_encoder(spec, track)?;
        let mut stream = output.new_stream();
        stream.set_codecpar(encoder.extract_codecpar());
        stream.set_time_base(spec.subtitle_time_base());
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
    let video_stream_tb = output.streams()[0].time_base;
    let audio_stream_tb = output.streams()[1].time_base;
    let subtitle_stream_tbs: Vec<sys::AVRational> = (0..spec.subtitles.len())
        .map(|index| output.streams()[FIRST_SUBTITLE_STREAM + index].time_base)
        .collect();

    let mut picture = new_video_frame(spec)?;
    let samples_per_frame = audio.frame_size.max(1024);
    let mut sound = new_audio_frame(spec, samples_per_frame)?;
    let beeps = Beeps::every_second(spec.sample_rate as u32);

    let total_frames = spec.total_video_frames();
    let mut samples_written: i64 = 0;
    let mut subtitle_events = subtitle_events(spec);
    subtitle_events.reverse();

    let captions = context(
        "scheduling captions",
        cea608::schedule(&spec.cea608, spec.fps, total_frames),
    )?;
    let dtvcc = cea708::schedule(&spec.cea708, spec.fps, total_frames);

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
            drain(
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
        // Both caption systems share one cc_data array, which is how a real
        // stream carries them, so they are gathered before being attached.
        ffi::clear_captions(&mut picture);
        let mut cc_data: Vec<u8> = Vec::new();
        if !spec.cea608.is_empty() {
            cc_data.extend_from_slice(&captions.at(index as usize));
        }
        for triplet in dtvcc.at(index as usize) {
            cc_data.extend_from_slice(triplet);
        }
        if !cc_data.is_empty() {
            ffi::attach_captions(&mut picture, &cc_data)?;
        }

        context("encoding video", video.send_frame(Some(&picture)))?;
        drain(
            &mut video,
            &mut output,
            0,
            spec.video_time_base(),
            video_stream_tb,
        )?;
    }

    context("flushing video", video.send_frame(None))?;
    drain(
        &mut video,
        &mut output,
        0,
        spec.video_time_base(),
        video_stream_tb,
    )?;
    context("flushing audio", audio.send_frame(None))?;
    drain(
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

/// Pull every packet an encoder is ready to give and write it out.
fn drain(
    encoder: &mut AVCodecContext,
    output: &mut AVFormatContextOutput,
    stream_index: i32,
    encoder_tb: sys::AVRational,
    stream_tb: sys::AVRational,
) -> Result<(), MediaError> {
    while let Ok(mut packet) = encoder.receive_packet() {
        ffi::route(&mut packet, stream_index);
        ffi::rescale(&mut packet, encoder_tb, stream_tb);
        context(
            "writing packet",
            output.interleaved_write_frame(&mut packet),
        )?;
    }
    Ok(())
}

fn open_video_encoder(spec: &ClipSpec, global_header: bool) -> Result<AVCodecContext, MediaError> {
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
    encoder.set_gop_size(spec.keyframe_interval());
    if global_header {
        encoder.set_flags(encoder.flags | sys::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    }
    context("opening libx264", encoder.open(None))?;
    Ok(encoder)
}

fn open_audio_encoder(spec: &ClipSpec, global_header: bool) -> Result<AVCodecContext, MediaError> {
    let codec = AVCodec::find_encoder_by_name(c"aac").ok_or(MediaError::MissingCodec("aac"))?;
    let mut encoder = AVCodecContext::new(&codec);
    encoder.set_sample_rate(spec.sample_rate);
    encoder.set_ch_layout(AVChannelLayout::from_nb_channels(spec.channels as i32).into_inner());
    encoder.set_sample_fmt(sys::AV_SAMPLE_FMT_FLTP);
    encoder.set_time_base(spec.audio_time_base());
    if global_header {
        encoder.set_flags(encoder.flags | sys::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    }
    context("opening aac", encoder.open(None))?;
    Ok(encoder)
}

fn new_video_frame(spec: &ClipSpec) -> Result<AVFrame, MediaError> {
    let mut frame = AVFrame::new();
    frame.set_width(spec.width);
    frame.set_height(spec.height);
    frame.set_format(sys::AV_PIX_FMT_YUV420P);
    context("allocating video frame", frame.alloc_buffer())?;
    Ok(frame)
}

fn new_audio_frame(spec: &ClipSpec, samples: i32) -> Result<AVFrame, MediaError> {
    let mut frame = AVFrame::new();
    frame.set_nb_samples(samples);
    frame.set_ch_layout(AVChannelLayout::from_nb_channels(spec.channels as i32).into_inner());
    frame.set_format(sys::AV_SAMPLE_FMT_FLTP);
    frame.set_sample_rate(spec.sample_rate);
    context("allocating audio frame", frame.alloc_buffer())?;
    Ok(frame)
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
    encoder.set_time_base(spec.subtitle_time_base());

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
    stream_tb: sys::AVRational,
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
