//! DVB bitmap subtitles.
//!
//! The opposite of CEA-608 in every respect. DVB is announced as a real stream
//! in the container, and carries pictures rather than text, so the player only
//! blits what we draw. That makes glyph rendering our problem and character set
//! coverage a non-issue.

use super::script::Cue;
use super::text::{self, Canvas};

/// A caption rendered ready for the encoder.
pub struct Rendered {
    /// Where the bitmap sits within the display, in display pixels.
    pub x: i32,
    pub y: i32,
    pub canvas: Canvas,
}

/// Layout choices, derived from the display size so a fixture looks right at
/// any resolution.
pub struct Layout {
    pub display_width: i32,
    pub display_height: i32,
    pub font_size: f32,
    pub padding: i32,
    /// Distance from the bottom of the display to the bottom of the box.
    pub bottom_margin: i32,
}

impl Layout {
    /// Proportional to the display height, which keeps captions the same
    /// relative size whether the fixture is 576p or 1080p.
    pub fn for_display(width: i32, height: i32) -> Self {
        Self {
            display_width: width,
            display_height: height,
            font_size: (height as f32 / 20.0).round(),
            padding: (height / 40).max(8),
            bottom_margin: (height / 12).max(16),
        }
    }
}

/// Draw a cue into a bitmap sized to its own text.
///
/// Lines are centred against each other, and the box is centred in the display,
/// which is where a viewer expects subtitles rather than where a caption
/// standard happens to put them.
pub fn render(cue: &Cue, layout: &Layout) -> Rendered {
    let size = layout.font_size;
    let spacing = text::line_height(size);

    let widest = cue
        .lines
        .iter()
        .map(|line| text::measure(line, size))
        .fold(0.0f32, f32::max);

    let max_width = layout.display_width - layout.padding * 4;
    let width = (widest.ceil() as i32 + layout.padding * 2)
        .min(max_width)
        .max(1);
    let height = (spacing * cue.lines.len() as f32).ceil() as i32 + layout.padding * 2;

    let mut canvas = Canvas::new(width, height);
    canvas.draw_box(2);

    for (index, line) in cue.lines.iter().enumerate() {
        let line_width = text::measure(line, size);
        let origin_x = ((width as f32 - line_width) / 2.0).max(layout.padding as f32);
        // Baselines step down from the top padding, offset within the line box
        // so descenders have room.
        let baseline = layout.padding as f32 + spacing * (index as f32 + 0.8);
        canvas.draw_line(line, size, origin_x, baseline);
    }

    Rendered {
        x: (layout.display_width - width) / 2,
        y: layout.display_height - height - layout.bottom_margin,
        canvas,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(lines: &[&str]) -> Cue {
        Cue {
            start: 1.0,
            duration: 2.0,
            lines: lines.iter().map(|line| line.to_string()).collect(),
        }
    }

    #[test]
    fn the_box_sits_inside_the_display() {
        let layout = Layout::for_display(1280, 720);
        let rendered = render(&cue(&["Lorem ipsum dolor sit amet"]), &layout);

        assert!(rendered.x >= 0, "box runs off the left edge");
        assert!(rendered.y >= 0, "box runs off the top edge");
        assert!(
            rendered.x + rendered.canvas.width <= layout.display_width,
            "box runs off the right edge"
        );
        assert!(
            rendered.y + rendered.canvas.height <= layout.display_height,
            "box runs off the bottom edge"
        );
    }

    #[test]
    fn a_very_long_line_is_clamped_to_the_display() {
        let layout = Layout::for_display(720, 576);
        let long = "M".repeat(200);
        let rendered = render(&cue(&[&long]), &layout);

        assert!(rendered.x >= 0);
        assert!(rendered.canvas.width <= layout.display_width);
    }

    #[test]
    fn two_lines_are_taller_than_one() {
        let layout = Layout::for_display(1280, 720);
        let one = render(&cue(&["Lorem ipsum"]), &layout);
        let two = render(&cue(&["Lorem ipsum", "dolor sit amet"]), &layout);

        assert!(two.canvas.height > one.canvas.height);
    }

    #[test]
    fn every_line_actually_draws() {
        let layout = Layout::for_display(1280, 720);
        let rendered = render(&cue(&["Lorem ipsum", "dolor sit amet"]), &layout);

        // Split the box in half and check both halves carry glyph pixels, which
        // catches a second line drawn off the bottom of its own bitmap.
        let half = rendered.canvas.height / 2;
        let row_width = rendered.canvas.width as usize;
        let split = half as usize * row_width;

        let top = rendered.canvas.pixels[..split]
            .iter()
            .filter(|p| **p >= text::ink::TEXT_BASE)
            .count();
        let bottom = rendered.canvas.pixels[split..]
            .iter()
            .filter(|p| **p >= text::ink::TEXT_BASE)
            .count();

        assert!(top > 0, "nothing drawn in the upper half");
        assert!(bottom > 0, "nothing drawn in the lower half");
    }

    #[test]
    fn the_box_scales_with_the_display() {
        let small = render(&cue(&["Lorem ipsum"]), &Layout::for_display(720, 576));
        let large = render(&cue(&["Lorem ipsum"]), &Layout::for_display(1920, 1080));

        assert!(
            large.canvas.height > small.canvas.height,
            "captions should scale with the display"
        );
    }
}
