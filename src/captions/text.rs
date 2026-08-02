//! Rendering caption text into a paletted bitmap.
//!
//! Only the bitmap formats need this. DVB carries pictures, so the glyphs are
//! ours to draw. The text formats carry UTF-8 and the player renders them, so
//! they never come through here.
//!
//! Coverage from the rasteriser is quantised into a short ramp, because DVB
//! colour lookup tables are 2, 4 or 8 bit and there is no room for smooth
//! antialiasing.

use ab_glyph::{Font, FontRef, Glyph, PxScale, ScaleFont};

/// Bundled so the finished binary depends on no system fonts.
/// Noto Sans, under the SIL Open Font License 1.1.
const FONT: &[u8] = include_bytes!("../../assets/fonts/NotoSans.ttf");

/// Palette slots, fixed so the renderer and any visual check agree.
pub mod ink {
    /// Outside the caption box.
    pub const TRANSPARENT: u8 = 0;
    /// The box itself.
    pub const BACKGROUND: u8 = 1;
    /// The box edge.
    pub const BORDER: u8 = 2;
    /// First step of the text coverage ramp.
    pub const TEXT_BASE: u8 = 3;
    /// Steps in the ramp, including the solid end.
    pub const TEXT_STEPS: u8 = 4;
}

/// Palette entries as `0xAARRGGBB`, in the order the `ink` slots describe.
pub fn palette() -> Vec<u32> {
    let mut colours = vec![
        0x0000_0000, // nothing
        0xC010_1010, // translucent dark box
        0xFFE0_E0E0, // border
    ];

    for step in 0..ink::TEXT_STEPS {
        let ratio = f32::from(step + 1) / f32::from(ink::TEXT_STEPS);
        let level = (16.0 + ratio * 239.0) as u32;
        colours.push(0xFF00_0000 | (level << 16) | (level << 8) | level);
    }

    colours
}

/// A paletted image, one byte per pixel.
pub struct Canvas {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            pixels: vec![ink::TRANSPARENT; (width * height) as usize],
        }
    }

    fn set(&mut self, x: i32, y: i32, index: u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        self.pixels[(y * self.width + x) as usize] = index;
    }

    fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return ink::TRANSPARENT;
        }
        self.pixels[(y * self.width + x) as usize]
    }

    /// Fill the canvas and draw a border around it.
    pub fn draw_box(&mut self, thickness: i32) {
        for y in 0..self.height {
            for x in 0..self.width {
                let edge = x < thickness
                    || y < thickness
                    || x >= self.width - thickness
                    || y >= self.height - thickness;
                self.set(x, y, if edge { ink::BORDER } else { ink::BACKGROUND });
            }
        }
    }

    /// Rasterise one line with its left edge at `origin_x` and its baseline at
    /// `baseline_y`.
    pub fn draw_line(&mut self, text: &str, size: f32, origin_x: f32, baseline_y: f32) {
        let Ok(font) = FontRef::try_from_slice(FONT) else {
            return;
        };
        let scaled = font.as_scaled(PxScale::from(size));

        let mut pen = origin_x;
        let mut previous: Option<char> = None;

        for character in text.chars() {
            let id = scaled.glyph_id(character);
            if let Some(last) = previous {
                pen += scaled.kern(scaled.glyph_id(last), id);
            }

            let glyph: Glyph = id.with_scale_and_position(size, ab_glyph::point(pen, baseline_y));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|dx, dy, coverage| {
                    if coverage <= 0.01 {
                        return;
                    }

                    let x = bounds.min.x as i32 + dx as i32;
                    let y = bounds.min.y as i32 + dy as i32;

                    let step = (coverage * f32::from(ink::TEXT_STEPS))
                        .ceil()
                        .clamp(1.0, f32::from(ink::TEXT_STEPS));
                    let index = ink::TEXT_BASE + step as u8 - 1;

                    // Keep whatever is brighter, so overlapping glyphs do not
                    // erase each other's edges.
                    if index > self.get(x, y) {
                        self.set(x, y, index);
                    }
                });
            }

            pen += scaled.h_advance(id);
            previous = Some(character);
        }
    }
}

/// How wide a line will be, without drawing it.
pub fn measure(text: &str, size: f32) -> f32 {
    let Ok(font) = FontRef::try_from_slice(FONT) else {
        return 0.0;
    };
    let scaled = font.as_scaled(PxScale::from(size));

    let mut width = 0.0;
    let mut previous: Option<char> = None;
    for character in text.chars() {
        let id = scaled.glyph_id(character);
        if let Some(last) = previous {
            width += scaled.kern(scaled.glyph_id(last), id);
        }
        width += scaled.h_advance(id);
        previous = Some(character);
    }
    width
}

/// The distance between the baselines of successive lines.
pub fn line_height(size: f32) -> f32 {
    let Ok(font) = FontRef::try_from_slice(FONT) else {
        return size * 1.2;
    };
    let scaled = font.as_scaled(PxScale::from(size));
    scaled.height() + scaled.line_gap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_font_loads() {
        assert!(
            FontRef::try_from_slice(FONT).is_ok(),
            "bundled font is unusable"
        );
    }

    #[test]
    fn the_palette_covers_every_ink_slot() {
        let palette = palette();
        let highest = ink::TEXT_BASE + ink::TEXT_STEPS - 1;
        assert_eq!(palette.len(), highest as usize + 1);
    }

    #[test]
    fn nothing_is_transparent_except_the_first_slot() {
        for (index, colour) in palette().iter().enumerate().skip(1) {
            assert_ne!(colour >> 24, 0, "slot {index} is fully transparent");
        }
    }

    #[test]
    fn wider_text_measures_wider() {
        assert!(measure("iiii", 32.0) < measure("MMMM", 32.0));
    }

    #[test]
    fn drawing_marks_the_canvas() {
        let mut canvas = Canvas::new(200, 60);
        canvas.draw_box(2);
        let before = canvas
            .pixels
            .iter()
            .filter(|p| **p >= ink::TEXT_BASE)
            .count();

        canvas.draw_line("Lorem", 32.0, 10.0, 45.0);
        let after = canvas
            .pixels
            .iter()
            .filter(|p| **p >= ink::TEXT_BASE)
            .count();

        assert_eq!(before, 0);
        assert!(after > 0, "no glyph pixels were drawn");
    }

    #[test]
    fn drawing_outside_the_canvas_is_ignored() {
        let mut canvas = Canvas::new(40, 20);
        // Far off the right edge, which must not panic or wrap onto other rows.
        canvas.draw_line("Lorem ipsum dolor", 32.0, 500.0, 15.0);
        assert!(canvas.pixels.iter().all(|p| *p == ink::TRANSPARENT));
    }

    #[test]
    fn accented_glyphs_render() {
        let mut plain = Canvas::new(200, 60);
        plain.draw_line("e", 40.0, 10.0, 45.0);

        let mut accented = Canvas::new(200, 60);
        accented.draw_line("é", 40.0, 10.0, 45.0);

        assert_ne!(plain.pixels, accented.pixels, "the accent did not render");
    }
}
