//! HLS packaging.
//!
//! libavformat's HLS muxer does the segmenting and playlist writing, and every
//! knob it has is an option on the muxer rather than something the ffmpeg
//! command line adds on top. So this is configuration rather than
//! reimplementation.
//!
//! Subtitles in HLS are always separate WebVTT renditions, whichever segment
//! format the media uses. There is no such thing as a subtitle baked into an
//! fMP4 segment here.

use crate::media::subtitles::SubtitleTrack;
use std::path::Path;

/// What the media segments are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentType {
    /// MPEG-TS segments, which is what older services serve and what a player
    /// is most likely to have been tested against.
    MpegTs,
    /// Fragmented MP4, used by newer services and required for some codecs.
    /// Adds an init segment and lifts the playlist to version 7.
    FragmentedMp4,
}

impl SegmentType {
    /// The muxer's own numbering for `hls_segment_type`.
    fn option_value(self) -> &'static std::ffi::CStr {
        match self {
            Self::MpegTs => c"mpegts",
            Self::FragmentedMp4 => c"fmp4",
        }
    }

    /// What a segment file is called.
    pub fn extension(self) -> &'static str {
        match self {
            Self::MpegTs => "ts",
            Self::FragmentedMp4 => "m4s",
        }
    }
}

/// Whether the playlist is complete or still growing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistKind {
    /// A finished recording. The playlist carries every segment and is marked
    /// complete, so a player knows it can seek anywhere.
    Vod,
    /// A live window. Old segments fall off the front as new ones arrive.
    Live { window_segments: usize },
}

/// How to package a clip as HLS.
#[derive(Debug, Clone)]
pub struct HlsOptions {
    pub segment_type: SegmentType,
    pub segment_seconds: f64,
    pub kind: PlaylistKind,
    /// Filename of the master playlist, written beside the variant playlists.
    pub master_name: String,
}

impl Default for HlsOptions {
    fn default() -> Self {
        Self {
            segment_type: SegmentType::MpegTs,
            segment_seconds: 4.0,
            kind: PlaylistKind::Vod,
            master_name: "master.m3u8".to_string(),
        }
    }
}

impl HlsOptions {
    /// Describe which streams belong to which variant.
    ///
    /// Subtitles go into a named group so the master playlist advertises them
    /// as selectable renditions rather than burying them in the variant.
    pub fn variant_map(&self, subtitles: &[SubtitleTrack]) -> String {
        let mut parts = vec!["v:0".to_string(), "a:0".to_string()];

        for index in 0..subtitles.len() {
            parts.push(format!("s:{index}"));
        }

        if !subtitles.is_empty() {
            parts.push("sgroup:subs".to_string());
        }

        parts.join(",")
    }

    /// What the variant playlist file is called.
    ///
    /// The `%v` is ffmpeg's variant index. It has to be there whenever
    /// `var_stream_map` is set, which is why the name carries a number even
    /// with a single variant.
    pub const VARIANT_TEMPLATE: &'static str = "stream%v.m3u8";

    /// What media segments are called, given the directory they live in.
    ///
    /// ffmpeg writes segments relative to the working directory rather than to
    /// the playlist, so this has to be a full path.
    pub fn segment_template(&self, directory: &Path) -> String {
        directory
            .join(format!("segment%v-%05d.{}", self.segment_type.extension()))
            .to_string_lossy()
            .into_owned()
    }

    /// The options the muxer is opened with.
    ///
    /// Returned as pairs rather than a dictionary so this stays testable
    /// without touching ffmpeg.
    pub fn as_pairs(&self, subtitles: &[SubtitleTrack], directory: &Path) -> Vec<(String, String)> {
        let mut pairs = vec![
            (
                "hls_segment_type".to_string(),
                self.segment_type
                    .option_value()
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("hls_time".to_string(), format!("{}", self.segment_seconds)),
            ("master_pl_name".to_string(), self.master_name.clone()),
            ("var_stream_map".to_string(), self.variant_map(subtitles)),
            (
                "hls_segment_filename".to_string(),
                self.segment_template(directory),
            ),
        ];

        match self.kind {
            PlaylistKind::Vod => {
                // Every segment listed and the playlist marked complete, so a
                // player knows the whole thing is seekable.
                pairs.push(("hls_list_size".to_string(), "0".to_string()));
                pairs.push(("hls_playlist_type".to_string(), "vod".to_string()));
                pairs.push(("hls_flags".to_string(), "independent_segments".to_string()));
            }
            PlaylistKind::Live { window_segments } => {
                pairs.push(("hls_list_size".to_string(), window_segments.to_string()));

                // Three flags that matter for a live window.
                //
                // delete_segments, or a stream left running fills the disk.
                //
                // temp_file, so a segment is written under another name and
                // renamed when finished. Without it a player can fetch a
                // segment that is still being written and get a truncated one,
                // which shows as a jump rather than an error.
                //
                // program_date_time, so a player anchors to a real timeline
                // rather than inferring one from segment durations.
                pairs.push((
                    "hls_flags".to_string(),
                    "delete_segments+temp_file+program_date_time+independent_segments".to_string(),
                ));
            }
        }

        pairs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captions::script::lorem_cues;
    use crate::media::subtitles::SubtitleFormat;

    fn tracks(count: usize) -> Vec<SubtitleTrack> {
        (0..count)
            .map(|index| {
                SubtitleTrack::new(
                    SubtitleFormat::WebVtt,
                    "eng",
                    &format!("Track {index}"),
                    lorem_cues(12.0, 3.0, 2.0),
                )
            })
            .collect()
    }

    fn value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn a_clip_without_subtitles_declares_no_group() {
        let options = HlsOptions::default();
        assert_eq!(options.variant_map(&[]), "v:0,a:0");
    }

    #[test]
    fn subtitle_tracks_join_a_named_group() {
        let options = HlsOptions::default();
        assert_eq!(
            options.variant_map(&tracks(3)),
            "v:0,a:0,s:0,s:1,s:2,sgroup:subs"
        );
    }

    #[test]
    fn vod_lists_every_segment_and_marks_itself_complete() {
        let pairs = HlsOptions::default().as_pairs(&[], Path::new("x"));
        assert_eq!(value(&pairs, "hls_list_size"), Some("0"));
        assert_eq!(value(&pairs, "hls_playlist_type"), Some("vod"));
    }

    #[test]
    fn live_keeps_a_window_and_deletes_what_falls_out_of_it() {
        // Without the delete flag a stream left running fills the disk.
        let options = HlsOptions {
            kind: PlaylistKind::Live { window_segments: 6 },
            ..HlsOptions::default()
        };
        let pairs = options.as_pairs(&[], Path::new("x"));

        assert_eq!(value(&pairs, "hls_list_size"), Some("6"));
        let flags = value(&pairs, "hls_flags").expect("a live playlist needs flags");
        assert!(flags.contains("delete_segments"));
        assert_eq!(
            value(&pairs, "hls_playlist_type"),
            None,
            "a live playlist is not vod"
        );
    }

    #[test]
    fn a_live_segment_is_only_published_once_it_is_complete() {
        // Without temp_file a player can fetch a segment while it is still
        // being written and get a truncated one, which shows as a glitch in the
        // picture rather than an error anyone would notice.
        let options = HlsOptions {
            kind: PlaylistKind::Live { window_segments: 6 },
            ..HlsOptions::default()
        };
        let flags = value(&options.as_pairs(&[], Path::new("x")), "hls_flags")
            .expect("a live playlist needs flags")
            .to_string();

        assert!(
            flags.contains("temp_file"),
            "segments would be published while still being written"
        );
    }

    #[test]
    fn a_live_playlist_carries_a_real_timeline() {
        // Otherwise a player infers the timeline from segment durations, which
        // leaves it guessing where the live edge is.
        let options = HlsOptions {
            kind: PlaylistKind::Live { window_segments: 6 },
            ..HlsOptions::default()
        };
        let flags = value(&options.as_pairs(&[], Path::new("x")), "hls_flags")
            .expect("a live playlist needs flags")
            .to_string();

        assert!(flags.contains("program_date_time"));
    }

    #[test]
    fn segments_are_declared_independently_decodable() {
        // True only because the keyframe interval matches the segment length.
        for kind in [PlaylistKind::Vod, PlaylistKind::Live { window_segments: 6 }] {
            let options = HlsOptions {
                kind,
                ..HlsOptions::default()
            };
            let flags = value(&options.as_pairs(&[], Path::new("x")), "hls_flags")
                .expect("flags")
                .to_string();
            assert!(
                flags.contains("independent_segments"),
                "{kind:?} did not declare it"
            );
        }
    }

    #[test]
    fn the_segment_type_reaches_the_muxer() {
        let ts = HlsOptions::default().as_pairs(&[], Path::new("x"));
        assert_eq!(value(&ts, "hls_segment_type"), Some("mpegts"));

        let fmp4 = HlsOptions {
            segment_type: SegmentType::FragmentedMp4,
            ..HlsOptions::default()
        }
        .as_pairs(&[], Path::new("x"));
        assert_eq!(value(&fmp4, "hls_segment_type"), Some("fmp4"));
    }

    #[test]
    fn segment_files_are_named_for_their_type() {
        assert_eq!(SegmentType::MpegTs.extension(), "ts");
        assert_eq!(SegmentType::FragmentedMp4.extension(), "m4s");
    }

    #[test]
    fn segments_are_written_beside_their_playlist() {
        // ffmpeg writes segments relative to the working directory, not to the
        // playlist, so a bare name would scatter them wherever the tool was
        // started from.
        let directory = Path::new("fixtures").join("live").join("hls");
        let template = HlsOptions::default().segment_template(&directory);

        // Compared as a path rather than a string, since the separator differs
        // by platform and the assertion is about the directory, not the style.
        assert_eq!(Path::new(&template).parent(), Some(directory.as_path()));
        assert!(template.ends_with(".ts"));
    }

    #[test]
    fn fragmented_segments_are_named_as_such() {
        let options = HlsOptions {
            segment_type: SegmentType::FragmentedMp4,
            ..HlsOptions::default()
        };
        assert!(options.segment_template(Path::new("x")).ends_with(".m4s"));
    }
}
