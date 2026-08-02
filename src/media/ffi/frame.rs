//! Writing into AVFrames: pixel planes and closed caption side data.

use super::FfiError;
use rsmpeg::avutil::AVFrame;
use rsmpeg::ffi;

/// Borrowed access to one image plane, as safe slices of rows.
///
/// Rows are handed out one at a time because a plane's stride is usually wider
/// than its visible width, so the buffer is not a single contiguous image.
pub struct PlaneWriter<'a> {
    rows: Vec<&'a mut [u8]>,
    width: usize,
}

impl PlaneWriter<'_> {
    pub fn height(&self) -> usize {
        self.rows.len()
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// The visible part of one row, excluding stride padding.
    pub fn row(&mut self, y: usize) -> &mut [u8] {
        let width = self.width;
        &mut self.rows[y][..width]
    }

    /// Set every visible pixel in the plane to one value.
    pub fn fill(&mut self, value: u8) {
        for y in 0..self.height() {
            self.row(y).fill(value);
        }
    }
}

/// Hand out safe row slices for one plane of a frame.
///
/// `width` and `height` are the plane's own dimensions, which for chroma planes
/// in YUV420P are half those of the frame.
pub fn plane_writer(
    frame: &mut AVFrame,
    plane: usize,
    width: usize,
    height: usize,
) -> Result<PlaneWriter<'_>, FfiError> {
    if plane >= 4 {
        return Err(FfiError::Shape("plane index must be below 4"));
    }

    // SAFETY: the frame owns buffers allocated for its declared format, and each
    // row slice is bounded by that plane's own stride, so the slices never
    // overlap and never leave the allocation.
    let rows = unsafe {
        let raw = frame.as_mut_ptr();
        let data = (*raw).data[plane];
        if data.is_null() {
            return Err(FfiError::Shape("frame plane is not allocated"));
        }
        let stride = (*raw).linesize[plane] as usize;
        if stride < width {
            return Err(FfiError::Shape("plane stride is narrower than its width"));
        }

        (0..height)
            .map(|y| std::slice::from_raw_parts_mut(data.add(y * stride), stride))
            .collect()
    };

    Ok(PlaneWriter { rows, width })
}

/// Borrow one channel of a planar audio frame as bytes.
///
/// Audio frames only populate `linesize[0]`, which gives the size of a single
/// plane and applies to every channel. The other linesize entries stay zero, so
/// the video plane accessor cannot be reused here.
pub fn audio_plane(
    frame: &mut AVFrame,
    channel: usize,
    bytes: usize,
) -> Result<&mut [u8], FfiError> {
    if channel >= 8 {
        return Err(FfiError::Shape(
            "planar audio beyond 8 channels needs extended_data",
        ));
    }

    // SAFETY: the frame owns a buffer per channel, each of linesize[0] bytes,
    // allocated for the sample count and format it was told about.
    unsafe {
        let raw = frame.as_mut_ptr();
        let plane_size = (*raw).linesize[0] as usize;
        if bytes > plane_size {
            return Err(FfiError::Shape("audio plane is smaller than the write"));
        }
        let data = (*raw).data[channel];
        if data.is_null() {
            return Err(FfiError::Shape("audio channel is not allocated"));
        }
        Ok(std::slice::from_raw_parts_mut(data, bytes))
    }
}

/// Attach CEA-608 or 708 caption data to a frame.
///
/// ffmpeg's libx264 wrapper reads this in `ff_alloc_a53_sei` and emits the SEI
/// itself, adding the country code, `GA94` identifier, type byte, count and
/// trailing marker. The caller supplies only the raw `cc_data` triplets. The
/// encoder's `a53cc` option, on by default, is what enables this.
pub fn attach_captions(frame: &mut AVFrame, cc_data: &[u8]) -> Result<(), FfiError> {
    if cc_data.is_empty() {
        return Ok(());
    }
    if !cc_data.len().is_multiple_of(3) {
        return Err(FfiError::Shape("cc_data must be whole three byte triplets"));
    }

    // SAFETY: av_frame_new_side_data allocates a buffer the frame then owns and
    // frees with itself. The copy stays within the size just reported.
    unsafe {
        let side_data = ffi::av_frame_new_side_data(
            frame.as_mut_ptr(),
            ffi::AV_FRAME_DATA_A53_CC,
            cc_data.len(),
        );
        if side_data.is_null() {
            return Err(FfiError::OutOfMemory);
        }
        std::ptr::copy_nonoverlapping(cc_data.as_ptr(), (*side_data).data, cc_data.len());
    }

    Ok(())
}

/// Remove caption side data left over from a previous use of a reused frame.
pub fn clear_captions(frame: &mut AVFrame) {
    // SAFETY: ffmpeg's own accessor over a frame this process owns.
    unsafe {
        ffi::av_frame_remove_side_data(frame.as_mut_ptr(), ffi::AV_FRAME_DATA_A53_CC);
    }
}
