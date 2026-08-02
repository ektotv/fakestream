//! Reading generated files back, so tests check the artefact rather than
//! trusting the code that wrote it.

use super::{MediaError, context, ffi};
use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::ffi as sys;
use std::ffi::CStr;

/// What a stream turned out to be, once decoded from the file.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamSummary {
    pub kind: StreamKind,
    pub codec: String,
    pub width: i32,
    pub height: i32,
    pub sample_rate: i32,
    pub channels: i32,
    /// Seconds, taken from the stream's own duration in its time base.
    pub duration: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
    Other,
}

impl From<sys::AVMediaType> for StreamKind {
    fn from(value: sys::AVMediaType) -> Self {
        match value {
            sys::AVMEDIA_TYPE_VIDEO => Self::Video,
            sys::AVMEDIA_TYPE_AUDIO => Self::Audio,
            sys::AVMEDIA_TYPE_SUBTITLE => Self::Subtitle,
            _ => Self::Other,
        }
    }
}

/// Open a generated file and describe every stream in it.
pub fn inspect(path: &CStr) -> Result<Vec<StreamSummary>, MediaError> {
    let input = context("opening generated file", AVFormatContextInput::open(path))?;

    let summaries = input
        .streams()
        .iter()
        .map(|stream| {
            let parameters = stream.codecpar();
            let time_base = stream.time_base;
            let duration = if stream.duration > 0 && time_base.den != 0 {
                stream.duration as f64 * f64::from(time_base.num) / f64::from(time_base.den)
            } else {
                0.0
            };

            StreamSummary {
                kind: StreamKind::from(parameters.codec_type),
                codec: codec_name(parameters.codec_id),
                width: parameters.width,
                height: parameters.height,
                sample_rate: parameters.sample_rate,
                channels: parameters.ch_layout.nb_channels,
                duration,
            }
        })
        .collect();

    Ok(summaries)
}

fn codec_name(id: sys::AVCodecID) -> String {
    // SAFETY: avcodec_get_name always returns a valid static C string, even for
    // an unknown id, where it yields "none".
    let name = unsafe { std::ffi::CStr::from_ptr(sys::avcodec_get_name(id)) };
    name.to_string_lossy().into_owned()
}

/// One decoded subtitle event, for checking that captions both appear and go
/// away again.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleEvent {
    /// Seconds into the clip.
    pub at: f64,
    /// Regions carried. Zero means the event clears the screen.
    pub rects: usize,
    /// Text carried, empty for bitmap formats and for clears.
    pub text: Vec<String>,
}

/// Decode one of a file's subtitle streams and report every event in it.
///
/// `track` counts subtitle streams rather than all streams, so zero is the
/// first subtitle track whatever its stream index.
///
/// Needed because a bitmap subtitle stream cannot be checked by reading text
/// out of it, and because an empty event, the thing that removes a caption, is
/// invisible to most tooling.
pub fn subtitle_events(path: &CStr, track: usize) -> Result<Vec<SubtitleEvent>, MediaError> {
    let mut input = context("opening generated file", AVFormatContextInput::open(path))?;

    let Some(index) = input
        .streams()
        .iter()
        .enumerate()
        .filter(|(_, stream)| stream.codecpar().codec_type == sys::AVMEDIA_TYPE_SUBTITLE)
        .map(|(position, _)| position)
        .nth(track)
    else {
        return Ok(Vec::new());
    };

    let (parameters, time_base) = {
        let stream = &input.streams()[index];
        (stream.codecpar().clone(), stream.time_base)
    };
    let codec = AVCodec::find_decoder(parameters.codec_id)
        .ok_or(MediaError::MissingCodec("subtitle decoder"))?;

    let mut decoder = AVCodecContext::new(&codec);
    context(
        "applying subtitle parameters",
        decoder.apply_codecpar(&parameters),
    )?;
    context("opening subtitle decoder", decoder.open(None))?;

    let mut events = Vec::new();
    while let Some(mut packet) = context("reading packet", input.read_packet())? {
        if packet.stream_index as usize != index {
            continue;
        }

        let pts = packet.pts;
        if let Ok(Some(subtitle)) = decoder.decode_subtitle(Some(&mut packet)) {
            events.push(SubtitleEvent {
                at: pts as f64 * f64::from(time_base.num) / f64::from(time_base.den),
                rects: ffi::rect_geometry(&subtitle).len(),
                text: ffi::rect_text(&subtitle),
            });
        }
    }

    Ok(events)
}
