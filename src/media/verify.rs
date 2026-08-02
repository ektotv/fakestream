//! Reading generated files back, so tests check the artefact rather than
//! trusting the code that wrote it.

use super::{MediaError, context};
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
        .into_iter()
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
