//! Synthetic picture and sound. Nothing here reads a file.

use super::MediaError;
use crate::media::ffi;
use rsmpeg::avutil::AVFrame;

/// How loud the beeps are, well below full scale so nothing clips.
const AMPLITUDE: f64 = 0.25;

/// Beeps on a repeating schedule rather than a continuous tone.
///
/// A steady tone is unpleasant to work alongside and tells you nothing. Short
/// bursts on each second boundary are easy to ignore, let you hear that
/// playback is progressing, and pair with the video marker to make audio and
/// video sync visible and audible at the same instant.
#[derive(Debug, Clone)]
pub struct Beeps {
    sample_rate: u32,
    /// Samples between the start of one beep and the next.
    interval: usize,
    /// How long each beep sounds.
    burst: usize,
    /// Fade in and out length, which stops the click a hard edge would make.
    ramp: usize,
    /// Every nth beep is the lower pitched one, so seconds can be counted.
    accent_every: usize,
    normal_hz: f64,
    accent_hz: f64,
}

impl Beeps {
    /// One beep per second, 80ms long, with a lower pitch every fifth second.
    pub fn every_second(sample_rate: u32) -> Self {
        let rate = sample_rate as usize;
        Self {
            sample_rate,
            interval: rate,
            burst: rate * 80 / 1000,
            ramp: rate * 5 / 1000,
            accent_every: 5,
            normal_hz: 1000.0,
            accent_hz: 600.0,
        }
    }

    /// The sample value at an absolute position in the timeline.
    ///
    /// Deriving from the absolute index rather than carrying state means frames
    /// can be generated in any order and always agree, and there is no drift to
    /// accumulate.
    fn sample_at(&self, index: usize) -> f32 {
        let position = index % self.interval;
        if position >= self.burst {
            return 0.0;
        }

        let beep_number = index / self.interval;
        let frequency = if beep_number % self.accent_every == 0 {
            self.accent_hz
        } else {
            self.normal_hz
        };

        // Starting each burst at zero phase keeps the waveform continuous
        // through the fade in.
        let phase = std::f64::consts::TAU * frequency * position as f64 / f64::from(self.sample_rate);
        (phase.sin() * AMPLITUDE * self.envelope(position)) as f32
    }

    /// Linear fade at both ends of a burst.
    fn envelope(&self, position: usize) -> f64 {
        if self.ramp == 0 {
            return 1.0;
        }
        let from_start = position;
        let from_end = self.burst.saturating_sub(position + 1);
        let edge = from_start.min(from_end);
        if edge >= self.ramp {
            1.0
        } else {
            edge as f64 / self.ramp as f64
        }
    }

    /// Is a beep sounding at this absolute sample? Used to place the matching
    /// video marker.
    pub fn beeping_at(&self, index: usize) -> bool {
        index % self.interval < self.burst
    }

    /// Fill one planar float frame starting from an absolute sample position.
    pub fn fill(
        &self,
        frame: &mut AVFrame,
        first_sample: usize,
        samples: usize,
        channels: usize,
    ) -> Result<(), MediaError> {
        let block: Vec<f32> = (0..samples).map(|offset| self.sample_at(first_sample + offset)).collect();
        let bytes = samples * size_of::<f32>();

        for channel in 0..channels {
            let plane = ffi::audio_plane(frame, channel, bytes)?;
            for (index, sample) in block.iter().enumerate() {
                let offset = index * size_of::<f32>();
                plane[offset..offset + size_of::<f32>()].copy_from_slice(&sample.to_ne_bytes());
            }
        }

        Ok(())
    }
}

/// Paint a moving test pattern into a YUV420P frame.
///
/// The pattern shifts with `phase` so successive frames genuinely differ. A
/// static picture would let the encoder collapse the whole clip into one tiny
/// keyframe, which makes a poor test asset.
///
/// When `marker` is set a bright block is drawn in the corner. It appears on
/// exactly the frames where a beep sounds, so audio and video sync can be
/// checked by eye and ear together.
pub fn paint_pattern(
    frame: &mut AVFrame,
    width: usize,
    height: usize,
    phase: usize,
    marker: bool,
) -> Result<(), MediaError> {
    let mut luma = ffi::plane_writer(frame, 0, width, height)?;
    for y in 0..height {
        let row = luma.row(y);
        for (x, pixel) in row.iter_mut().enumerate() {
            *pixel = (((x + phase) ^ y) & 0xFF) as u8;
        }
    }

    let marker_area = marker.then(|| {
        let size = (height / 8).max(8);
        let margin = size / 2;
        (margin, size)
    });

    if let Some((margin, size)) = marker_area {
        for y in margin..(margin + size).min(height) {
            let row = luma.row(y);
            let end = (margin + size).min(width);
            row[margin..end].fill(235);
        }
    }

    // Chroma planes are half resolution in YUV420P. Holding them steady keeps
    // the colour flat while the luma pattern moves.
    for (plane, level) in [(1usize, 128u8), (2usize, 100u8)] {
        let mut chroma = ffi::plane_writer(frame, plane, width / 2, height / 2)?;
        chroma.fill(level);

        // Neutral chroma under the marker, otherwise raising luma alone tints
        // it with whatever colour the rest of the picture carries and the
        // marker reads as a pale patch rather than a clear white flash.
        if let Some((margin, size)) = marker_area {
            let half_margin = margin / 2;
            let half_size = size / 2;
            for y in half_margin..(half_margin + half_size).min(height / 2) {
                let row = chroma.row(y);
                let end = (half_margin + half_size).min(width / 2);
                row[half_margin..end].fill(128);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_between_beeps() {
        let beeps = Beeps::every_second(48_000);
        // Half a second in, well clear of the 80ms burst.
        assert_eq!(beeps.sample_at(24_000), 0.0);
        assert!(!beeps.beeping_at(24_000));
    }

    #[test]
    fn a_beep_starts_on_every_second() {
        let beeps = Beeps::every_second(48_000);
        for second in 0..5 {
            assert!(beeps.beeping_at(second * 48_000), "second {second} should beep");
        }
    }

    #[test]
    fn bursts_fade_in_rather_than_clicking() {
        let beeps = Beeps::every_second(48_000);
        // The very first sample sits at the bottom of the ramp.
        assert_eq!(beeps.envelope(0), 0.0);
        // By the end of the ramp it is at full level.
        assert_eq!(beeps.envelope(beeps.ramp), 1.0);
    }

    #[test]
    fn every_fifth_beep_is_the_accent() {
        let beeps = Beeps::every_second(48_000);
        let quarter = beeps.burst / 2;

        let accent = beeps.sample_at(quarter);
        let normal = beeps.sample_at(48_000 + quarter);
        assert_ne!(
            accent, normal,
            "the accented beep must differ from the ordinary one"
        );
    }

    #[test]
    fn nothing_exceeds_full_scale() {
        let beeps = Beeps::every_second(48_000);
        for index in 0..48_000 {
            assert!(beeps.sample_at(index).abs() <= AMPLITUDE as f32 + f32::EPSILON);
        }
    }
}
