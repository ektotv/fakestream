//! Minimal CEA-608 byte pair generation, enough to prove the A53 side data
//! path. No unsafe here.
//!
//! Production will hand this job to libcaption, which covers roll-up, the full
//! character map and 708 wrapping. This exists so the spike proves the ffmpeg
//! plumbing on its own, without a C link in the way.
//!
//! The stream is a pop-on caption. Load into the hidden buffer, then swap it to
//! the display in one go.

/// One 608 field 1 pair carried on a frame, as an A53 `cc_data` triplet.
pub const CC_VALID_FIELD_1: u8 = 0xFC;

/// 608 bytes carry odd parity in the top bit.
fn with_parity(byte: u8) -> u8 {
    let value = byte & 0x7F;
    if value.count_ones() % 2 == 0 {
        value | 0x80
    } else {
        value
    }
}

/// Control codes, channel 1.
mod control {
    /// Resume caption loading, which selects pop-on mode.
    pub const RCL: (u8, u8) = (0x14, 0x20);
    /// Erase non-displayed memory.
    pub const ENM: (u8, u8) = (0x14, 0x2E);
    /// End of caption, swaps the hidden buffer to the display.
    pub const EOC: (u8, u8) = (0x14, 0x2F);
    /// Erase displayed memory.
    pub const EDM: (u8, u8) = (0x14, 0x2C);
    /// Preamble address code for row 15, white, no underline.
    pub const PAC_ROW_15: (u8, u8) = (0x14, 0x60);
}

/// Map a character to its 608 code. The basic set is ASCII with a few
/// substitutions, so anything outside it degrades to a space.
fn encode_char(character: char) -> u8 {
    match character {
        ' '..='~' => character as u8,
        // The accented set lives in extended codes that need two more pairs
        // each. Out of scope for the spike, so fold them to their base letter.
        'é' | 'è' | 'ê' => b'e',
        'ü' | 'û' => b'u',
        'ß' => b's',
        '—' | '–' => b'-',
        _ => b' ',
    }
}

/// Build the pair stream that displays `text`, then clears it.
///
/// Control codes are sent twice, which is what broadcast decoders expect and
/// what makes them robust against a dropped pair.
pub fn pop_on_caption(text: &str) -> Vec<(u8, u8)> {
    let mut pairs = Vec::new();

    for pair in [control::RCL, control::ENM, control::PAC_ROW_15] {
        pairs.push(pair);
        pairs.push(pair);
    }

    let encoded: Vec<u8> = text.chars().map(encode_char).collect();
    for chunk in encoded.chunks(2) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0x80);
        pairs.push((first, second));
    }

    pairs.push(control::EOC);
    pairs.push(control::EOC);
    pairs
}

/// The pair that clears the display.
pub fn clear_pairs() -> Vec<(u8, u8)> {
    vec![control::EDM, control::EDM]
}

/// Turn a pair into the three byte `cc_data` tuple ffmpeg expects as side data.
/// Null pairs keep the caption channel alive on frames with nothing to say.
pub fn triplet(pair: Option<(u8, u8)>) -> [u8; 3] {
    match pair {
        Some((first, second)) => [CC_VALID_FIELD_1, with_parity(first), with_parity(second)],
        None => [CC_VALID_FIELD_1, with_parity(0x00), with_parity(0x00)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parity_is_odd() {
        // 0x14 has two set bits, so it gains the parity bit.
        assert_eq!(with_parity(0x14), 0x94);
        // 0x20 has one set bit already.
        assert_eq!(with_parity(0x20), 0x20);
    }

    #[test]
    fn caption_starts_by_selecting_pop_on() {
        let pairs = pop_on_caption("HI");
        assert_eq!(pairs[0], control::RCL);
        assert_eq!(pairs[1], control::RCL);
    }
}
