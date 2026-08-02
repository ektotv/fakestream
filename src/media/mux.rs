//! Writing a clip. Encoders in, container out.

use super::{MediaError, context, ffi, source::Beeps, source::paint_pattern};
use crate::captions::cea608::{self, ChannelCues};
use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVFormatContextOutput;
use rsmpeg::avutil::{AVChannelLayout, AVFrame};
use rsmpeg::ffi as sys;
use std::ffi::CStr;

/// What to generate. Everything is synthetic, so this is the whole input.
#[derive(Debug, Clone)]
pub struct ClipSpec {
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub duration_seconds: f64,
    pub video_bitrate: i64,
    pub sample_rate: i32,
    pub channels: u32,
    /// Captions carried in the video's SEI, one entry per caption channel.
    /// Empty means none.
    pub cea608: Vec<ChannelCues>,
}

impl Default for ClipSpec {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 25,
            duration_seconds: 10.0,
            video_bitrate: 2_000_000,
            sample_rate: 48_000,
            channels: 2,
            cea608: Vec::new(),
        }
    }
}

impl ClipSpec {
    fn total_video_frames(&self) -> i64 {
        (self.duration_seconds * f64::from(self.fps)).round() as i64
    }

    fn video_time_base(&self) -> sys::AVRational {
        sys::AVRational {
            num: 1,
            den: self.fps,
        }
    }

    fn audio_time_base(&self) -> sys::AVRational {
        sys::AVRational {
            num: 1,
            den: self.sample_rate,
        }
    }
}

/// Generate a clip and write it to `path`. The container is inferred from the
/// file extension, which is how libavformat picks a muxer.
pub fn write_clip(path: &CStr, spec: &ClipSpec) -> Result<(), MediaError> {
    let mut output = context("creating output", AVFormatContextOutput::create(path))?;

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

    context("writing header", output.write_header(&mut None))?;

    // write_header is free to replace the time bases just set, and MPEG-TS
    // always does, forcing 90kHz. Read back what the muxer settled on.
    let video_stream_tb = output.streams()[0].time_base;
    let audio_stream_tb = output.streams()[1].time_base;

    let mut picture = new_video_frame(spec)?;
    let samples_per_frame = audio.frame_size.max(1024);
    let mut sound = new_audio_frame(spec, samples_per_frame)?;
    let beeps = Beeps::every_second(spec.sample_rate as u32);

    let total_frames = spec.total_video_frames();
    let mut samples_written: i64 = 0;

    let captions = context(
        "scheduling captions",
        cea608::schedule(&spec.cea608, spec.fps, total_frames),
    )?;

    for index in 0..total_frames {
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
        if !spec.cea608.is_empty() {
            ffi::attach_captions(&mut picture, &captions.at(index as usize))?;
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
    encoder.set_gop_size(spec.fps);
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
