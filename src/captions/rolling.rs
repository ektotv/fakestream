//! Captions for a stream with no end.
//!
//! The VOD scheduler lays every cue onto a timeline of known length. A live
//! stream has no such timeline, so cues are produced as their moment
//! approaches and transmitted from a queue.
//!
//! Each caption carries the time it is due to appear, which is the same time
//! the on-screen clock shows on that frame. Caption timing then becomes
//! checkable by eye: if the caption says 15:45:22 while the clock says
//! 15:45:24, the player is two seconds out on captions.

use super::libcaption::{self, CaptionError, Channel, Triplet};
use super::script::Cue;
use std::collections::BTreeMap;

/// How long before its display time a caption starts being prepared.
///
/// Only has to exceed the longest caption's transmission, which for two rows is
/// well under a second at one byte pair per frame.
const PREPARE_SECONDS: f64 = 2.0;

/// Words cycled through so successive captions differ visibly.
const WORDS: [&str; 8] = [
    "Lorem ipsum dolor",
    "sit amet consectetur",
    "adipiscing elit sed",
    "do eiusmod tempor",
    "incididunt ut labore",
    "et dolore magna",
    "aliqua Ut enim",
    "ad minim veniam",
];

/// Produces caption data for an endless stream, one triplet per frame.
pub struct RollingCaptions {
    fps: i32,
    /// Seconds between captions appearing.
    interval: f64,
    /// How long each stays up.
    visible: f64,
    channel: Channel,

    /// Prepared triplets, keyed by the frame each belongs on.
    ///
    /// A map rather than a list because placement has to know which frames are
    /// already taken. Only one byte pair rides per frame, so a caption whose
    /// slots collide with the previous caption's clear must route around them
    /// rather than be pushed later, which showed up as every caption after the
    /// first drifting two frames late.
    queue: BTreeMap<u64, Triplet>,
    /// Which caption is next to be prepared.
    next_caption: u64,
}

impl RollingCaptions {
    pub fn new(fps: i32, interval: f64, visible: f64, channel: Channel) -> Self {
        Self {
            fps: fps.max(1),
            interval,
            visible: visible.min(interval),
            channel,
            queue: BTreeMap::new(),
            // Number one appears one interval in, since a caption at zero
            // cannot finish transmitting before the moment it should appear.
            next_caption: 1,
        }
    }

    /// The triplet to attach to a frame, preparing more work if it is time.
    ///
    /// `unix_start` is when the stream began, used only to put a readable clock
    /// time in the caption text.
    pub fn triplet_for(&mut self, frame: u64, unix_start: f64) -> Result<Triplet, CaptionError> {
        self.prepare_if_due(frame, unix_start)?;

        if let Some(triplet) = self.queue.remove(&frame) {
            return Ok(triplet);
        }

        // Anything left behind by a frame that has already passed would sit in
        // the map for ever, so it is dropped rather than delivered late.
        while let Some(stale) = self.queue.keys().next().copied() {
            if stale >= frame {
                break;
            }
            self.queue.remove(&stale);
        }

        Ok(super::cea608::NULL_TRIPLET)
    }

    /// Encode the next caption once its preparation window opens.
    fn prepare_if_due(&mut self, frame: u64, unix_start: f64) -> Result<(), CaptionError> {
        let display_seconds = self.next_caption as f64 * self.interval;
        let prepare_at =
            ((display_seconds - PREPARE_SECONDS) * f64::from(self.fps)).max(0.0) as u64;

        if frame < prepare_at {
            return Ok(());
        }

        let cue = self.cue_for(self.next_caption, display_seconds, unix_start);
        let triplets = libcaption::encode(&cue.flattened(), self.channel)?;

        // Anchor the end-of-caption code on the display frame and work
        // backwards, exactly as the VOD scheduler does, so the caption appears
        // when it says it will rather than when transmission happens to finish.
        if let Some(anchor) = libcaption::display_offset(&triplets) {
            let display_frame = (display_seconds * f64::from(self.fps)).round() as u64;
            self.place_anchored(display_frame, &triplets, anchor);

            // Clear the screen when the caption's time is up.
            let clear_frame =
                ((display_seconds + self.visible) * f64::from(self.fps)).round() as u64;
            self.place_forwards(clear_frame, &libcaption::clear(self.channel));
        }

        self.next_caption += 1;
        Ok(())
    }

    /// Place triplets so that `triplets[anchor]` lands exactly on
    /// `anchor_frame`, filling backwards for everything before it.
    ///
    /// Frames already taken push transmission earlier rather than delaying the
    /// caption, which is what keeps a run of captions all landing on time.
    fn place_anchored(&mut self, anchor_frame: u64, triplets: &[Triplet], anchor: usize) {
        let mut cursor = anchor_frame;
        while self.queue.contains_key(&cursor) {
            cursor += 1;
        }
        self.queue.insert(cursor, triplets[anchor]);
        let placed_at = cursor;

        for triplet in triplets[..anchor].iter().rev() {
            loop {
                if cursor == 0 {
                    return;
                }
                cursor -= 1;
                if !self.queue.contains_key(&cursor) {
                    break;
                }
            }
            self.queue.insert(cursor, *triplet);
        }

        self.place_forwards(placed_at + 1, &triplets[anchor + 1..]);
    }

    /// Place triplets on the next free frames from `from` onwards.
    fn place_forwards(&mut self, from: u64, triplets: &[Triplet]) {
        let mut cursor = from;
        for triplet in triplets {
            while self.queue.contains_key(&cursor) {
                cursor += 1;
            }
            self.queue.insert(cursor, *triplet);
            cursor += 1;
        }
    }

    /// Build the caption text, carrying the time it is due to appear.
    fn cue_for(&self, number: u64, display_seconds: f64, unix_start: f64) -> Cue {
        let wall = (unix_start + display_seconds) % 86_400.0;
        let whole = wall as u64;

        let stamp = format!(
            "{:02}:{:02}:{:02}",
            whole / 3600,
            (whole % 3600) / 60,
            whole % 60
        );
        let words = WORDS[(number as usize) % WORDS.len()];

        Cue {
            start: display_seconds,
            duration: self.visible,
            // Two rows: the time, then the words. Both fit a 608 row easily.
            lines: vec![stamp, format!("{number}. {words}")],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rolling() -> RollingCaptions {
        RollingCaptions::new(25, 3.0, 2.5, Channel::One)
    }

    #[test]
    fn nothing_is_sent_before_the_first_caption_is_prepared() {
        let mut captions = rolling();
        // The first caption is due at three seconds and prepared at one, so
        // frame zero has nothing to say.
        let triplet = captions.triplet_for(0, 0.0).expect("a triplet");
        assert_eq!(triplet, super::super::cea608::NULL_TRIPLET);
    }

    #[test]
    fn a_caption_is_transmitted_before_its_display_time() {
        let mut captions = rolling();
        let display_frame = 75; // three seconds at 25fps

        let mut sent = 0;
        for frame in 0..display_frame {
            if captions.triplet_for(frame, 0.0).expect("a triplet")
                != super::super::cea608::NULL_TRIPLET
            {
                sent += 1;
            }
        }

        assert!(sent > 0, "the caption never started transmitting");
    }

    #[test]
    fn the_caption_lands_on_its_display_frame() {
        let mut captions = rolling();
        let display_frame = 75u64;

        let mut last = super::super::cea608::NULL_TRIPLET;
        for frame in 0..=display_frame {
            last = captions.triplet_for(frame, 0.0).expect("a triplet");
        }

        let expected = libcaption::encode(&captions.cue_for(1, 3.0, 0.0).flattened(), Channel::One)
            .expect("encodes");
        let anchor = libcaption::display_offset(&expected).expect("has a display point");

        assert_eq!(
            last, expected[anchor],
            "end of caption did not land on the display frame"
        );
    }

    #[test]
    fn a_run_of_captions_all_land_exactly() {
        // The first caption always landed correctly. It was every one after it
        // that drifted, because the previous caption's clear took the slots the
        // next one needed and placement pushed it later.
        let mut captions = rolling();
        let mut landed = Vec::new();

        for frame in 0..(25 * 30) {
            let triplet = captions.triplet_for(frame, 0.0).expect("a triplet");
            // End of caption, on either channel, is what makes a caption show.
            if triplet[1] & 0x7F & !0x08 == 0x14 && triplet[2] & 0x7F == 0x2F {
                landed.push(frame);
            }
        }

        // Two per caption, since the code is doubled. The first of each pair is
        // what the decoder acts on.
        let displayed: Vec<u64> = landed.chunks(2).map(|pair| pair[0]).collect();
        assert!(
            displayed.len() >= 8,
            "only {} captions displayed",
            displayed.len()
        );

        for (index, frame) in displayed.iter().enumerate() {
            let expected = ((index as f64 + 1.0) * 3.0 * 25.0).round() as u64;
            assert_eq!(
                *frame,
                expected,
                "caption {} displayed on frame {frame}, expected {expected}",
                index + 1
            );
        }
    }

    #[test]
    fn it_keeps_producing_captions_indefinitely() {
        let mut captions = rolling();
        let mut seen = 0;

        // Two minutes of frames, which is well past anything precomputed.
        for frame in 0..(25 * 120) {
            if captions.triplet_for(frame, 0.0).expect("a triplet")
                != super::super::cea608::NULL_TRIPLET
            {
                seen += 1;
            }
        }

        assert!(seen > 100, "only {seen} triplets over two minutes");
        assert!(
            captions.next_caption > 30,
            "captions stopped being prepared"
        );
    }

    #[test]
    fn the_queue_does_not_grow_without_bound() {
        let mut captions = rolling();
        for frame in 0..(25 * 120) {
            captions.triplet_for(frame, 0.0).expect("a triplet");
        }

        // Only the caption being prepared should still be waiting, so a stream
        // running for days cannot accumulate a queue.
        assert!(
            captions.queue.len() < 64,
            "{} triplets left queued",
            captions.queue.len()
        );
    }

    #[test]
    fn the_caption_carries_the_time_it_will_appear() {
        let captions = rolling();
        // One hour in, caption one is due three seconds later.
        let cue = captions.cue_for(1, 3.0, 3600.0);
        assert_eq!(cue.lines[0], "01:00:03");
    }

    #[test]
    fn successive_captions_differ() {
        let captions = rolling();
        let first = captions.cue_for(1, 3.0, 0.0).flattened();
        let second = captions.cue_for(2, 6.0, 0.0).flattened();
        assert_ne!(first, second);
    }
}
