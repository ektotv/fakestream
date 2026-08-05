//! A box of text lines, sized to its own content, with a border.
//!
//! DVB subtitles and the live clock both draw the same thing: a bordered box
//! holding a few lines, each on a baseline stepped down from the top. They
//! differ only in whether the lines are centred and whether the box is capped
//! to the display width, so those are the two knobs here. The rasterising
//! itself belongs to `text`.

use super::text::{self, Canvas};

/// How lines sit horizontally within the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Each line pinned to the left padding. The clock reads left to right.
    Left,
    /// Each line centred against the box, where a viewer expects subtitles.
    Centre,
}

/// Draw `lines` into a box sized to the widest line, with a two-pixel border.
///
/// `max_width` caps the box for a subtitle that must not run off the display;
/// None lets the box grow to its content.
pub fn render(
    lines: &[String],
    font_size: f32,
    padding: i32,
    align: Align,
    max_width: Option<i32>,
) -> Canvas {
    let spacing = text::line_height(font_size);

    let widest = lines
        .iter()
        .map(|line| text::measure(line, font_size))
        .fold(0.0f32, f32::max);

    let mut width = widest.ceil() as i32 + padding * 2;
    if let Some(cap) = max_width {
        width = width.min(cap);
    }
    let width = width.max(1);
    let height = ((spacing * lines.len() as f32).ceil() as i32 + padding * 2).max(1);

    let mut canvas = Canvas::new(width, height);
    canvas.draw_box(2);

    for (index, line) in lines.iter().enumerate() {
        let origin_x = match align {
            Align::Left => padding as f32,
            Align::Centre => {
                let line_width = text::measure(line, font_size);
                ((width as f32 - line_width) / 2.0).max(padding as f32)
            }
        };
        // Baselines step down from the top padding, offset within the line box
        // so descenders have room.
        let baseline = padding as f32 + spacing * (index as f32 + 0.8);
        canvas.draw_line(line, font_size, origin_x, baseline);
    }

    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn more_lines_make_a_taller_box() {
        let one = render(&lines(&["Lorem ipsum"]), 24.0, 8, Align::Left, None);
        let two = render(
            &lines(&["Lorem ipsum", "dolor sit"]),
            24.0,
            8,
            Align::Left,
            None,
        );
        assert!(two.height > one.height);
    }

    #[test]
    fn max_width_caps_the_box() {
        let long = "M".repeat(200);
        let capped = render(&lines(&[&long]), 24.0, 8, Align::Centre, Some(300));
        assert!(capped.width <= 300);
    }

    #[test]
    fn both_halves_carry_glyph_pixels() {
        // Two lines, each drawn on its own baseline: a second line drawn off the
        // bottom of its box would leave the lower half blank.
        let canvas = render(
            &lines(&["Lorem ipsum", "dolor sit amet"]),
            24.0,
            8,
            Align::Centre,
            None,
        );
        let split = (canvas.height / 2) as usize * canvas.width as usize;
        let top = canvas.pixels[..split]
            .iter()
            .filter(|p| **p >= text::ink::TEXT_BASE)
            .count();
        let bottom = canvas.pixels[split..]
            .iter()
            .filter(|p| **p >= text::ink::TEXT_BASE)
            .count();
        assert!(top > 0 && bottom > 0, "a line failed to draw in its half");
    }
}
