//! What a subtitle track is, and how each format reaches the encoder.
//!
//! Two families sit behind one type. Bitmap formats are pictures we draw, so
//! the player only blits them. Text formats carry characters and the player
//! renders them, which is what makes them worth testing separately: font
//! coverage, line breaking and positioning all become the player's problem.

use crate::captions::script::Cue;

/// The subtitle formats fakestream can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    /// Bitmap subtitles on their own stream, common on European broadcast.
    Dvb,
    /// 3GPP timed text, the usual choice inside MP4.
    Tx3g,
    /// Used by DASH and CMAF, carried in MP4 as `stpp`.
    Ttml,
    SubRip,
    /// Carries styling, so it exercises more of a renderer than the others.
    Ass,
    WebVtt,
}

impl SubtitleFormat {
    /// The ffmpeg encoder behind this format.
    pub fn encoder_name(self) -> &'static std::ffi::CStr {
        match self {
            Self::Dvb => c"dvbsub",
            Self::Tx3g => c"mov_text",
            Self::Ttml => c"ttml",
            Self::SubRip => c"subrip",
            Self::Ass => c"ass",
            Self::WebVtt => c"webvtt",
        }
    }

    /// Bitmap formats are drawn by us, text formats are rendered by the player.
    pub fn is_bitmap(self) -> bool {
        matches!(self, Self::Dvb)
    }

    /// Text encoders parse an ASS header when opened and refuse without one.
    pub fn needs_ass_header(self) -> bool {
        !self.is_bitmap()
    }
}

/// One subtitle track in a clip.
#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    pub format: SubtitleFormat,
    /// ISO 639 language tag, written as stream metadata so a player can offer
    /// a choice and match a stored preference.
    pub language: String,
    /// Shown in a player's track list, where it has one.
    pub title: String,
    pub cues: Vec<Cue>,
}

impl SubtitleTrack {
    pub fn new(format: SubtitleFormat, language: &str, title: &str, cues: Vec<Cue>) -> Self {
        Self {
            format,
            language: language.to_string(),
            title: title.to_string(),
            cues,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dvb_is_a_bitmap_format() {
        assert!(SubtitleFormat::Dvb.is_bitmap());
        for format in [
            SubtitleFormat::Tx3g,
            SubtitleFormat::Ttml,
            SubtitleFormat::SubRip,
            SubtitleFormat::Ass,
            SubtitleFormat::WebVtt,
        ] {
            assert!(!format.is_bitmap(), "{format:?} should be a text format");
        }
    }

    #[test]
    fn every_text_format_needs_the_ass_header() {
        // Opening a text encoder without one fails, and the failure is obscure,
        // so this pairing is worth pinning.
        for format in [
            SubtitleFormat::Tx3g,
            SubtitleFormat::Ttml,
            SubtitleFormat::SubRip,
            SubtitleFormat::Ass,
            SubtitleFormat::WebVtt,
        ] {
            assert!(format.needs_ass_header(), "{format:?} needs a header");
        }
        assert!(!SubtitleFormat::Dvb.needs_ass_header());
    }

    #[test]
    fn every_format_names_an_encoder() {
        for format in [
            SubtitleFormat::Dvb,
            SubtitleFormat::Tx3g,
            SubtitleFormat::Ttml,
            SubtitleFormat::SubRip,
            SubtitleFormat::Ass,
            SubtitleFormat::WebVtt,
        ] {
            assert!(!format.encoder_name().to_bytes().is_empty());
        }
    }
}
