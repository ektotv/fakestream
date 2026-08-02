//! DVB bitmap subtitle probe.
//!
//! Generates an MPEG-TS holding H.264 video and a DVB subtitle track, both made
//! from nothing. The question being answered is whether a real player would find
//! and decode the subtitle stream, so the output is checked with the system
//! ffmpeg afterwards.

mod cea608;
mod frame;
mod subtitle;
mod text;

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVFormatContextOutput;
use rsmpeg::avutil::AVFrame;
use rsmpeg::ffi;
use std::ffi::CStr;
use subtitle::BitmapCue;

const WIDTH: i32 = 720;
const HEIGHT: i32 = 576;
const FPS: i32 = 25;
const SECONDS: i32 = 6;
const SUBTITLE_TIMEBASE: i32 = 1000;

/// When each caption appears, and for how long, in seconds.
const CUES: [(f64, f64, &str); 2] = [
    (1.0, 2.5, "Lorem ipsum dolor sit amet"),
    (4.0, 1.5, "Consectetur adipiscing elit \u{2014} \u{e9}\u{e8}\u{fc}\u{df}"),
];

/// Bundled so the finished binary needs no system fonts. OFL 1.1.
const FONT: &[u8] = include_bytes!("../../../assets/fonts/NotoSans.ttf");

const FONT_SIZE: f32 = 34.0;
const BOX_PADDING: i32 = 18;

fn main() {
    if let Err(message) = run(c"dvb.ts") {
        eprintln!("failed: {message}");
        std::process::exit(1);
    }
}

fn run(path: &CStr) -> Result<(), String> {
    let mut video = open_video_encoder()?;
    let mut subtitles = open_subtitle_encoder()?;

    let mut output = AVFormatContextOutput::create(path).map_err(|e| format!("create output: {e}"))?;

    {
        let mut stream = output.new_stream();
        stream.set_codecpar(video.extract_codecpar());
        stream.set_time_base(ffi::AVRational { num: 1, den: FPS });
    }
    {
        let mut stream = output.new_stream();
        stream.set_codecpar(subtitles.extract_codecpar());
        stream.set_time_base(ffi::AVRational { num: 1, den: SUBTITLE_TIMEBASE });
    }

    output.write_header(&mut None).map_err(|e| format!("write header: {e}"))?;

    // write_header may replace the time bases we asked for. MPEG-TS always
    // does, forcing 90kHz, so read back what the muxer actually settled on.
    let video_tb = output.streams()[0].time_base;
    let subtitle_tb = output.streams()[1].time_base;

    let encoder_tb = ffi::AVRational { num: 1, den: FPS };

    let mut picture = AVFrame::new();
    picture.set_width(WIDTH);
    picture.set_height(HEIGHT);
    picture.set_format(ffi::AV_PIX_FMT_YUV420P);
    picture.alloc_buffer().map_err(|e| format!("alloc frame: {e}"))?;

    let total_frames = FPS * SECONDS;
    let mut cues_written = 0usize;

    // CEA-608 rides on the video frames themselves, one byte pair per frame, so
    // the whole timeline is planned up front.
    let caption_schedule = build_caption_schedule(total_frames);

    for index in 0..total_frames {
        let seconds = f64::from(index) / f64::from(FPS);

        // Captions go in as their moment arrives, so the interleaving stays
        // monotonic. Writing them all at the end would break the muxer.
        if let Some((start, duration, text)) = CUES.get(cues_written) {
            if seconds >= *start {
                write_caption(&mut output, &mut subtitles, *start, *duration, text, subtitle_tb)?;
                cues_written += 1;
            }
        }

        frame::paint_pattern(&mut picture, index * 2);
        picture.set_pts(i64::from(index));

        frame::clear_side_data(&mut picture);
        let triplet = cea608::triplet(caption_schedule[index as usize]);
        frame::attach_captions(&mut picture, &triplet).map_err(|e| format!("attach captions: {e}"))?;

        video.send_frame(Some(&picture)).map_err(|e| format!("send frame: {e}"))?;
        drain_video(&mut video, &mut output, encoder_tb, video_tb)?;
    }

    video.send_frame(None).map_err(|e| format!("flush encoder: {e}"))?;
    drain_video(&mut video, &mut output, encoder_tb, video_tb)?;

    output.write_trailer().map_err(|e| format!("write trailer: {e}"))?;

    println!("wrote {} with {} captions", path.to_string_lossy(), cues_written);
    Ok(())
}

/// Lay the caption byte pairs onto a frame timeline. One pair per frame is the
/// 608 field rate, so a caption takes as many frames as it has pairs.
fn build_caption_schedule(total_frames: i32) -> Vec<Option<(u8, u8)>> {
    let mut schedule: Vec<Option<(u8, u8)>> = vec![None; total_frames as usize];

    for (start, duration, text) in CUES {
        let mut cursor = (start * f64::from(FPS)) as usize;
        for pair in cea608::pop_on_caption(text) {
            if cursor >= schedule.len() {
                break;
            }
            schedule[cursor] = Some(pair);
            cursor += 1;
        }

        let clear_at = ((start + duration) * f64::from(FPS)) as usize;
        let mut cursor = clear_at;
        for pair in cea608::clear_pairs() {
            if cursor >= schedule.len() {
                break;
            }
            schedule[cursor] = Some(pair);
            cursor += 1;
        }
    }

    schedule
}

fn open_video_encoder() -> Result<AVCodecContext, String> {
    let codec = AVCodec::find_encoder_by_name(c"libx264").ok_or("libx264 encoder missing")?;
    let mut context = AVCodecContext::new(&codec);
    context.set_width(WIDTH);
    context.set_height(HEIGHT);
    context.set_pix_fmt(ffi::AV_PIX_FMT_YUV420P);
    context.set_time_base(ffi::AVRational { num: 1, den: FPS });
    context.set_framerate(ffi::AVRational { num: FPS, den: 1 });
    context.set_bit_rate(800_000);
    context.set_gop_size(FPS);
    context.open(None).map_err(|e| format!("open libx264: {e}"))?;
    Ok(context)
}

fn open_subtitle_encoder() -> Result<AVCodecContext, String> {
    let codec = AVCodec::find_encoder_by_name(c"dvbsub").ok_or("dvbsub encoder missing")?;
    let mut context = AVCodecContext::new(&codec);
    context.set_width(WIDTH);
    context.set_height(HEIGHT);
    context.set_time_base(ffi::AVRational { num: 1, den: SUBTITLE_TIMEBASE });
    context.open(None).map_err(|e| format!("open dvbsub: {e}"))?;
    Ok(context)
}

fn drain_video(
    video: &mut AVCodecContext,
    output: &mut AVFormatContextOutput,
    encoder_tb: ffi::AVRational,
    stream_tb: ffi::AVRational,
) -> Result<(), String> {
    loop {
        let mut packet = match video.receive_packet() {
            Ok(packet) => packet,
            Err(_) => return Ok(()),
        };
        subtitle::route_packet(&mut packet, 0);
        subtitle::rescale_packet(&mut packet, encoder_tb, stream_tb);
        output
            .interleaved_write_frame(&mut packet)
            .map_err(|e| format!("write video packet: {e}"))?;
    }
}

fn write_caption(
    output: &mut AVFormatContextOutput,
    encoder: &mut AVCodecContext,
    start: f64,
    duration: f64,
    text: &str,
    stream_tb: ffi::AVRational,
) -> Result<(), String> {
    // Size the box to the text rather than guessing, which is what a real
    // caption renderer does.
    let text_width = text::measure(FONT, text, FONT_SIZE).map_err(|e| format!("measure: {e:?}"))?;
    let width = (text_width.ceil() as i32 + BOX_PADDING * 2).min(WIDTH - 40);
    let height = FONT_SIZE as i32 + BOX_PADDING * 2;

    let mut canvas = text::Canvas::new(width, height);
    canvas.draw_box(2);
    text::draw_line(
        &mut canvas,
        FONT,
        text,
        FONT_SIZE,
        BOX_PADDING as f32,
        (height - BOX_PADDING) as f32,
    )
    .map_err(|e| format!("draw text: {e:?}"))?;

    let cue = BitmapCue {
        x: (WIDTH - width) / 2,
        y: HEIGHT - height - 40,
        width,
        height,
        pixels: canvas.pixels,
        palette: text::palette(),
        start_ms: 0,
        end_ms: (duration * 1000.0) as u32,
    };

    let built = cue.to_subtitle().map_err(|e| format!("build cue: {e:?}"))?;

    let mut buffer = vec![0u8; 128 * 1024];
    encoder
        .encode_subtitle(&built, &mut buffer)
        .map_err(|e| format!("encode subtitle: {e}"))?;

    let used = buffer.iter().rposition(|byte| *byte != 0).map_or(0, |i| i + 1);
    let mut packet = subtitle::packet_from_bytes(&buffer[..used]).map_err(|e| format!("packet: {e:?}"))?;

    // Prepared in milliseconds, then rescaled into whatever base the muxer
    // chose for this stream.
    let source_tb = ffi::AVRational { num: 1, den: SUBTITLE_TIMEBASE };
    let pts = (start * f64::from(SUBTITLE_TIMEBASE)) as i64;
    let length = (duration * f64::from(SUBTITLE_TIMEBASE)) as i64;
    subtitle::stamp_packet(&mut packet, 1, pts, length);
    subtitle::rescale_packet(&mut packet, source_tb, stream_tb);

    output
        .interleaved_write_frame(&mut packet)
        .map_err(|e| format!("write subtitle packet: {e}"))?;
    Ok(())
}
