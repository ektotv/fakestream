//! The caption source a muxer drives, one interface for every frame.
//!
//! CEA-608 and 708 share a single `cc_data` array in the video's SEI, 608 pair
//! first, then any 708 pairs. That packing, and the choice between a fixed cue
//! list and an endless rolling generator, used to be hand-rolled at each muxer.
//! It lives here now: a caller asks for a frame's bytes and attaches them,
//! nothing more.
//!
//! A [`CaptionPlan`] says what to carry; a [`CaptionFeed`] built from it yields
//! the bytes. The finite plan schedules cue lists onto a timeline of known
//! length, for VOD. The rolling plan generates captions endlessly, for live.

use super::cea608::{self, ChannelCues, Timeline};
use super::cea708::{self, Schedule};
use super::libcaption::{CaptionError, Channel};
use super::rolling::{RollingCaptions, RollingDtvcc};
use super::script::Cue;

/// Seconds between captions appearing on a rolling stream.
const ROLLING_INTERVAL_SECONDS: f64 = 3.0;

/// How long each rolling caption stays up, leaving a visible gap before the
/// next.
const ROLLING_VISIBLE_SECONDS: f64 = 2.5;

/// What captions a clip carries, and in which timing model.
///
/// One field replaces the four flat ones a spec used to hold, so a clip cannot
/// ask for VOD and live captions at once, and every consumer reads the same
/// place.
#[derive(Debug, Clone, Default)]
pub enum CaptionPlan {
    /// No captions.
    #[default]
    None,
    /// A fixed cue list scheduled onto a timeline of known length. For VOD.
    Finite {
        cea608: Vec<ChannelCues>,
        cea708: Vec<Cue>,
    },
    /// Endlessly generated captions with no timeline. For live streams.
    Rolling {
        /// The 608 channel to carry, or None for no 608.
        cea608: Option<Channel>,
        /// Whether to carry a 708 service.
        cea708: bool,
    },
}

impl CaptionPlan {
    /// Whether this plan carries any CEA-608.
    pub fn has_608(&self) -> bool {
        match self {
            Self::None => false,
            Self::Finite { cea608, .. } => !cea608.is_empty(),
            Self::Rolling { cea608, .. } => cea608.is_some(),
        }
    }

    /// Whether this plan carries any CEA-708.
    pub fn has_708(&self) -> bool {
        match self {
            Self::None => false,
            Self::Finite { cea708, .. } => !cea708.is_empty(),
            Self::Rolling { cea708, .. } => *cea708,
        }
    }
}

/// The per-frame caption source. Ask [`cc_data`](Self::cc_data) for a frame's
/// bytes and attach them to the picture; the 608-then-708 packing is inside.
pub struct CaptionFeed {
    inner: Inner,
}

enum Inner {
    None,
    /// VOD: cue lists scheduled up front against a known frame count.
    Finite {
        line21: Option<Timeline>,
        dtvcc: Schedule,
    },
    /// Live: rolling generators producing on demand.
    Rolling {
        line21: Option<RollingCaptions>,
        dtvcc: Option<RollingDtvcc>,
        /// When the stream began, only used to stamp the readable clock in the
        /// rolling caption text.
        unix_start: f64,
    },
}

impl CaptionFeed {
    /// Build a feed for a plan.
    ///
    /// `total_frames` sizes the finite timelines and is ignored by a rolling
    /// plan; `unix_start` stamps rolling captions and is ignored by a finite
    /// one. So a caller passes what its delivery knows and leaves the other at
    /// a default.
    pub fn build(
        plan: &CaptionPlan,
        fps: i32,
        total_frames: i64,
        unix_start: f64,
    ) -> Result<Self, CaptionError> {
        let inner = match plan {
            CaptionPlan::None => Inner::None,
            CaptionPlan::Finite { cea608, cea708 } => {
                let line21 = if cea608.is_empty() {
                    None
                } else {
                    Some(cea608::schedule(cea608, fps, total_frames)?)
                };
                let dtvcc = cea708::schedule(cea708, fps, total_frames);
                Inner::Finite { line21, dtvcc }
            }
            CaptionPlan::Rolling { cea608, cea708 } => {
                let line21 = cea608.map(|channel| {
                    RollingCaptions::new(
                        fps,
                        ROLLING_INTERVAL_SECONDS,
                        ROLLING_VISIBLE_SECONDS,
                        channel,
                    )
                });
                let dtvcc = cea708.then(|| {
                    RollingDtvcc::new(fps, ROLLING_INTERVAL_SECONDS, ROLLING_VISIBLE_SECONDS)
                });
                Inner::Rolling {
                    line21,
                    dtvcc,
                    unix_start,
                }
            }
        };
        Ok(Self { inner })
    }

    /// The `cc_data` bytes for a frame: the 608 pair first, then any 708 pairs,
    /// all whole triplets. Empty when there is nothing to send on this frame.
    pub fn cc_data(&mut self, frame: u64) -> Result<Vec<u8>, CaptionError> {
        let mut data = Vec::new();
        match &mut self.inner {
            Inner::None => {}
            Inner::Finite { line21, dtvcc } => {
                if let Some(line21) = line21 {
                    data.extend_from_slice(&line21.at(frame as usize));
                }
                for triplet in dtvcc.at(frame as usize) {
                    data.extend_from_slice(triplet);
                }
            }
            Inner::Rolling {
                line21,
                dtvcc,
                unix_start,
            } => {
                if let Some(line21) = line21 {
                    data.extend_from_slice(&line21.triplet_for(frame, *unix_start)?);
                }
                if let Some(dtvcc) = dtvcc {
                    for triplet in dtvcc.triplets_for(frame, *unix_start) {
                        data.extend_from_slice(&triplet);
                    }
                }
            }
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captions::script::lorem_cues;

    fn channel(channel: Channel) -> ChannelCues {
        ChannelCues {
            channel,
            cues: lorem_cues(4.0, 1.0, 0.8),
        }
    }

    #[test]
    fn no_plan_sends_nothing() {
        let mut feed = CaptionFeed::build(&CaptionPlan::None, 25, 100, 0.0).expect("build");
        for frame in 0..100 {
            assert!(feed.cc_data(frame).expect("cc_data").is_empty());
        }
    }

    #[test]
    fn finite_608_sends_one_triplet_per_frame() {
        let plan = CaptionPlan::Finite {
            cea608: vec![channel(Channel::One)],
            cea708: vec![],
        };
        let mut feed = CaptionFeed::build(&plan, 25, 100, 0.0).expect("build");
        for frame in 0..100 {
            // One 608 pair rides per frame, a null pair when idle.
            assert_eq!(feed.cc_data(frame).expect("cc_data").len(), 3);
        }
    }

    #[test]
    fn finite_608_and_708_packs_608_first() {
        let plan = CaptionPlan::Finite {
            cea608: vec![channel(Channel::One)],
            cea708: lorem_cues(4.0, 1.0, 0.8),
        };
        let mut feed = CaptionFeed::build(&plan, 25, 100, 0.0).expect("build");

        let mut saw_708 = false;
        for frame in 0..100 {
            let data = feed.cc_data(frame).expect("cc_data");
            // 608 always leads with its single pair; 708 pairs, when present,
            // follow and make the buffer longer than three bytes.
            assert!(data.len() >= 3 && data.len().is_multiple_of(3));
            if data.len() > 3 {
                saw_708 = true;
            }
        }
        assert!(saw_708, "708 pairs never rode alongside the 608 pair");
    }

    #[test]
    fn rolling_608_and_708_produces_captions_endlessly() {
        let plan = CaptionPlan::Rolling {
            cea608: Some(Channel::One),
            cea708: true,
        };
        let mut feed = CaptionFeed::build(&plan, 25, 0, 0.0).expect("build");

        let mut bytes = 0;
        // Two minutes, well past any finite timeline.
        for frame in 0..(25 * 120) {
            let data = feed.cc_data(frame).expect("cc_data");
            assert!(
                data.len().is_multiple_of(3),
                "cc_data must be whole triplets"
            );
            bytes += data.len();
        }
        assert!(bytes > 100, "the rolling feed produced almost nothing");
    }

    #[test]
    fn has_608_and_has_708_read_the_plan() {
        assert!(!CaptionPlan::None.has_608());
        assert!(!CaptionPlan::None.has_708());

        let rolling = CaptionPlan::Rolling {
            cea608: Some(Channel::One),
            cea708: false,
        };
        assert!(rolling.has_608());
        assert!(!rolling.has_708());

        let finite = CaptionPlan::Finite {
            cea608: vec![],
            cea708: lorem_cues(4.0, 1.0, 0.8),
        };
        assert!(!finite.has_608());
        assert!(finite.has_708());
    }
}
