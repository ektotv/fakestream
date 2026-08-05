//! The encoder-to-muxer pipeline, shared by the VOD writer and the live stream.
//!
//! Both open an libx264 video encoder and an aac audio encoder, add those two
//! streams to a muxer, and drain encoded packets out with the timestamps
//! rescaled to the stream's time base. The only differences are policy, a live
//! stream holds no B-frames and never emits a global header, so those become
//! flags rather than a second copy of the pipeline.

use super::hls::HlsOptions;
use super::mux::ClipSpec;
use super::subtitles::SubtitleTrack;
use super::{MediaError, context, ffi};
use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVFormatContextOutput;
use rsmpeg::avutil::{AVChannelLayout, AVDictionary, AVFrame};
use rsmpeg::ffi as sys;
use std::ffi::CString;
use std::path::Path;

/// How an encoder is tuned for its delivery.
#[derive(Clone, Copy)]
pub(crate) struct EncoderPolicy {
    /// The container keeps codec configuration in its header, so the encoder
    /// must emit a global header rather than in-band extradata. True for MP4
    /// and fragmented outputs, false for MPEG-TS and Matroska.
    pub global_header: bool,
    /// Hold no B-frames, trading compression for latency. Set for a live
    /// stream, where a held-back frame is delay a viewer feels; left off for
    /// VOD, which encodes at the codec's default.
    pub cap_b_frames: bool,
}

/// Open the libx264 video encoder for a clip.
pub(crate) fn open_video_encoder(
    spec: &ClipSpec,
    policy: EncoderPolicy,
) -> Result<AVCodecContext, MediaError> {
    let codec =
        AVCodec::find_encoder_by_name(c"libx264").ok_or(MediaError::MissingCodec("libx264"))?;
    let mut encoder = AVCodecContext::new(&codec);
    encoder.set_width(spec.width);
    encoder.set_height(spec.height);
    encoder.set_pix_fmt(sys::AV_PIX_FMT_YUV420P);
    encoder.set_time_base(spec.video_time_base().raw());
    encoder.set_framerate(sys::AVRational {
        num: spec.fps,
        den: 1,
    });
    encoder.set_bit_rate(spec.video_bitrate);
    // A player joining or recovering waits at most this long for a picture, and
    // for segmented delivery it also defines the segment boundary.
    encoder.set_gop_size(spec.keyframe_interval());
    if policy.cap_b_frames {
        encoder.set_max_b_frames(0);
    }
    if policy.global_header {
        encoder.set_flags(encoder.flags | sys::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    }
    context("opening libx264", encoder.open(None))?;
    Ok(encoder)
}

/// Open the aac audio encoder for a clip.
pub(crate) fn open_audio_encoder(
    spec: &ClipSpec,
    policy: EncoderPolicy,
) -> Result<AVCodecContext, MediaError> {
    let codec = AVCodec::find_encoder_by_name(c"aac").ok_or(MediaError::MissingCodec("aac"))?;
    let mut encoder = AVCodecContext::new(&codec);
    encoder.set_sample_rate(spec.sample_rate);
    encoder.set_ch_layout(AVChannelLayout::from_nb_channels(spec.channels as i32).into_inner());
    encoder.set_sample_fmt(sys::AV_SAMPLE_FMT_FLTP);
    encoder.set_time_base(spec.audio_time_base().raw());
    if policy.global_header {
        encoder.set_flags(encoder.flags | sys::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    }
    context("opening aac", encoder.open(None))?;
    Ok(encoder)
}

/// Add the video stream then the audio stream to a muxer, in that order, so
/// video is stream 0 and audio is stream 1.
pub(crate) fn add_av_streams(
    output: &mut AVFormatContextOutput,
    spec: &ClipSpec,
    video: &AVCodecContext,
    audio: &AVCodecContext,
) {
    {
        let mut stream = output.new_stream();
        stream.set_codecpar(video.extract_codecpar());
        stream.set_time_base(spec.video_time_base().raw());
    }
    {
        let mut stream = output.new_stream();
        stream.set_codecpar(audio.extract_codecpar());
        stream.set_time_base(spec.audio_time_base().raw());
    }
}

/// Pull every ready packet out of an encoder and write it, rescaling its
/// timestamps from the encoder's time base to the stream's.
pub(crate) fn drain(
    encoder: &mut AVCodecContext,
    output: &mut AVFormatContextOutput,
    stream_index: i32,
    encoder_tb: ffi::TimeBase,
    stream_tb: ffi::TimeBase,
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

/// Turn HLS muxer options into a dictionary the muxer consumes at
/// `write_header`. None when there are no options.
pub(crate) fn hls_settings(
    options: &HlsOptions,
    subtitles: &[SubtitleTrack],
    directory: &Path,
) -> Result<Option<AVDictionary>, MediaError> {
    let mut settings: Option<AVDictionary> = None;
    for (key, value) in options.as_pairs(subtitles, directory) {
        let key = CString::new(key)
            .map_err(|_| MediaError::Ffi(ffi::FfiError::Shape("bad option name")))?;
        let value = CString::new(value)
            .map_err(|_| MediaError::Ffi(ffi::FfiError::Shape("bad option value")))?;
        settings = Some(match settings {
            Some(existing) => existing.set(&key, &value, 0),
            None => AVDictionary::new(&key, &value, 0),
        });
    }
    Ok(settings)
}

/// Allocate the reusable YUV420P picture a clip's frames are painted into.
pub(crate) fn new_video_frame(spec: &ClipSpec) -> Result<AVFrame, MediaError> {
    let mut frame = AVFrame::new();
    frame.set_width(spec.width);
    frame.set_height(spec.height);
    frame.set_format(sys::AV_PIX_FMT_YUV420P);
    context("allocating video frame", frame.alloc_buffer())?;
    Ok(frame)
}

/// Allocate the reusable planar-float audio frame a clip's samples fill.
pub(crate) fn new_audio_frame(spec: &ClipSpec, samples: i32) -> Result<AVFrame, MediaError> {
    let mut frame = AVFrame::new();
    frame.set_nb_samples(samples);
    frame.set_ch_layout(AVChannelLayout::from_nb_channels(spec.channels as i32).into_inner());
    frame.set_format(sys::AV_SAMPLE_FMT_FLTP);
    frame.set_sample_rate(spec.sample_rate);
    context("allocating audio frame", frame.alloc_buffer())?;
    Ok(frame)
}
