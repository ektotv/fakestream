//! The on-screen clock for live streams.
//!
//! A live stream that looks identical from one second to the next tells you
//! nothing. Burning the time into the picture makes three things checkable at a
//! glance: that the stream really is live rather than a loop, how far behind
//! real time the player is, and whether captions line up with the picture.
//!
//! Latency is the useful one. Hold a real clock next to the screen and the
//! difference is the end-to-end delay, with no instrumentation on either side.

use crate::captions::text::{Canvas, ink};

/// Where the clock sits, as a fraction of the display.
const MARGIN_RATIO: f64 = 0.03;

/// What the clock shows on a given frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// Wall clock, as `HH:MM:SS.mmm` in UTC.
    pub wall: String,
    /// Time since the stream started, as `HH:MM:SS.mmm`.
    pub elapsed: String,
    /// Frame number since the stream started, which exposes dropped frames that
    /// a time display alone would hide.
    pub frame: u64,
}

impl Reading {
    /// The two lines the clock renders.
    pub fn lines(&self) -> Vec<String> {
        vec![
            format!("UTC  {}", self.wall),
            format!("LIVE {}   f{}", self.elapsed, self.frame),
        ]
    }
}

/// Format a duration in seconds as `HH:MM:SS.mmm`.
fn as_timecode(total_seconds: f64) -> String {
    let clamped = total_seconds.max(0.0);
    let whole = clamped as u64;
    let millis = ((clamped - whole as f64) * 1000.0).round() as u64;

    // Rounding can carry into the next second, which would otherwise print
    // something like 00:00:07.1000.
    let (whole, millis) = if millis >= 1000 {
        (whole + 1, 0)
    } else {
        (whole, millis)
    };

    format!(
        "{:02}:{:02}:{:02}.{:03}",
        whole / 3600,
        (whole % 3600) / 60,
        whole % 60,
        millis
    )
}

/// What the clock reads on a frame, given when the stream began.
///
/// `unix_start` is seconds since the epoch. Taking it as a number rather than a
/// system call keeps this testable and keeps every frame's reading derived from
/// its own position rather than from when it happened to be encoded.
pub fn reading(unix_start: f64, frame: u64, fps: i32) -> Reading {
    let elapsed = frame as f64 / f64::from(fps.max(1));
    let wall = unix_start + elapsed;

    Reading {
        // Seconds within the day, which is all a clock face needs.
        wall: as_timecode(wall % 86_400.0),
        elapsed: as_timecode(elapsed),
        frame,
    }
}

/// Draw the clock into a canvas sized for the display.
///
/// Returns the canvas and where to put it, so the caller composites without
/// having to know the layout.
pub fn render(reading: &Reading, display_width: i32, display_height: i32) -> (Canvas, i32, i32) {
    let font_size = (display_height as f32 / 22.0).round().max(12.0);
    let padding = (display_height as f32 * 0.015).round().max(6.0) as i32;

    // The clock reads left to right and sits in a corner, so it is not centred
    // and not capped to the display.
    let canvas = crate::captions::panel::render(
        &reading.lines(),
        font_size,
        padding,
        crate::captions::panel::Align::Left,
        None,
    );

    let margin = (display_height as f64 * MARGIN_RATIO).round() as i32;
    // Top right, away from the bottom where subtitles sit.
    let x = display_width - canvas.width - margin;
    (canvas, x.max(0), margin)
}

/// Slots the clock uses, exposed so a test can assert the panel drew.
pub const PANEL: u8 = ink::BACKGROUND;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timecode_has_hours_minutes_seconds_and_milliseconds() {
        assert_eq!(as_timecode(0.0), "00:00:00.000");
        assert_eq!(as_timecode(1.5), "00:00:01.500");
        assert_eq!(as_timecode(3661.25), "01:01:01.250");
    }

    #[test]
    fn rounding_up_carries_into_the_next_second() {
        // Without the carry this prints 00:00:00.1000, which is nonsense.
        assert_eq!(as_timecode(0.9999), "00:00:01.000");
    }

    #[test]
    fn a_negative_time_clamps_rather_than_wrapping() {
        assert_eq!(as_timecode(-5.0), "00:00:00.000");
    }

    #[test]
    fn elapsed_time_follows_the_frame_number() {
        let reading = reading(0.0, 50, 25);
        assert_eq!(reading.elapsed, "00:00:02.000");
        assert_eq!(reading.frame, 50);
    }

    #[test]
    fn the_wall_clock_advances_with_the_stream() {
        let start = 3600.0;
        let first = reading(start, 0, 25);
        let later = reading(start, 250, 25);

        assert_eq!(first.wall, "01:00:00.000");
        assert_eq!(later.wall, "01:00:10.000");
    }

    #[test]
    fn the_wall_clock_wraps_at_midnight() {
        // One second before midnight, plus two seconds.
        let reading = reading(86_399.0, 50, 25);
        assert_eq!(reading.wall, "00:00:01.000");
    }

    #[test]
    fn the_clock_sits_inside_the_display() {
        let (canvas, x, y) = render(&reading(0.0, 0, 25), 1280, 720);
        assert!(x >= 0 && y >= 0);
        assert!(x + canvas.width <= 1280, "clock runs off the right edge");
        assert!(y + canvas.height <= 720, "clock runs off the bottom edge");
    }

    #[test]
    fn the_clock_actually_draws_something() {
        let (canvas, _, _) = render(&reading(0.0, 0, 25), 1280, 720);
        let glyph_pixels = canvas
            .pixels
            .iter()
            .filter(|slot| **slot >= ink::TEXT_BASE)
            .count();
        assert!(glyph_pixels > 0, "no digits were drawn");
    }

    #[test]
    fn the_clock_scales_with_the_display() {
        let (small, _, _) = render(&reading(0.0, 0, 25), 640, 360);
        let (large, _, _) = render(&reading(0.0, 0, 25), 1920, 1080);
        assert!(large.height > small.height);
    }
}
