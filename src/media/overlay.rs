//! Drawing onto video frames.
//!
//! The glyph rasteriser produces a paletted canvas, which suits DVB because DVB
//! wants a palette. Video wants luma and chroma planes, so the canvas is
//! composited onto the frame here rather than converted.

use super::MediaError;
use crate::captions::text::{Canvas, ink};
use crate::media::ffi;
use rsmpeg::avutil::AVFrame;

/// Neutral chroma, which renders as grey and lets luma alone decide brightness.
const NEUTRAL_CHROMA: u8 = 128;

/// Luma for the brightest text.
const TEXT_LUMA: u8 = 235;

/// Luma for the panel behind text, dark enough for white to read over any
/// picture.
const PANEL_LUMA: u8 = 24;

/// Composite a canvas onto a frame at `x`, `y`.
///
/// The canvas is paletted, and each slot maps to a luma value rather than a
/// colour, because an overlay that has to stay readable over an arbitrary
/// picture is better off greyscale. Chroma is neutralised under the whole
/// canvas so the panel cannot pick up whatever colour was behind it.
pub fn draw_canvas(
    frame: &mut AVFrame,
    canvas: &Canvas,
    x: i32,
    y: i32,
    width: usize,
    height: usize,
) -> Result<(), MediaError> {
    {
        let mut luma = ffi::plane_writer(frame, 0, width, height)?;

        for row in 0..canvas.height {
            let target_y = y + row;
            if target_y < 0 || target_y as usize >= height {
                continue;
            }

            let line = luma.row(target_y as usize);
            for column in 0..canvas.width {
                let target_x = x + column;
                if target_x < 0 || target_x as usize >= width {
                    continue;
                }

                let slot = canvas.pixels[(row * canvas.width + column) as usize];
                if let Some(value) = luma_for(slot) {
                    line[target_x as usize] = value;
                }
            }
        }
    }

    // Chroma planes are half resolution, so the canvas footprint halves too.
    for plane in 1..3 {
        let mut chroma = ffi::plane_writer(frame, plane, width / 2, height / 2)?;

        for row in 0..(canvas.height / 2) {
            let target_y = y / 2 + row;
            if target_y < 0 || target_y as usize >= height / 2 {
                continue;
            }

            let line = chroma.row(target_y as usize);
            for column in 0..(canvas.width / 2) {
                let target_x = x / 2 + column;
                if target_x < 0 || target_x as usize >= width / 2 {
                    continue;
                }

                // Only where the canvas actually covers, so a transparent
                // corner does not grey out the picture behind it.
                let slot = canvas.pixels[(row * 2 * canvas.width + column * 2) as usize];
                if slot != ink::TRANSPARENT {
                    line[target_x as usize] = NEUTRAL_CHROMA;
                }
            }
        }
    }

    Ok(())
}

/// Map a palette slot to a luma value, or nothing where the canvas is clear.
fn luma_for(slot: u8) -> Option<u8> {
    match slot {
        ink::TRANSPARENT => None,
        ink::BACKGROUND => Some(PANEL_LUMA),
        ink::BORDER => Some(TEXT_LUMA),
        step => {
            // The text ramp runs from the panel colour up to full brightness,
            // which is what keeps the glyph edges smooth.
            let position = step.saturating_sub(ink::TEXT_BASE);
            let steps = ink::TEXT_STEPS.saturating_sub(1).max(1);
            let ratio = f32::from(position.min(steps)) / f32::from(steps);
            let value = f32::from(PANEL_LUMA) + ratio * f32::from(TEXT_LUMA - PANEL_LUMA);
            Some(value as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clear_pixel_leaves_the_picture_alone() {
        assert_eq!(luma_for(ink::TRANSPARENT), None);
    }

    #[test]
    fn the_panel_is_dark_and_the_border_is_bright() {
        assert_eq!(luma_for(ink::BACKGROUND), Some(PANEL_LUMA));
        assert_eq!(luma_for(ink::BORDER), Some(TEXT_LUMA));
    }

    #[test]
    fn the_text_ramp_climbs_from_the_panel_to_full_brightness() {
        let lowest = luma_for(ink::TEXT_BASE).expect("ramp start");
        let highest = luma_for(ink::TEXT_BASE + ink::TEXT_STEPS - 1).expect("ramp end");

        assert_eq!(lowest, PANEL_LUMA);
        assert_eq!(highest, TEXT_LUMA);
        assert!(lowest < highest);
    }

    #[test]
    fn the_ramp_never_runs_past_full_brightness() {
        // A slot beyond the declared ramp must clamp rather than wrap around
        // into darkness, which would show as speckled glyph edges.
        for slot in ink::TEXT_BASE..=255 {
            let value = luma_for(slot).expect("a text slot always draws");
            assert!(value <= TEXT_LUMA, "slot {slot} gave {value}");
        }
    }
}
