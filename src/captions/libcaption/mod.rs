//! Safe access to libcaption.
//!
//! libcaption owns the hard parts of CEA-608, the character map including
//! accented letters, screen layout, and the control code sequences. This module
//! turns caption text into `cc_data` triplets and hands scheduling back to the
//! caller, because placing those triplets on a frame timeline is our problem,
//! not the library's.

mod sys;

use std::ffi::CString;
use std::os::raw::c_int;

/// The ATSC header libcaption writes before the caption bytes, and which
/// ffmpeg writes again from its own side data. Country code 181, provider
/// 0x0031, the `GA94` identifier, then user data type code 3.
const ATSC_HEADER: [u8; 8] = [181, 0, 49, b'G', b'A', b'9', b'4', 3];

/// Bytes before the triplets: the header above, then the flags and count byte,
/// then em_data.
const PAYLOAD_PREFIX: usize = 10;

/// A single trailing marker byte follows the triplets.
const PAYLOAD_SUFFIX: usize = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum CaptionError {
    /// The text held a null byte and cannot cross into C.
    UnusableText,
    /// libcaption could not allocate.
    OutOfMemory,
    /// libcaption rejected the text.
    Rejected(i32),
    /// A payload did not carry the ATSC header we expect, which would mean
    /// libcaption changed its output format underneath us.
    UnexpectedPayload,
}

impl std::fmt::Display for CaptionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnusableText => write!(formatter, "caption text contains a null byte"),
            Self::OutOfMemory => write!(formatter, "libcaption allocation failed"),
            Self::Rejected(code) => write!(formatter, "libcaption rejected the text, code {code}"),
            Self::UnexpectedPayload => {
                write!(
                    formatter,
                    "libcaption produced a payload without the expected ATSC header"
                )
            }
        }
    }
}

impl std::error::Error for CaptionError {}

/// Owns a libcaption frame so it is always freed, including on an early return.
struct Frame(*mut sys::CaptionFrame);

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: the pointer came from the matching allocator and is freed once.
        unsafe { sys::fakestream_caption_frame_free(self.0) }
    }
}

/// Owns an SEI list, including the messages libcaption hung off it.
struct SeiList(*mut sys::Sei);

impl Drop for SeiList {
    fn drop(&mut self) {
        // SAFETY: frees the message list and the struct together, once.
        unsafe { sys::fakestream_sei_free(self.0) }
    }
}

/// Marks a triplet as carrying valid CEA-608 field 1 data.
const FIELD_1_VALID: u8 = 0xFC;

/// Which caption channel a cue is transmitted on.
///
/// CEA-608 carries four channels. One and two share field 1, three and four
/// share field 2. Broadcasters traditionally put the primary language on one
/// and a secondary language on two, and a player is expected to let a viewer
/// choose between them.
///
/// Only field 1 is implemented, since that is what the A53 carriage in use
/// here transmits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    One,
    Two,
}

impl Channel {
    /// The bit libcaption sets in a control word's high byte to select this
    /// channel, matching `eia608_control_command`.
    fn selector(self) -> u8 {
        match self {
            Self::One => 0x00,
            Self::Two => 0x08,
        }
    }
}

/// Control and preamble words occupy this range once parity is stripped. Text
/// bytes sit at 0x20 and above and carry no channel of their own, going to
/// whichever channel the last control word selected.
const CONTROL_RANGE: std::ops::RangeInclusive<u8> = 0x10..=0x17;

/// CEA-608 bytes carry odd parity in the top bit.
fn with_parity(byte: u8) -> u8 {
    let value = byte & 0x7F;
    if value.count_ones().is_multiple_of(2) {
        value | 0x80
    } else {
        value
    }
}

/// Move a run of triplets onto another caption channel.
///
/// Only control and preamble words are tagged with a channel, so text passes
/// through untouched and follows whichever channel was last selected.
fn retag(triplets: &mut [Triplet], channel: Channel) {
    for triplet in triplets {
        let stripped = triplet[1] & 0x7F;
        if CONTROL_RANGE.contains(&stripped) {
            triplet[1] = with_parity(stripped | channel.selector());
        }
    }
}

/// One `cc_data` triplet, which is what ffmpeg wants in
/// `AV_FRAME_DATA_A53_CC` since it adds the ATSC wrapper back itself.
pub type Triplet = [u8; 3];

/// Encode caption text into an ordered run of `cc_data` triplets.
///
/// libcaption packs an entire caption into a single frame's user data, up to
/// 31 triplets at once. Real 608 trickles roughly one pair per frame, and a
/// decoder meeting a whole caption in one frame is not what a player will see
/// in the wild, so the run is returned flat and the caller spreads it across
/// frames.
pub fn encode(text: &str, channel: Channel) -> Result<Vec<Triplet>, CaptionError> {
    let source = CString::new(text).map_err(|_| CaptionError::UnusableText)?;

    // SAFETY: both allocations are checked for null and wrapped in owners that
    // free them exactly once, including if an error returns early below.
    let frame = Frame(unsafe { sys::fakestream_caption_frame_new() });
    if frame.0.is_null() {
        return Err(CaptionError::OutOfMemory);
    }

    let sei = SeiList(unsafe { sys::fakestream_sei_new(0.0) });
    if sei.0.is_null() {
        return Err(CaptionError::OutOfMemory);
    }

    // SAFETY: both pointers are live and the text outlives the call.
    let status = unsafe { sys::caption_frame_from_text(frame.0, source.as_ptr()) };
    if status < 0 {
        return Err(CaptionError::Rejected(status));
    }

    // SAFETY: both pointers are live for the duration of the call.
    unsafe { sys::sei_from_caption_frame(sei.0, frame.0) };

    let mut triplets = Vec::new();
    // SAFETY: walking libcaption's own list, stopping at the null terminator it
    // uses, and reading each payload with the length it reports.
    unsafe {
        let mut message = sys::fakestream_sei_message_head(sei.0);
        while !message.is_null() {
            let size = sys::sei_message_size(message);
            let data = sys::sei_message_data(message);

            if !data.is_null() && size > PAYLOAD_PREFIX + PAYLOAD_SUFFIX {
                let payload = std::slice::from_raw_parts(data, size);
                triplets.extend(triplets_from(payload)?);
            }

            message = sys::sei_message_next(message);
        }
    }

    // libcaption always encodes on channel one, so anything else is retagged
    // afterwards rather than by reaching into its internals.
    retag(&mut triplets, channel);

    Ok(triplets)
}

/// Strip the ATSC wrapper, leaving the triplets ffmpeg expects.
fn triplets_from(payload: &[u8]) -> Result<Vec<Triplet>, CaptionError> {
    if payload.len() < PAYLOAD_PREFIX + PAYLOAD_SUFFIX || payload[..8] != ATSC_HEADER {
        return Err(CaptionError::UnexpectedPayload);
    }

    let body = &payload[PAYLOAD_PREFIX..payload.len() - PAYLOAD_SUFFIX];
    if !body.len().is_multiple_of(3) {
        return Err(CaptionError::UnexpectedPayload);
    }

    Ok(body
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect())
}

/// The triplets that erase what is on screen.
///
/// This is deliberately not `encode("")`. That produces a whole pop-on
/// sequence ending in end-of-caption, which swaps the loading buffer onto the
/// screen. If the next caption has already begun loading, that shows it early
/// and half finished. Erase-display-memory clears what is visible and leaves
/// the loading buffer alone, which is the behaviour a gap between captions
/// needs.
///
/// The word is doubled, as broadcast decoders expect for control codes.
pub fn clear(channel: Channel) -> Vec<Triplet> {
    // SAFETY: a pure encoding function over two integers, no pointers involved.
    let word = unsafe { sys::eia608_control_command(sys::ERASE_DISPLAY_MEMORY, channel as c_int) };
    let triplet = [FIELD_1_VALID, (word >> 8) as u8, (word & 0xFF) as u8];
    vec![triplet, triplet]
}

/// Where the end-of-caption code sits in the run, which is the moment the
/// caption becomes visible.
///
/// Found by looking for the code rather than assuming a position, so a change
/// in how libcaption orders its output cannot silently shift caption timing.
/// The second byte keeps its value under 608's odd parity, since 0x2F already
/// has an odd number of bits set.
pub fn display_offset(triplets: &[Triplet]) -> Option<usize> {
    /// End-of-caption's control byte on channel one. Channel two sets the
    /// selector bit, so the channel is masked out before comparing rather than
    /// matching one channel's encoding and silently ignoring the other.
    const EOC_CONTROL: u8 = 0x14;
    const EOC_COMMAND: u8 = 0x2F;

    triplets.iter().position(|triplet| {
        let control = triplet[1] & 0x7F & !Channel::Two.selector();
        let command = triplet[2] & 0x7F;
        control == EOC_CONTROL && command == EOC_COMMAND
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_produces_a_run_of_triplets() {
        let triplets =
            encode("Lorem ipsum dolor sit amet", Channel::One).expect("libcaption should encode");
        assert!(!triplets.is_empty(), "no caption data was produced");
    }

    #[test]
    fn the_caption_has_a_display_point() {
        let triplets =
            encode("Lorem ipsum dolor sit amet", Channel::One).expect("libcaption should encode");
        let offset = display_offset(&triplets).expect("end of caption should be present");
        assert!(offset > 0, "a caption cannot display before it is sent");
        assert!(
            offset < triplets.len(),
            "the display point must be within the run"
        );
    }

    #[test]
    fn clearing_does_not_swap_the_loading_buffer() {
        // End-of-caption is what swaps, and a clear must not contain it, or a
        // half transmitted caption appears early.
        let clear = clear(Channel::One);
        assert!(
            display_offset(&clear).is_none(),
            "a clear must not carry end-of-caption"
        );
    }

    #[test]
    fn clearing_is_a_doubled_control_code() {
        let clear = clear(Channel::One);
        assert_eq!(clear.len(), 2);
        assert_eq!(clear[0], clear[1]);
        assert_eq!(clear[0][0], FIELD_1_VALID);
    }

    #[test]
    fn every_triplet_is_valid_field_one_data() {
        // 0xFC marks a valid CEA-608 field 1 triplet, which is the carriage
        // ffmpeg expects inside A53 side data.
        for triplet in encode("Lorem ipsum", Channel::One).expect("encodes") {
            assert_eq!(triplet[0], 0xFC);
        }
    }

    #[test]
    fn longer_text_takes_longer_to_send() {
        let short = encode("Lorem", Channel::One).expect("encodes");
        let long = encode(
            "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor",
            Channel::One,
        )
        .expect("encodes");

        assert!(
            long.len() > short.len(),
            "more text should need more frames of transmission"
        );
    }

    #[test]
    fn accented_characters_survive() {
        // The hand-rolled encoder folded these to base letters. libcaption
        // carries them properly through the extended character set, which is
        // the main reason for taking the dependency.
        let plain = encode("Legere", Channel::One).expect("encodes");
        let accented = encode("Légère", Channel::One).expect("encodes");

        assert_ne!(
            plain, accented,
            "accented text should not encode identically to its unaccented form"
        );
    }

    #[test]
    fn text_with_a_null_is_refused() {
        assert_eq!(
            encode("bad\0text", Channel::One),
            Err(CaptionError::UnusableText)
        );
    }

    #[test]
    fn retagging_matches_what_libcaption_would_encode() {
        // The channel is one bit in a control word, but rather than trust that
        // reading of the spec, compare against libcaption encoding the same
        // command directly on channel two.
        // SAFETY: pure encoding over two integers.
        let native = unsafe {
            sys::eia608_control_command(sys::ERASE_DISPLAY_MEMORY, Channel::Two as c_int)
        };
        let expected = [FIELD_1_VALID, (native >> 8) as u8, (native & 0xFF) as u8];

        let mut ours = clear(Channel::One);
        retag(&mut ours, Channel::Two);

        assert_eq!(ours[0], expected);
    }

    #[test]
    fn clearing_targets_the_channel_it_was_asked_for() {
        assert_ne!(
            clear(Channel::One),
            clear(Channel::Two),
            "each channel must be cleared separately"
        );
    }

    #[test]
    fn the_display_point_is_found_on_either_channel() {
        // Matching only channel one's encoding here dropped every channel two
        // caption silently, since a caption with no display point is skipped.
        for channel in [Channel::One, Channel::Two] {
            let triplets = encode("Lorem ipsum", channel).expect("encodes");
            assert!(
                display_offset(&triplets).is_some(),
                "no display point found on {channel:?}"
            );
        }
    }

    #[test]
    fn the_two_channels_encode_differently() {
        let one = encode("Lorem ipsum", Channel::One).expect("encodes");
        let two = encode("Lorem ipsum", Channel::Two).expect("encodes");
        assert_ne!(one, two, "channel two must be tagged differently");
        assert_eq!(one.len(), two.len(), "retagging must not change the length");
    }

    #[test]
    fn text_bytes_are_left_alone_when_retagging() {
        // Only control words carry a channel. Touching text would corrupt the
        // caption, so anything at 0x20 or above must pass through unchanged.
        let one = encode("Lorem ipsum", Channel::One).expect("encodes");
        let two = encode("Lorem ipsum", Channel::Two).expect("encodes");

        for (first, second) in one.iter().zip(two.iter()) {
            if (first[1] & 0x7F) >= 0x20 {
                assert_eq!(first, second, "a text triplet was altered");
            }
        }
    }

    #[test]
    fn a_payload_without_the_atsc_header_is_refused() {
        let rubbish = vec![0u8; 20];
        assert_eq!(
            triplets_from(&rubbish),
            Err(CaptionError::UnexpectedPayload)
        );
    }
}
