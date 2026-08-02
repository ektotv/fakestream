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

use super::libcaption::{self, CaptionError, Channel, Triplet};
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

/// The cues for one caption channel.
///
/// Channels share a single stream of triplets, each tagged with the channel it
/// belongs to, so a player picks out the one a viewer selected. Two channels
/// therefore take twice as many frames to transmit as one.
#[derive(Debug, Clone)]
pub struct ChannelCues {
    pub channel: Channel,
    pub cues: Vec<Cue>,
}

/// Lay every cue onto a frame timeline, one triplet per frame.
///
/// Transmission is anchored so the end-of-caption code lands exactly on the
/// frame where the cue should appear, and earlier pairs fill backwards. Slots
/// already taken push transmission earlier rather than delaying the caption,
/// which is what keeps a run of cues all landing on time.
pub fn schedule(
    channels: &[ChannelCues],
    fps: i32,
    total_frames: i64,
) -> Result<Timeline, CaptionError> {
    let mut frames: Vec<Option<Triplet>> = vec![None; total_frames.max(0) as usize];

    // Clears go down first, across every channel. Their timing is not critical,
    // and reserving them now lets the captions placed next route around them.
    for channel in channels {
        let clear = libcaption::clear(channel.channel);
        for cue in &channel.cues {
            let clear_frame = ((cue.start + cue.duration) * f64::from(fps)).round() as i64;
            place_forwards(&mut frames, clear_frame, &clear);
        }
    }

    for channel in channels {
        for cue in &channel.cues {
            let triplets = libcaption::encode(&cue.flattened(), channel.channel)?;
            let Some(anchor) = libcaption::display_offset(&triplets) else {
                continue;
            };

            let display_frame = (cue.start * f64::from(fps)).round() as i64;
            place_anchored(&mut frames, display_frame, &triplets, anchor);
        }
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

    // Only one triplet rides per frame, so if something already holds the
    // requested frame the caption displays on the next free one. This happens
    // when two channels ask to display at the same instant, and silently
    // overwriting instead would lose a whole caption.
    let mut anchor_index = anchor_frame as usize;
    while anchor_index < frames.len() && frames[anchor_index].is_some() {
        anchor_index += 1;
    }
    if anchor_index >= frames.len() {
        return;
    }

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

    fn on_one(cues: Vec<Cue>) -> Vec<ChannelCues> {
        vec![ChannelCues {
            channel: Channel::One,
            cues,
        }]
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
        let timeline = schedule(&on_one(vec![subject.clone()]), 25, 250).expect("schedules");

        let triplets = libcaption::encode(&subject.flattened(), Channel::One).expect("encodes");
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

        let timeline =
            schedule(&on_one(cues.clone()), fps, 30 * i64::from(fps)).expect("schedules");

        for subject in &cues {
            let triplets = libcaption::encode(&subject.flattened(), Channel::One).expect("encodes");
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
        let timeline = schedule(&on_one(cues.clone()), 25, 250).expect("schedules");

        let expected: usize = cues
            .iter()
            .map(|subject| {
                libcaption::encode(&subject.flattened(), Channel::One)
                    .expect("encodes")
                    .len()
            })
            .sum::<usize>()
            + libcaption::clear(Channel::One).len() * cues.len();

        assert_eq!(
            timeline.occupied(),
            expected,
            "some triplets were lost to a collision"
        );
    }

    #[test]
    fn both_channels_reach_the_timeline() {
        // ffmpeg's decoder only ever reads channel one, so nothing downstream
        // can tell us channel two arrived. This checks the stream we produce
        // actually carries control words tagged for both.
        let channels = vec![
            ChannelCues {
                channel: Channel::One,
                cues: vec![cue("first channel", 3.0)],
            },
            ChannelCues {
                channel: Channel::Two,
                cues: vec![cue("second channel", 4.5)],
            },
        ];

        let timeline = schedule(&channels, 25, 250).expect("schedules");

        let mut saw_one = false;
        let mut saw_two = false;
        for frame in 0..timeline.len() {
            let stripped = timeline.at(frame)[1] & 0x7F;
            if (0x10..=0x17).contains(&stripped) {
                saw_one = true;
            }
            if (0x18..=0x1F).contains(&stripped) {
                saw_two = true;
            }
        }

        assert!(saw_one, "no channel one control words were transmitted");
        assert!(saw_two, "no channel two control words were transmitted");
    }

    #[test]
    fn channels_asking_to_display_at_once_both_survive() {
        // Both channels ride one stream of byte pairs, so only one caption can
        // become visible on any given frame. The second has to shift rather
        // than overwrite the first, which would lose a caption entirely.
        let channels = vec![
            ChannelCues {
                channel: Channel::One,
                cues: vec![cue("first", 3.0)],
            },
            ChannelCues {
                channel: Channel::Two,
                cues: vec![cue("second", 3.0)],
            },
        ];

        let timeline = schedule(&channels, 25, 250).expect("schedules");
        let occupied = timeline.occupied();

        let one = libcaption::encode("first", Channel::One)
            .expect("encodes")
            .len()
            + libcaption::clear(Channel::One).len();
        let two = libcaption::encode("second", Channel::Two)
            .expect("encodes")
            .len()
            + libcaption::clear(Channel::Two).len();

        assert_eq!(occupied, one + two, "channels overwrote each other");
    }

    #[test]
    fn a_cue_beyond_the_clip_is_dropped_rather_than_panicking() {
        let timeline = schedule(&on_one(vec![cue("too late", 999.0)]), 25, 250).expect("schedules");
        assert_eq!(timeline.len(), 250);
    }
}
