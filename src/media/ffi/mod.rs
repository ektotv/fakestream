//! Every unsafe operation in fakestream lives under this module.
//!
//! rsmpeg wraps most of libav* safely, but leaves gaps exactly where this tool
//! does its work. `AVSubtitle` cannot take rects, `AVPacket` cannot take data,
//! and `AVFrame` can take neither pixels nor side data. Each gap is closed here
//! behind a safe function, so no other module needs `unsafe` at all.
//!
//! The rule that keeps this sound: anything ffmpeg will later free must be
//! allocated by ffmpeg's own allocator. `avsubtitle_free` runs `av_freep` over
//! the rects array, each rect and each data plane, so those come from
//! `av_malloc` rather than Rust's allocator.

mod frame;
mod packet;
mod subtitle;

pub use frame::{attach_captions, audio_plane, clear_captions, plane_writer};
pub use packet::{from_bytes, rescale, route, stamp};
pub use subtitle::{
    bitmap_subtitle, empty_subtitle, encode_subtitle, rect_geometry, rect_text,
    set_subtitle_header, text_subtitle,
};

/// A stream or encoder time base: the tick that timestamps are counted in.
///
/// A newtype so the raw ffmpeg rational stays inside this module rather than
/// crossing into the muxers, which is where the "no ffmpeg types escape" rule
/// was leaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeBase {
    num: i32,
    den: i32,
}

impl TimeBase {
    /// A time base of `num/den` seconds per tick.
    pub fn new(num: i32, den: i32) -> Self {
        Self { num, den }
    }

    /// Read a time base back off a stream ffmpeg configured.
    pub(crate) fn from_raw(raw: rsmpeg::ffi::AVRational) -> Self {
        Self {
            num: raw.num,
            den: raw.den,
        }
    }

    /// The raw rational, for handing to an ffmpeg call.
    pub(crate) fn raw(self) -> rsmpeg::ffi::AVRational {
        rsmpeg::ffi::AVRational {
            num: self.num,
            den: self.den,
        }
    }
}

/// The short name of a codec, for example `h264` or `aac`.
pub fn codec_name(id: rsmpeg::ffi::AVCodecID) -> String {
    // SAFETY: avcodec_get_name always returns a valid static C string, even for
    // an unknown id, where it yields "none".
    let name = unsafe { std::ffi::CStr::from_ptr(rsmpeg::ffi::avcodec_get_name(id)) };
    name.to_string_lossy().into_owned()
}

/// Anything that can go wrong at the FFI boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfiError {
    /// An allocation ffmpeg owns could not be made.
    OutOfMemory,
    /// A caller passed a buffer whose length contradicts its declared shape.
    Shape(&'static str),
}

impl std::fmt::Display for FfiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfMemory => write!(formatter, "ffmpeg allocation failed"),
            Self::Shape(detail) => write!(formatter, "{detail}"),
        }
    }
}

impl std::error::Error for FfiError {}

/// Push whatever a muxer is holding out through its IO context.
///
/// ffmpeg buffers output and only writes when its buffer fills. For a file that
/// is exactly right, but on a live stream it means up to a buffer's worth of
/// delay before a viewer sees anything, and a header small enough to sit in the
/// buffer never arrives at all.
pub fn flush_output(output: &mut rsmpeg::avformat::AVFormatContextOutput) {
    // SAFETY: the context owns its IO, and flushing a null pb is a no-op in
    // ffmpeg, so the null check is defensive rather than required.
    unsafe {
        let raw = output.as_mut_ptr();
        if !(*raw).pb.is_null() {
            rsmpeg::ffi::avio_flush((*raw).pb);
        }
    }
}
