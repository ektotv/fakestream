//! Generating caption text.
//!
//! Lorem ipsum, cut into cues on a fixed cadence so a viewer can tell at a
//! glance whether captions are keeping pace with the picture.

/// One caption, in seconds against the clip timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub duration: f64,
    /// Already wrapped. Each entry is one displayed line.
    pub lines: Vec<String>,
}

impl Cue {
    /// The whole cue as a single line, for formats that do their own wrapping.
    pub fn flattened(&self) -> String {
        self.lines.join(" ")
    }
}

const LOREM: &str = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do \
eiusmod tempor incididunt ut labore et dolore magna aliqua Ut enim ad minim veniam \
quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat \
Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu \
fugiat nulla pariatur Excepteur sint occaecat cupidatat non proident sunt in culpa \
qui officia deserunt mollit anim id est laborum";

/// How wide a line may be. CEA-608 rows hold 32 characters and silently
/// truncate beyond that, so it is the tightest constraint we have and every
/// format is wrapped to it for consistency between fixtures.
pub const LINE_LENGTH: usize = 32;

/// Cues covering `duration_seconds`, one every `interval` seconds.
///
/// Each cue is numbered so a viewer can see at a glance whether any were
/// dropped, and whether the one on screen belongs to the moment it is over.
///
/// The first cue lands one interval in rather than at zero. CEA-608 sends one
/// byte pair per frame, so a caption at zero cannot finish transmitting before
/// the moment it should appear and always shows late. Starting later keeps
/// every fixture honest, and gives a player a moment to settle before captions
/// begin.
pub fn lorem_cues(duration_seconds: f64, interval: f64, visible: f64) -> Vec<Cue> {
    let words: Vec<&str> = LOREM.split_whitespace().collect();
    // One fewer, since cues start at the first interval rather than at zero.
    let count = (duration_seconds / interval).floor() as usize - 1;

    let mut cues = Vec::with_capacity(count);
    let mut cursor = 0usize;

    for index in 0..count {
        let label = format!("{}.", index + 1);
        let mut taken = Vec::new();
        // Six words is roughly two full 608 rows, which is what a real caption
        // tends to be.
        for _ in 0..6 {
            taken.push(words[cursor % words.len()]);
            cursor += 1;
        }

        let sentence = format!("{label} {}", taken.join(" "));
        cues.push(Cue {
            start: (index + 1) as f64 * interval,
            duration: visible.min(interval),
            lines: wrap(&sentence, LINE_LENGTH),
        });
    }

    cues
}

/// Shift a set of cues in time and relabel them.
///
/// Used to put a second caption channel alongside the first without the two
/// transmitting at the same moment, since channels share one stream of byte
/// pairs and interleaving them would slow both down.
pub fn offset_cues(cues: Vec<Cue>, shift: f64, label: &str) -> Vec<Cue> {
    cues.into_iter()
        .map(|cue| {
            let text = format!("{label} {}", cue.flattened());
            Cue {
                start: cue.start + shift,
                duration: cue.duration,
                lines: wrap(&text, LINE_LENGTH),
            }
        })
        .collect()
}

/// Break text into lines no longer than `width`, on word boundaries.
///
/// A word longer than the limit is placed on its own line rather than split,
/// since splitting mid-word reads worse than overflowing, and the caller has
/// already chosen the vocabulary.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_cover_the_clip_at_the_asked_for_cadence() {
        let cues = lorem_cues(10.0, 2.0, 1.5);
        assert_eq!(cues.len(), 4);
        assert_eq!(cues[0].start, 2.0);
        assert_eq!(cues[3].start, 8.0);
    }

    #[test]
    fn nothing_is_scheduled_at_zero() {
        // A CEA-608 caption at zero cannot finish transmitting in time and
        // always appears late, which reads as a broken fixture.
        for cue in lorem_cues(30.0, 3.0, 2.5) {
            assert!(cue.start > 0.0);
        }
    }

    #[test]
    fn no_cue_runs_past_the_clip() {
        let duration = 30.0;
        for cue in lorem_cues(duration, 3.0, 2.5) {
            assert!(cue.start + cue.duration <= duration);
        }
    }

    #[test]
    fn a_cue_never_outlasts_its_slot() {
        // Asking for a longer visible time than the gap must not overlap the
        // next cue, or two captions fight for the screen.
        let cues = lorem_cues(10.0, 2.0, 5.0);
        for cue in &cues {
            assert!(cue.duration <= 2.0);
        }
    }

    #[test]
    fn every_line_fits_a_608_row() {
        for cue in lorem_cues(60.0, 2.0, 1.5) {
            for line in &cue.lines {
                assert!(
                    line.chars().count() <= LINE_LENGTH,
                    "{line:?} is {} characters",
                    line.chars().count()
                );
            }
        }
    }

    #[test]
    fn cues_are_numbered_so_drops_are_visible() {
        let cues = lorem_cues(12.0, 2.0, 1.5);
        assert!(cues[0].flattened().starts_with("1."));
        assert!(cues[1].flattened().starts_with("2."));
        assert!(cues[2].flattened().starts_with("3."));
    }

    #[test]
    fn offsetting_shifts_and_relabels() {
        let original = lorem_cues(12.0, 3.0, 2.5);
        let shifted = offset_cues(original.clone(), 1.5, "CC2:");

        assert_eq!(shifted.len(), original.len());
        assert_eq!(shifted[0].start, original[0].start + 1.5);
        assert!(shifted[0].flattened().starts_with("CC2:"));
    }

    #[test]
    fn offset_cues_still_fit_a_608_row() {
        for cue in offset_cues(lorem_cues(60.0, 3.0, 2.5), 1.5, "CC2:") {
            for line in &cue.lines {
                assert!(line.chars().count() <= LINE_LENGTH, "{line:?} is too wide");
            }
        }
    }

    #[test]
    fn wrapping_breaks_on_words() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
    }

    #[test]
    fn an_overlong_word_gets_its_own_line() {
        assert_eq!(
            wrap("a supercalifragilistic b", 8),
            vec!["a", "supercalifragilistic", "b"]
        );
    }
}
