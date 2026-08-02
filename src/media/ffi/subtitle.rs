//! Building bitmap subtitles, which rsmpeg can encode but cannot construct.

use super::FfiError;
use rsmpeg::avcodec::{AVCodecContext, AVSubtitle};
use rsmpeg::ffi;
use std::ffi::CString;

/// Geometry read back from a decoded subtitle, used for verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectGeometry {
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub colours: i32,
}

/// Build a subtitle owning a single paletted bitmap rect.
///
/// `pixels` holds one palette index per pixel and must be exactly
/// `width * height` long. `palette` holds up to 256 colours as `0xAARRGGBB`.
pub fn bitmap_subtitle(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    pixels: &[u8],
    palette: &[u32],
    duration_ms: u32,
) -> Result<AVSubtitle, FfiError> {
    let expected = (width as usize) * (height as usize);
    if pixels.len() != expected {
        return Err(FfiError::Shape(
            "pixel count does not match width times height",
        ));
    }
    if palette.len() > 256 {
        return Err(FfiError::Shape("palette holds at most 256 colours"));
    }

    let mut subtitle = AVSubtitle::new();

    // SAFETY: every allocation here uses av_malloc, matching the av_freep that
    // avsubtitle_free performs when the subtitle drops. On any failure the
    // partial allocations are released before returning, so nothing leaks.
    unsafe {
        let rect = ffi::av_mallocz(size_of::<ffi::AVSubtitleRect>()) as *mut ffi::AVSubtitleRect;
        if rect.is_null() {
            return Err(FfiError::OutOfMemory);
        }

        let pixel_data = ffi::av_malloc(pixels.len()) as *mut u8;
        let palette_data = ffi::av_mallocz(ffi::AVPALETTE_SIZE as usize) as *mut u8;
        let rect_list =
            ffi::av_malloc(size_of::<*mut ffi::AVSubtitleRect>()) as *mut *mut ffi::AVSubtitleRect;

        if pixel_data.is_null() || palette_data.is_null() || rect_list.is_null() {
            ffi::av_free(pixel_data as *mut _);
            ffi::av_free(palette_data as *mut _);
            ffi::av_free(rect_list as *mut _);
            ffi::av_free(rect as *mut _);
            return Err(FfiError::OutOfMemory);
        }

        std::ptr::copy_nonoverlapping(pixels.as_ptr(), pixel_data, pixels.len());
        for (index, colour) in palette.iter().enumerate() {
            (palette_data.add(index * 4) as *mut u32).write_unaligned(*colour);
        }

        (*rect).x = x;
        (*rect).y = y;
        (*rect).w = width;
        (*rect).h = height;
        (*rect).nb_colors = palette.len() as i32;
        (*rect).type_ = ffi::SUBTITLE_BITMAP;
        (*rect).data[0] = pixel_data;
        (*rect).data[1] = palette_data;
        (*rect).linesize[0] = width;

        *rect_list = rect;

        let raw = subtitle.as_mut_ptr();
        (*raw).format = 0; // graphics rather than text
        (*raw).start_display_time = 0;
        (*raw).end_display_time = duration_ms;
        (*raw).num_rects = 1;
        (*raw).rects = rect_list;
    }

    Ok(subtitle)
}

/// A subtitle carrying no regions, which is how DVB ends a caption.
///
/// Without one the caption stays on screen until the decoder's own page
/// timeout, which is measured in tens of seconds and has nothing to do with how
/// long the cue was meant to last.
pub fn empty_subtitle() -> AVSubtitle {
    AVSubtitle::new()
}

/// Read the rect geometry out of a subtitle, for checking a round trip.
pub fn rect_geometry(subtitle: &AVSubtitle) -> Vec<RectGeometry> {
    // SAFETY: read-only inspection of a subtitle ffmpeg filled in, bounded by
    // the rect count it reported.
    unsafe {
        let raw = subtitle.as_ptr();
        (0..(*raw).num_rects)
            .map(|index| {
                let rect = **(*raw).rects.add(index as usize);
                RectGeometry {
                    width: rect.w,
                    height: rect.h,
                    x: rect.x,
                    y: rect.y,
                    colours: rect.nb_colors,
                }
            })
            .collect()
    }
}

/// Encode a subtitle, returning how many bytes were written.
///
/// rsmpeg's wrapper discards the length `avcodec_encode_subtitle` returns,
/// which leaves a caller guessing. Guessing by trimming trailing zeroes is
/// actively wrong here: a DVB packet ends with an end-of-display-set segment
/// whose last bytes are `00 00`, so trimming truncates the segment that tells a
/// decoder the page is complete.
pub fn encode_subtitle(
    encoder: &mut AVCodecContext,
    subtitle: &AVSubtitle,
    buffer: &mut [u8],
) -> Result<usize, FfiError> {
    // SAFETY: the encoder and subtitle are live, and the buffer length is
    // passed alongside the pointer so ffmpeg cannot write past it.
    let written = unsafe {
        ffi::avcodec_encode_subtitle(
            encoder.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len() as i32,
            subtitle.as_ptr(),
        )
    };

    if written < 0 {
        return Err(FfiError::Shape(
            "the subtitle encoder rejected the subtitle",
        ));
    }

    Ok(written as usize)
}

/// Build a subtitle carrying one ASS dialogue line.
///
/// Every text subtitle encoder in ffmpeg reads this form and converts on the
/// way out, whichever format it finally writes. The rect type must be
/// `SUBTITLE_ASS` or the encoder refuses outright.
pub fn text_subtitle(dialogue: &str, duration_ms: u32) -> Result<AVSubtitle, FfiError> {
    let line =
        CString::new(dialogue).map_err(|_| FfiError::Shape("dialogue contains a null byte"))?;
    let bytes = line.as_bytes_with_nul();

    let mut subtitle = AVSubtitle::new();

    // SAFETY: allocations use av_malloc to match the av_freep that
    // avsubtitle_free performs on the rect, its ass string and the rect list.
    // Partial allocations are released before any early return.
    unsafe {
        let rect = ffi::av_mallocz(size_of::<ffi::AVSubtitleRect>()) as *mut ffi::AVSubtitleRect;
        if rect.is_null() {
            return Err(FfiError::OutOfMemory);
        }

        let ass = ffi::av_malloc(bytes.len()) as *mut std::os::raw::c_char;
        let rect_list =
            ffi::av_malloc(size_of::<*mut ffi::AVSubtitleRect>()) as *mut *mut ffi::AVSubtitleRect;

        if ass.is_null() || rect_list.is_null() {
            ffi::av_free(ass as *mut _);
            ffi::av_free(rect_list as *mut _);
            ffi::av_free(rect as *mut _);
            return Err(FfiError::OutOfMemory);
        }

        std::ptr::copy_nonoverlapping(
            bytes.as_ptr() as *const std::os::raw::c_char,
            ass,
            bytes.len(),
        );

        (*rect).type_ = ffi::SUBTITLE_ASS;
        (*rect).ass = ass;

        *rect_list = rect;

        let raw = subtitle.as_mut_ptr();
        (*raw).format = 1; // text rather than graphics
        (*raw).start_display_time = 0;
        (*raw).end_display_time = duration_ms;
        (*raw).num_rects = 1;
        (*raw).rects = rect_list;
    }

    Ok(subtitle)
}

/// Give an encoder the ASS header it parses when opened.
///
/// Text encoders read style definitions out of this, and refuse to open
/// without it. ffmpeg builds one internally for its own decoders, but that
/// function is not public, so the header is supplied by the caller.
///
/// Must be called before the encoder is opened.
pub fn set_subtitle_header(encoder: &mut AVCodecContext, header: &str) -> Result<(), FfiError> {
    let bytes = header.as_bytes();

    // SAFETY: the buffer is allocated with av_malloc, which is what
    // avcodec_free_context releases it with, and ownership passes to the
    // context. ffmpeg expects the size to exclude a terminator but the buffer
    // to be padded, hence the extra zeroed bytes.
    unsafe {
        let buffer =
            ffi::av_mallocz(bytes.len() + ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize) as *mut u8;
        if buffer.is_null() {
            return Err(FfiError::OutOfMemory);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());

        let raw = encoder.as_mut_ptr();
        (*raw).subtitle_header = buffer;
        (*raw).subtitle_header_size = bytes.len() as i32;
    }

    Ok(())
}

/// Read the text out of a decoded subtitle.
///
/// Text rects carry an ASS dialogue line whose last field is the text, so the
/// leading fields are dropped. Bitmap rects carry no text and yield nothing.
pub fn rect_text(subtitle: &AVSubtitle) -> Vec<String> {
    // SAFETY: read-only inspection of a subtitle ffmpeg filled in, bounded by
    // the rect count it reported, and each string is ffmpeg's own nul
    // terminated allocation.
    unsafe {
        let raw = subtitle.as_ptr();
        (0..(*raw).num_rects)
            .filter_map(|index| {
                let rect = **(*raw).rects.add(index as usize);
                let source = if !rect.ass.is_null() {
                    rect.ass
                } else if !rect.text.is_null() {
                    rect.text
                } else {
                    return None;
                };

                let raw_text = std::ffi::CStr::from_ptr(source)
                    .to_string_lossy()
                    .into_owned();
                Some(strip_ass_fields(&raw_text))
            })
            .collect()
    }
}

/// Drop ASS's nine leading fields, leaving the text itself.
///
/// A dialogue looks like `0,0,Default,,0,0,0,,the text`, and the text may
/// contain commas of its own, so the split has to be bounded.
fn strip_ass_fields(dialogue: &str) -> String {
    const FIELDS_BEFORE_TEXT: usize = 8;

    let mut parts = dialogue.splitn(FIELDS_BEFORE_TEXT + 1, ',');
    for _ in 0..FIELDS_BEFORE_TEXT {
        if parts.next().is_none() {
            return dialogue.to_string();
        }
    }
    parts.next().unwrap_or(dialogue).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ass_fields_are_stripped_from_the_text() {
        assert_eq!(
            strip_ass_fields("0,0,Default,,0,0,0,,Lorem ipsum"),
            "Lorem ipsum"
        );
    }

    #[test]
    fn commas_in_the_text_survive() {
        assert_eq!(
            strip_ass_fields("0,0,Default,,0,0,0,,one, two, three"),
            "one, two, three"
        );
    }

    #[test]
    fn something_that_is_not_a_dialogue_is_left_alone() {
        assert_eq!(strip_ass_fields("plain text"), "plain text");
    }
}
