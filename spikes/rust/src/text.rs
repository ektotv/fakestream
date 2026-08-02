//! Rendering caption text into a paletted bitmap.
//!
//! No unsafe here. ab_glyph rasterises each glyph to coverage values, and those
//! are quantised into a small palette because DVB CLUTs are 2, 4 or 8 bit.
//!
//! The palette layout is fixed so the encoder and any visual check agree:
//!
//! | index | meaning |
//! | --- | --- |
//! | 0 | fully transparent, outside the box |
//! | 1 | box background |
//! | 2 | box border |
//! | 3..=6 | text coverage ramp, 6 is solid |

use ab_glyph::{Font, FontRef, Glyph, PxScale, ScaleFont};

pub const TRANSPARENT: u8 = 0;
pub const BACKGROUND: u8 = 1;
pub const BORDER: u8 = 2;
/// First index of the antialiasing ramp.
pub const TEXT_BASE: u8 = 3;
/// Number of steps in the ramp, including the solid end.
pub const TEXT_STEPS: u8 = 4;

/// Palette entries as `0xAARRGGBB`, matching the indices above.
pub fn palette() -> Vec<u32> {
    let mut colours = vec![
        0x0000_0000, // transparent
        0xC010_1010, // translucent dark box
        0xFFE0_E0E0, // border
    ];
    for step in 0..TEXT_STEPS {
        let ratio = f32::from(step + 1) / f32::from(TEXT_STEPS);
        let level = (16.0 + ratio * 239.0) as u32;
        colours.push(0xFF00_0000 | (level << 16) | (level << 8) | level);
    }
    colours
}

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
            pixels: vec![TRANSPARENT; (width * height) as usize],
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
            return TRANSPARENT;
        }
        self.pixels[(y * self.width + x) as usize]
    }

    /// Fill the whole canvas with a background and draw a border.
    pub fn draw_box(&mut self, border_thickness: i32) {
        for y in 0..self.height {
            for x in 0..self.width {
                let on_border = x < border_thickness
                    || y < border_thickness
                    || x >= self.width - border_thickness
                    || y >= self.height - border_thickness;
                self.set(x, y, if on_border { BORDER } else { BACKGROUND });
            }
        }
    }
}

#[derive(Debug)]
pub enum TextError {
    FontLoad,
}

/// Lay a single line out horizontally and rasterise it into the canvas.
///
/// Returns the width the text occupied, which is what a caller needs to size or
/// centre a caption box.
pub fn draw_line(
    canvas: &mut Canvas,
    font_bytes: &[u8],
    text: &str,
    size: f32,
    origin_x: f32,
    baseline_y: f32,
) -> Result<f32, TextError> {
    let font = FontRef::try_from_slice(font_bytes).map_err(|_| TextError::FontLoad)?;
    let scaled = font.as_scaled(PxScale::from(size));

    let mut pen_x = origin_x;
    let mut previous: Option<char> = None;

    for character in text.chars() {
        let glyph_id = scaled.glyph_id(character);

        if let Some(last) = previous {
            pen_x += scaled.kern(scaled.glyph_id(last), glyph_id);
        }

        let glyph: Glyph = glyph_id.with_scale_and_position(size, ab_glyph::point(pen_x, baseline_y));

        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|dx, dy, coverage| {
                if coverage <= 0.01 {
                    return;
                }
                let x = bounds.min.x as i32 + dx as i32;
                let y = bounds.min.y as i32 + dy as i32;

                // Quantise coverage into the ramp. Keep whatever is already
                // there if it is brighter, so overlapping glyphs do not erase
                // each other.
                let step = (coverage * f32::from(TEXT_STEPS)).ceil().clamp(1.0, f32::from(TEXT_STEPS));
                let index = TEXT_BASE + step as u8 - 1;
                if index > canvas.get(x, y) {
                    canvas.set(x, y, index);
                }
            });
        }

        pen_x += scaled.h_advance(glyph_id);
        previous = Some(character);
    }

    Ok(pen_x - origin_x)
}

/// Measure a line without drawing it.
pub fn measure(font_bytes: &[u8], text: &str, size: f32) -> Result<f32, TextError> {
    let font = FontRef::try_from_slice(font_bytes).map_err(|_| TextError::FontLoad)?;
    let scaled = font.as_scaled(PxScale::from(size));

    let mut width = 0.0;
    let mut previous: Option<char> = None;
    for character in text.chars() {
        let glyph_id = scaled.glyph_id(character);
        if let Some(last) = previous {
            width += scaled.kern(scaled.glyph_id(last), glyph_id);
        }
        width += scaled.h_advance(glyph_id);
        previous = Some(character);
    }
    Ok(width)
}
