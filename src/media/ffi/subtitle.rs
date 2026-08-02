//! Building bitmap subtitles, which rsmpeg can encode but cannot construct.

use super::FfiError;
use rsmpeg::avcodec::AVSubtitle;
use rsmpeg::ffi;

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
