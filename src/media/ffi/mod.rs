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
