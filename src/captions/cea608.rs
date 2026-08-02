//! Scheduling CEA-608 captions onto a frame timeline.
//!
//! libcaption owns the encoding, meaning the character map, the control code
//! sequences and screen layout. This module owns placement, because when a
//! caption is transmitted decides when it appears, and that is a property of
//! the clip rather than of the caption.
//!
//! Two properties of 608 shape everything here. One byte pair rides per frame,
//! so a caption only appears once its whole run has been sent, meaning
//! transmission has to begin before the moment it should be on screen. And a
//! caption at time zero can never be on time, since there is nowhere earlier to
//! start.

use super::libcaption::{self, CaptionError, Triplet};
use super::script::Cue;

/// A frame with nothing to send still carries a null pair, which keeps the
/// caption channel alive rather than letting a decoder time out.
pub const NULL_TRIPLET: Triplet = [0xFC, 0x80, 0x80];

/// Every frame's caption data for a clip.
pub struct Timeline {
    frames: Vec<Option<Triplet>>,
}

impl Timeline {
    /// The triplet to attach to a given frame.
    pub fn at(&self, frame: usize) -> Triplet {
        self.frames
            .get(frame)
            .copied()
            .flatten()
            .unwrap_or(NULL_TRIPLET)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// How many frames actually carry caption data, used in tests.
    #[cfg(test)]
    fn occupied(&self) -> usize {
        self.frames.iter().filter(|slot| slot.is_some()).count()
    }
}

/// Lay every cue onto a frame timeline, one triplet per frame.
///
/// Transmission is anchored so the end-of-caption code lands exactly on the
/// frame where the cue should appear, and earlier pairs fill backwards. Slots
/// already taken push transmission earlier rather than delaying the caption,
/// which is what keeps a run of cues all landing on time.
pub fn schedule(cues: &[Cue], fps: i32, total_frames: i64) -> Result<Timeline, CaptionError> {
    let mut frames: Vec<Option<Triplet>> = vec![None; total_frames.max(0) as usize];

    // Clears go down first. Their timing is not critical, and reserving them
    // now lets the captions placed next route around them.
    for cue in cues {
        let clear = libcaption::clear();
        let clear_frame = ((cue.start + cue.duration) * f64::from(fps)).round() as i64;
        place_forwards(&mut frames, clear_frame, &clear);
    }

    for cue in cues {
        let triplets = libcaption::encode(&cue.flattened())?;
        let Some(anchor) = libcaption::display_offset(&triplets) else {
            continue;
        };

        let display_frame = (cue.start * f64::from(fps)).round() as i64;
        place_anchored(&mut frames, display_frame, &triplets, anchor);
    }

    Ok(Timeline { frames })
}

/// Write triplets onto the next free frames from `from` onwards.
fn place_forwards(frames: &mut [Option<Triplet>], from: i64, triplets: &[Triplet]) {
    let mut cursor = from.max(0) as usize;
    for triplet in triplets {
        while cursor < frames.len() && frames[cursor].is_some() {
            cursor += 1;
        }
        if cursor >= frames.len() {
            return;
        }
        frames[cursor] = Some(*triplet);
        cursor += 1;
    }
}

/// Place `triplets` so that `triplets[anchor]` lands exactly on `anchor_frame`.
fn place_anchored(
    frames: &mut [Option<Triplet>],
    anchor_frame: i64,
    triplets: &[Triplet],
    anchor: usize,
) {
    if anchor_frame < 0 || anchor_frame as usize >= frames.len() {
        return;
    }

    let anchor_index = anchor_frame as usize;
    frames[anchor_index] = Some(triplets[anchor]);

    let mut cursor = anchor_index;
    for triplet in triplets[..anchor].iter().rev() {
        loop {
            if cursor == 0 {
                return;
            }
            cursor -= 1;
            if frames[cursor].is_none() {
                break;
            }
        }
        frames[cursor] = Some(*triplet);
    }

    place_forwards(frames, anchor_index as i64 + 1, &triplets[anchor + 1..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(text: &str, start: f64) -> Cue {
        Cue {
            start,
            duration: 2.5,
            lines: vec![text.to_string()],
        }
    }

    #[test]
    fn a_frame_with_nothing_to_send_carries_a_null() {
        let timeline = schedule(&[], 25, 100).expect("empty schedule");
        assert_eq!(timeline.at(0), NULL_TRIPLET);
    }

    #[test]
    fn reading_past_the_end_is_still_a_null() {
        let timeline = schedule(&[], 25, 10).expect("empty schedule");
        assert_eq!(timeline.at(999), NULL_TRIPLET);
    }

    #[test]
    fn the_caption_lands_on_the_frame_it_was_asked_for() {
        let subject = cue("Lorem ipsum dolor", 4.0);
        let timeline = schedule(std::slice::from_ref(&subject), 25, 250).expect("schedules");

        let triplets = libcaption::encode(&subject.flattened()).expect("encodes");
        let anchor = libcaption::display_offset(&triplets).expect("has a display point");

        assert_eq!(
            timeline.at(100),
            triplets[anchor],
            "end of caption should land on the display frame"
        );
    }

    #[test]
    fn a_run_of_cues_all_land_exactly() {
        // The single cue case hides the real bug. With several cues the clear
        // pairs of one land in the slots the next needs, and placing forwards
        // pushes every caption after the first two frames late.
        let fps = 25;
        let cues: Vec<Cue> = (1..=8)
            .map(|index| cue("lorem ipsum dolor", f64::from(index) * 3.0))
            .collect();

        let timeline = schedule(&cues, fps, 30 * i64::from(fps)).expect("schedules");

        for subject in &cues {
            let triplets = libcaption::encode(&subject.flattened()).expect("encodes");
            let anchor = libcaption::display_offset(&triplets).expect("has a display point");
            let display_frame = (subject.start * f64::from(fps)).round() as usize;

            assert_eq!(
                timeline.at(display_frame),
                triplets[anchor],
                "cue at {}s does not display on its own frame",
                subject.start
            );
        }
    }

    #[test]
    fn cues_do_not_overwrite_each_other() {
        let cues = vec![cue("first caption", 1.0), cue("second caption", 1.4)];
        let timeline = schedule(&cues, 25, 250).expect("schedules");

        let expected: usize = cues
            .iter()
            .map(|subject| {
                libcaption::encode(&subject.flattened())
                    .expect("encodes")
                    .len()
            })
            .sum::<usize>()
            + libcaption::clear().len() * cues.len();

        assert_eq!(
            timeline.occupied(),
            expected,
            "some triplets were lost to a collision"
        );
    }

    #[test]
    fn a_cue_beyond_the_clip_is_dropped_rather_than_panicking() {
        let timeline = schedule(&[cue("too late", 999.0)], 25, 250).expect("schedules");
        assert_eq!(timeline.len(), 250);
    }
}
