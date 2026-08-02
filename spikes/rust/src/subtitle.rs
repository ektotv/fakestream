//! The whole unsafe surface of the DVB path lives here.
//!
//! rsmpeg wraps the encode call but gives no way to build subtitle rects, so
//! populating one is raw FFI. Everything below `BitmapCue` is safe.
//!
//! Allocation rule that makes this work: rsmpeg's `AVSubtitle` runs
//! `avsubtitle_free` on drop, which `av_freep`s the rects array, each rect, and
//! each rect's data planes. So every one of those has to come from `av_malloc`,
//! not from Rust's allocator, or the free corrupts the heap.

use rsmpeg::avcodec::{AVPacket, AVSubtitle};
use rsmpeg::ffi;

/// Wrap encoded bytes in a packet. rsmpeg gives no safe way to attach data to a
/// packet either, so this sits in the same quarantined module.
pub fn packet_from_bytes(bytes: &[u8]) -> Result<AVPacket, CueError> {
    let mut packet = AVPacket::new();

    // SAFETY: av_new_packet allocates the buffer ffmpeg will later free through
    // the packet's own unref, so ownership stays consistent.
    unsafe {
        let raw = packet.as_mut_ptr();
        if ffi::av_new_packet(raw, bytes.len() as i32) < 0 {
            return Err(CueError::OutOfMemory);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), (*raw).data, bytes.len());
    }

    Ok(packet)
}

/// Route a packet to a stream without touching its timestamps. Encoded video
/// carries its own pts and dts, and with B-frames those differ, so overwriting
/// dts breaks the muxer's monotonic ordering.
pub fn route_packet(packet: &mut AVPacket, stream_index: i32) {
    // SAFETY: plain scalar write into a packet this process owns.
    unsafe {
        (*packet.as_mut_ptr()).stream_index = stream_index;
    }
}

/// Convert a packet's timestamps from one time base to another.
///
/// Needed because `write_header` is free to replace the time base you asked
/// for. MPEG-TS always does, forcing 90kHz, so timestamps prepared in any other
/// base are silently misread unless they are rescaled after the header lands.
pub fn rescale_packet(packet: &mut AVPacket, from: ffi::AVRational, to: ffi::AVRational) {
    // SAFETY: ffmpeg's own arithmetic over a packet this process owns.
    unsafe {
        ffi::av_packet_rescale_ts(packet.as_mut_ptr(), from, to);
    }
}

/// Stamp a packet for muxing. Timestamps are in the stream's own time base.
/// Only for streams where we generate the timing ourselves, such as subtitles.
pub fn stamp_packet(packet: &mut AVPacket, stream_index: i32, pts: i64, duration: i64) {
    // SAFETY: plain scalar writes into a packet this process owns.
    unsafe {
        let raw = packet.as_mut_ptr();
        (*raw).stream_index = stream_index;
        (*raw).pts = pts;
        (*raw).dts = pts;
        (*raw).duration = duration;
    }
}

/// Read back a decoded subtitle's rect geometry for verification.
pub fn describe(subtitle: &AVSubtitle) -> Vec<(i32, i32, i32, i32, i32)> {
    // SAFETY: read-only inspection of a subtitle ffmpeg just filled in, bounded
    // by the rect count it reported.
    unsafe {
        let raw = subtitle.as_ptr();
        (0..(*raw).num_rects)
            .map(|index| {
                let rect = **(*raw).rects.add(index as usize);
                (rect.w, rect.h, rect.x, rect.y, rect.nb_colors)
            })
            .collect()
    }
}

/// A caption as a paletted bitmap. This is the safe description callers use.
pub struct BitmapCue {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// One palette index per pixel, `width * height` of them.
    pub pixels: Vec<u8>,
    /// Up to 256 colours as `0xAARRGGBB`.
    pub palette: Vec<u32>,
    pub start_ms: u32,
    pub end_ms: u32,
}

#[derive(Debug)]
pub enum CueError {
    PixelCountMismatch { expected: usize, got: usize },
    TooManyColours(usize),
    OutOfMemory,
}

impl BitmapCue {
    fn validate(&self) -> Result<(), CueError> {
        let expected = (self.width as usize) * (self.height as usize);
        if self.pixels.len() != expected {
            return Err(CueError::PixelCountMismatch {
                expected,
                got: self.pixels.len(),
            });
        }
        if self.palette.len() > 256 {
            return Err(CueError::TooManyColours(self.palette.len()));
        }
        Ok(())
    }

    /// Build an ffmpeg subtitle owning a single bitmap rect.
    pub fn to_subtitle(&self) -> Result<AVSubtitle, CueError> {
        self.validate()?;

        let mut subtitle = AVSubtitle::new();

        // SAFETY: every allocation below uses av_malloc, matching the av_freep
        // that avsubtitle_free will perform when `subtitle` drops. The raw
        // pointer comes from rsmpeg's own live allocation, so it is valid for
        // the duration of this function.
        unsafe {
            let rect = ffi::av_mallocz(size_of::<ffi::AVSubtitleRect>()) as *mut ffi::AVSubtitleRect;
            if rect.is_null() {
                return Err(CueError::OutOfMemory);
            }

            let pixel_bytes = self.pixels.len();
            let pixels = ffi::av_malloc(pixel_bytes) as *mut u8;
            let palette = ffi::av_mallocz(ffi::AVPALETTE_SIZE as usize) as *mut u8;
            if pixels.is_null() || palette.is_null() {
                ffi::av_free(pixels as *mut _);
                ffi::av_free(palette as *mut _);
                ffi::av_free(rect as *mut _);
                return Err(CueError::OutOfMemory);
            }

            std::ptr::copy_nonoverlapping(self.pixels.as_ptr(), pixels, pixel_bytes);
            for (index, colour) in self.palette.iter().enumerate() {
                let entry = palette.add(index * 4) as *mut u32;
                entry.write_unaligned(*colour);
            }

            (*rect).x = self.x;
            (*rect).y = self.y;
            (*rect).w = self.width;
            (*rect).h = self.height;
            (*rect).nb_colors = self.palette.len() as i32;
            (*rect).type_ = ffi::SUBTITLE_BITMAP;
            (*rect).data[0] = pixels;
            (*rect).data[1] = palette;
            (*rect).linesize[0] = self.width;

            let rects = ffi::av_malloc(size_of::<*mut ffi::AVSubtitleRect>()) as *mut *mut ffi::AVSubtitleRect;
            if rects.is_null() {
                ffi::av_free(pixels as *mut _);
                ffi::av_free(palette as *mut _);
                ffi::av_free(rect as *mut _);
                return Err(CueError::OutOfMemory);
            }
            *rects = rect;

            let raw = subtitle.as_mut_ptr();
            (*raw).format = 0; // graphics
            (*raw).start_display_time = self.start_ms;
            (*raw).end_display_time = self.end_ms;
            (*raw).num_rects = 1;
            (*raw).rects = rects;
        }

        Ok(subtitle)
    }
}
