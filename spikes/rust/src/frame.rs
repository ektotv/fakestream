//! Second quarantined unsafe module. Writing pixels into an AVFrame means
//! touching the raw plane pointers, so it is kept here behind a safe call
//! rather than spread through the muxing code.

use rsmpeg::avutil::AVFrame;
use rsmpeg::ffi;

/// Attach A53 closed caption data to a frame.
///
/// ffmpeg's libx264 wrapper reads this side data in `ff_alloc_a53_sei` and
/// emits the SEI itself, so the caller only supplies raw `cc_data` triplets and
/// the encoder does the ATSC wrapping. The `a53cc` encoder option, on by
/// default, is what enables that.
pub fn attach_captions(frame: &mut AVFrame, cc_data: &[u8]) -> Result<(), &'static str> {
    if cc_data.is_empty() {
        return Ok(());
    }
    if cc_data.len() % 3 != 0 {
        return Err("cc_data must be whole three byte triplets");
    }

    // SAFETY: av_frame_new_side_data allocates a buffer the frame then owns, so
    // it is freed with the frame. The copy stays inside the reported size.
    unsafe {
        let side_data = ffi::av_frame_new_side_data(
            frame.as_mut_ptr(),
            ffi::AV_FRAME_DATA_A53_CC,
            cc_data.len(),
        );
        if side_data.is_null() {
            return Err("could not allocate A53 side data");
        }
        std::ptr::copy_nonoverlapping(cc_data.as_ptr(), (*side_data).data, cc_data.len());
    }

    Ok(())
}

/// Drop any side data left from the previous use of a reused frame.
pub fn clear_side_data(frame: &mut AVFrame) {
    // SAFETY: ffmpeg's own accessor over a frame this process owns.
    unsafe {
        ffi::av_frame_remove_side_data(frame.as_mut_ptr(), ffi::AV_FRAME_DATA_A53_CC);
    }
}

/// Paint a moving test pattern. `phase` shifts the pattern so successive frames
/// differ, which stops the encoder collapsing everything into one tiny keyframe
/// and makes the output look like real motion.
pub fn paint_pattern(frame: &mut AVFrame, phase: i32) {
    let width = frame.width;
    let height = frame.height;

    // SAFETY: the frame owns buffers allocated by alloc_buffer for exactly
    // these dimensions, and each plane is written within its own linesize.
    unsafe {
        let raw = frame.as_mut_ptr();

        let y_plane = (*raw).data[0];
        let y_stride = (*raw).linesize[0] as isize;
        for y in 0..height {
            let row = y_plane.offset(y as isize * y_stride);
            for x in 0..width {
                let value = (((x + phase) ^ y) & 0xFF) as u8;
                row.offset(x as isize).write(value);
            }
        }

        // Chroma planes are half resolution in YUV420P.
        for plane in 1..3 {
            let data = (*raw).data[plane];
            let stride = (*raw).linesize[plane] as isize;
            let level = if plane == 1 { 128u8 } else { 100u8 };
            for y in 0..height / 2 {
                let row = data.offset(y as isize * stride);
                for x in 0..width / 2 {
                    row.offset(x as isize).write(level);
                }
            }
        }
    }
}
