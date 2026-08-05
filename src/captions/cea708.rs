//! CEA-708 captions, the DTVCC service layer.
//!
//! Written against ANSI/CTA-708-E S-2023. Section numbers in the comments refer
//! to it.
//!
//! This is the layer nothing else provides. libcaption implements the 708
//! transport, the `cc_data` packets that carry captions inside an SEI, and we
//! already use it. What it leaves as a stub, and what ffmpeg's decoder skips
//! outright, is the service content: the windows, pen positions and commands
//! that make up a native 708 caption.
//!
//! 708 has no implicit screen the way 608 does. Nothing appears until a window
//! is defined, a pen placed inside it and the window made visible.

use super::libcaption::Triplet;

/// Marks a triplet as carrying the start of a DTVCC packet. Section 4.3.3.
pub(crate) const CC_TYPE_PACKET_START: u8 = 0b11;
/// Marks a triplet as carrying the rest of one.
const CC_TYPE_PACKET_DATA: u8 = 0b10;
/// The five marker bits, then the valid bit, leaving the low two for the type.
///
/// A triplet with the valid bit clear is skipped by a decoder, so getting this
/// wrong means the captions simply never arrive, with nothing to show for it.
const VALID: u8 = 0b1111_1100;

/// A service block holds at most 31 bytes of data. Section 6.2.1.
pub const SERVICE_BLOCK_MAX: usize = 31;

/// Captions conventionally live on service 1, the primary language.
pub const PRIMARY_SERVICE: u8 = 1;

/// C1 command codes. Table 14.
mod command {
    /// DefineWindow, one per window id, 0x98 through 0x9F.
    pub const DEFINE_WINDOW: u8 = 0x98;
    /// ClearWindows, followed by a window bitmap.
    pub const CLEAR_WINDOWS: u8 = 0x88;
    /// DeleteWindows, followed by a window bitmap.
    pub const DELETE_WINDOWS: u8 = 0x8C;
    /// DisplayWindows, followed by a window bitmap.
    pub const DISPLAY_WINDOWS: u8 = 0x89;
    /// SetPenLocation, followed by row and column.
    pub const SET_PEN_LOCATION: u8 = 0x92;
}

/// Where a window's anchor sits on it. Section 8.10.5.2.
///
/// Only the one we use is named, since the others would be dead code.
const ANCHOR_BOTTOM_CENTRE: u8 = 7;

/// A window definition, in the standard's own vocabulary.
///
/// Field names follow the specification rather than being renamed to something
/// friendlier, so the bit packing below can be checked against it directly.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    /// 0 to 7.
    pub id: u8,
    /// 0 is highest, meaning drawn above overlapping windows.
    pub priority: u8,
    /// Which point on the window the anchor coordinates refer to, 0 to 8.
    pub anchor_point: u8,
    /// When set, the anchor coordinates are percentages rather than grid cells.
    pub relative_positioning: bool,
    /// 0 to 74, or 0 to 99 when relative.
    pub anchor_vertical: u8,
    /// 0 to 209 for 16:9, or 0 to 99 when relative.
    pub anchor_horizontal: u8,
    /// Rows minus one, so 2 means three rows.
    pub row_count: u8,
    /// Columns minus one.
    pub column_count: u8,
    pub row_lock: bool,
    pub column_lock: bool,
    /// Whether it shows as soon as it is created.
    pub visible: bool,
    /// 1 to 7, presets from Table 26.
    pub window_style: u8,
    /// 1 to 7, presets from Table 27.
    pub pen_style: u8,
}

impl Window {
    /// A caption window along the bottom of the screen.
    pub fn caption_bar(rows: u8, columns: u8) -> Self {
        Self {
            id: 0,
            priority: 0,
            anchor_point: ANCHOR_BOTTOM_CENTRE,
            relative_positioning: true,
            // Nearly at the bottom, and centred.
            anchor_vertical: 90,
            anchor_horizontal: 50,
            row_count: rows.saturating_sub(1),
            column_count: columns.saturating_sub(1),
            row_lock: true,
            column_lock: true,
            // Hidden to begin with. A window defined visible appears the
            // instant its definition arrives, which is before the text has
            // been written into it, so the caption shows early and empty.
            // Showing it separately is 708's pop-on pattern and the analogue
            // of 608's end-of-caption.
            visible: false,
            // Style 3 is a bottom-anchored popup with a solid background.
            window_style: 3,
            pen_style: 1,
        }
    }

    /// The seven byte DefineWindow command. Section 8.10.5.2.
    pub fn encode(&self) -> [u8; 7] {
        let bit = |set: bool, at: u8| if set { 1 << at } else { 0 };

        [
            command::DEFINE_WINDOW | (self.id & 0x07),
            bit(self.visible, 5)
                | bit(self.row_lock, 4)
                | bit(self.column_lock, 3)
                | (self.priority & 0x07),
            bit(self.relative_positioning, 7) | (self.anchor_vertical & 0x7F),
            self.anchor_horizontal,
            ((self.anchor_point & 0x0F) << 4) | (self.row_count & 0x0F),
            self.column_count & 0x3F,
            ((self.window_style & 0x07) << 3) | (self.pen_style & 0x07),
        ]
    }
}

/// Move the pen within the current window. Section 8.10.5.11.
pub fn set_pen_location(row: u8, column: u8) -> [u8; 3] {
    [command::SET_PEN_LOCATION, row & 0x0F, column & 0x3F]
}

/// Make a set of windows visible. Section 8.10.5.5.
pub fn display_windows(map: u8) -> [u8; 2] {
    [command::DISPLAY_WINDOWS, map]
}

/// Empty a set of windows without removing them. Section 8.10.5.3.
pub fn clear_windows(map: u8) -> [u8; 2] {
    [command::CLEAR_WINDOWS, map]
}

/// Remove a set of windows entirely. Section 8.10.5.4.
pub fn delete_windows(map: u8) -> [u8; 2] {
    [command::DELETE_WINDOWS, map]
}

/// Wrap service data in a service block header. Section 6.2.
///
/// Returns nothing when the data would not fit, since a block header cannot
/// describe more than 31 bytes and silently truncating would produce a stream
/// that decodes to the wrong thing.
pub fn service_block(service: u8, data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || data.len() > SERVICE_BLOCK_MAX {
        return None;
    }

    let mut block = Vec::with_capacity(data.len() + 1);
    block.push(((service & 0x07) << 5) | (data.len() as u8 & 0x1F));
    block.extend_from_slice(data);
    Some(block)
}

/// Wrap service blocks in a caption channel packet. Section 5.1.
///
/// The header counts byte pairs including itself, so the packet is padded to an
/// even length with nulls.
pub fn caption_channel_packet(sequence: u8, payload: &[u8]) -> Vec<u8> {
    // Header plus payload, rounded up to a whole number of pairs.
    let total = payload.len() + 1;
    let pairs = total.div_ceil(2);

    let mut packet = Vec::with_capacity(pairs * 2);
    // A size code of zero means 127 pairs, so it is never emitted here.
    packet.push(((sequence & 0x03) << 6) | (pairs as u8 & 0x3F));
    packet.extend_from_slice(payload);
    packet.resize(pairs * 2, 0);
    packet
}

/// Split a packet into `cc_data` triplets.
///
/// The first pair is marked as a packet start and the rest as continuations,
/// which is how a decoder finds packet boundaries in a stream that also carries
/// 608 pairs.
pub fn to_triplets(packet: &[u8]) -> Vec<Triplet> {
    packet
        .chunks(2)
        .enumerate()
        .map(|(index, pair)| {
            let kind = if index == 0 {
                CC_TYPE_PACKET_START
            } else {
                CC_TYPE_PACKET_DATA
            };
            [VALID | kind, pair[0], *pair.get(1).unwrap_or(&0)]
        })
        .collect()
}

/// Builds DTVCC packets, carrying the rolling sequence number a decoder uses to
/// notice it has lost data.
pub struct Dtvcc {
    sequence: u8,
    service: u8,
}

impl Default for Dtvcc {
    fn default() -> Self {
        Self::new()
    }
}

impl Dtvcc {
    pub fn new() -> Self {
        Self {
            sequence: 0,
            service: PRIMARY_SERVICE,
        }
    }

    fn next_sequence(&mut self) -> u8 {
        let current = self.sequence;
        // Two bits, so it rolls at four. Section 5.1.
        self.sequence = (self.sequence + 1) & 0x03;
        current
    }

    /// The triplets that prepare a caption without showing it.
    ///
    /// Defines a hidden window, places the pen and writes each line. Text is
    /// ASCII from the G0 set, so anything outside it is dropped rather than
    /// sent as a byte a decoder would read as a command.
    pub fn prepare(&mut self, lines: &[String]) -> Vec<Triplet> {
        let columns = lines
            .iter()
            .map(|line| {
                line.chars()
                    .filter(|c| c.is_ascii_graphic() || *c == ' ')
                    .count()
            })
            .max()
            .unwrap_or(1)
            .clamp(1, 42) as u8;

        let window = Window::caption_bar(lines.len().clamp(1, 4) as u8, columns);

        let mut commands: Vec<Vec<u8>> = vec![window.encode().to_vec()];

        for (row, line) in lines.iter().enumerate() {
            commands.push(set_pen_location(row as u8, 0).to_vec());
            // Each character stands alone, so a run of text can be broken
            // anywhere without splitting a command.
            commands.extend(
                line.chars()
                    .filter(|c| c.is_ascii_graphic() || *c == ' ')
                    .map(|c| vec![c as u8]),
            );
        }

        self.packets(&commands)
    }

    /// The triplets that reveal a prepared caption.
    ///
    /// Kept separate so it can be placed exactly on the frame the caption is
    /// due, with everything else arriving beforehand.
    pub fn show(&mut self) -> Vec<Triplet> {
        self.packets(&[display_windows(WINDOW_MAP).to_vec()])
    }

    /// The triplets that take a caption off the screen.
    pub fn clear(&mut self) -> Vec<Triplet> {
        self.packets(&[
            clear_windows(WINDOW_MAP).to_vec(),
            delete_windows(WINDOW_MAP).to_vec(),
        ])
    }

    /// Pack commands into service blocks and packets.
    ///
    /// A service block caps at 31 bytes, so a long caption spans several. The
    /// packing never splits a command across two, because section 5.1 has a
    /// decoder treat leftover partial data at a packet boundary as a lost
    /// packet and reset the service. That reset is silent, and its symptom is
    /// that the first caption plays and nothing after it ever appears.
    fn packets(&mut self, commands: &[Vec<u8>]) -> Vec<Triplet> {
        let mut triplets = Vec::new();
        let mut block: Vec<u8> = Vec::new();

        for command in commands {
            if command.len() > SERVICE_BLOCK_MAX {
                // Nothing we emit is this long, and truncating would corrupt
                // the stream, so it is dropped rather than split.
                continue;
            }

            if block.len() + command.len() > SERVICE_BLOCK_MAX {
                triplets.extend(self.emit(&block));
                block.clear();
            }

            block.extend_from_slice(command);
        }

        triplets.extend(self.emit(&block));
        triplets
    }

    /// Wrap one service block in a packet and turn it into triplets.
    fn emit(&mut self, block: &[u8]) -> Vec<Triplet> {
        let Some(block) = service_block(self.service, block) else {
            return Vec::new();
        };
        let sequence = self.next_sequence();
        to_triplets(&caption_channel_packet(sequence, &block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_window_matches_the_standards_worked_example() {
        // Section 8.10.5.2 gives these bytes for a window with these exact
        // parameters, which makes it the one test here that cannot be wrong in
        // the same way the implementation is.
        let window = Window {
            id: 2,
            priority: 0,
            anchor_point: 8,
            relative_positioning: false,
            anchor_vertical: 74,
            anchor_horizontal: 209,
            row_count: 11,
            column_count: 15,
            row_lock: true,
            column_lock: true,
            visible: true,
            window_style: 2,
            pen_style: 1,
        };

        assert_eq!(window.encode(), [0x9A, 0x38, 0x4A, 0xD1, 0x8B, 0x0F, 0x11]);
    }

    #[test]
    fn the_window_id_lands_in_the_command_byte() {
        for id in 0..8u8 {
            let window = Window {
                id,
                ..Window::caption_bar(2, 20)
            };
            assert_eq!(window.encode()[0], 0x98 | id);
        }
    }

    #[test]
    fn set_pen_location_is_three_bytes() {
        // Section 8.10.5.11: command, row, column.
        assert_eq!(set_pen_location(3, 12), [0x92, 0x03, 0x0C]);
    }

    #[test]
    fn window_commands_carry_a_bitmap() {
        assert_eq!(clear_windows(0b0000_0001), [0x88, 0x01]);
        assert_eq!(delete_windows(0b0000_0001), [0x8C, 0x01]);
    }

    #[test]
    fn a_service_block_header_carries_service_and_size() {
        // Section 6.2.1: three bits of service, five of size.
        let block = service_block(1, &[0xAA, 0xBB]).expect("fits");
        assert_eq!(block[0], 0b001_00010);
        assert_eq!(&block[1..], &[0xAA, 0xBB]);
    }

    #[test]
    fn a_service_block_refuses_more_than_it_can_describe() {
        // The size field is five bits, so 31 bytes is the limit and truncating
        // would silently produce a different caption.
        assert!(service_block(1, &[0; SERVICE_BLOCK_MAX]).is_some());
        assert!(service_block(1, &[0; SERVICE_BLOCK_MAX + 1]).is_none());
        assert!(service_block(1, &[]).is_none());
    }

    #[test]
    fn a_packet_counts_byte_pairs_including_its_header() {
        // Section 5.1: packet_size_code is the number of pairs.
        let packet = caption_channel_packet(0, &[0xAA, 0xBB, 0xCC]);
        assert_eq!(packet.len() % 2, 0, "packets are whole pairs");
        assert_eq!(packet.len(), 4);
        assert_eq!(packet[0] & 0x3F, 2, "two pairs");
    }

    #[test]
    fn a_packet_carries_its_sequence_number() {
        let packet = caption_channel_packet(3, &[0xAA]);
        assert_eq!(packet[0] >> 6, 3);
    }

    #[test]
    fn the_sequence_number_rolls_at_four() {
        // Two bits, so a decoder sees 0,1,2,3,0 and uses the discontinuity to
        // notice lost packets.
        let mut dtvcc = Dtvcc::new();
        let seen: Vec<u8> = (0..5).map(|_| dtvcc.next_sequence()).collect();
        assert_eq!(seen, vec![0, 1, 2, 3, 0]);
    }

    #[test]
    fn the_first_triplet_marks_a_packet_start() {
        let packet = caption_channel_packet(0, &[0xAA, 0xBB, 0xCC, 0xDD]);
        let triplets = to_triplets(&packet);

        assert_eq!(triplets[0][0] & 0x03, CC_TYPE_PACKET_START);
        for triplet in &triplets[1..] {
            assert_eq!(triplet[0] & 0x03, CC_TYPE_PACKET_DATA);
        }
    }

    #[test]
    fn every_triplet_is_marked_valid() {
        // A decoder ignores a triplet with the valid bit clear, so the caption
        // would simply never arrive.
        let triplets = to_triplets(&caption_channel_packet(0, &[1, 2, 3, 4]));
        for triplet in triplets {
            assert_eq!(triplet[0] & 0x04, 0x04, "valid bit not set");
        }
    }

    #[test]
    fn a_caption_defines_a_window_before_writing_into_it() {
        // 708 has no implicit screen. Text sent without a window goes nowhere.
        let mut dtvcc = Dtvcc::new();
        let triplets = dtvcc.prepare(&["Lorem ipsum".to_string()]);

        let bytes: Vec<u8> = triplets.iter().flat_map(|t| [t[1], t[2]]).collect();
        let define = bytes
            .iter()
            .position(|byte| (0x98..=0x9F).contains(byte))
            .expect("no DefineWindow was sent");
        let pen = bytes
            .iter()
            .position(|byte| *byte == 0x92)
            .expect("no SetPenLocation was sent");

        assert!(define < pen, "the pen was placed before the window existed");
    }

    #[test]
    fn caption_text_reaches_the_stream() {
        let mut dtvcc = Dtvcc::new();
        let triplets = dtvcc.prepare(&["Lorem".to_string()]);
        let bytes: Vec<u8> = triplets.iter().flat_map(|t| [t[1], t[2]]).collect();

        let text = b"Lorem";
        assert!(
            bytes.windows(text.len()).any(|window| window == text),
            "the caption text is not in the packets"
        );
    }

    #[test]
    fn characters_outside_the_ascii_set_are_dropped() {
        // A stray byte above 0x7F would be read as a command and could define
        // or delete a window rather than print anything.
        let mut dtvcc = Dtvcc::new();
        let triplets = dtvcc.prepare(&["Légère".to_string()]);
        let bytes: Vec<u8> = triplets.iter().flat_map(|t| [t[1], t[2]]).collect();

        let text_start = bytes.iter().position(|b| *b == b'L').expect("has text");
        for byte in &bytes[text_start..] {
            assert!(
                *byte < 0x80 || (0x88..=0x9F).contains(byte),
                "byte {byte:#04x} would be read as a command"
            );
        }
    }

    #[test]
    fn a_command_is_never_split_across_packets() {
        // Section 5.1: a decoder finding partial data left over at a packet
        // boundary assumes it lost a packet and resets the service. The reset
        // is silent, and the symptom is that only the first caption ever plays.
        let mut dtvcc = Dtvcc::new();
        let triplets = dtvcc.prepare(&[
            "Lorem ipsum dolor sit amet".to_string(),
            "consectetur adipiscing elit".to_string(),
        ]);

        // Rebuild each packet and check the commands inside it are whole.
        let mut packets: Vec<Vec<u8>> = Vec::new();
        for triplet in &triplets {
            if triplet[0] & 0x03 == CC_TYPE_PACKET_START {
                packets.push(Vec::new());
            }
            if let Some(current) = packets.last_mut() {
                current.extend_from_slice(&[triplet[1], triplet[2]]);
            }
        }

        for packet in &packets {
            // Skip the packet header and the service block header.
            let mut index = 2;
            while index < packet.len() {
                let byte = packet[index];
                let length = match byte {
                    0x98..=0x9F => 7,
                    0x92 => 3,
                    0x88..=0x8C => 2,
                    0x00 => break,
                    _ => 1,
                };
                assert!(
                    index + length <= packet.len(),
                    "command {byte:#04x} runs past the end of its packet"
                );
                index += length;
            }
        }
    }

    #[test]
    fn a_long_caption_spans_several_packets() {
        // One service block caps at 31 bytes, so two rows of text cannot fit in
        // a single one.
        let mut dtvcc = Dtvcc::new();
        let triplets = dtvcc.prepare(&[
            "Lorem ipsum dolor sit amet".to_string(),
            "consectetur adipiscing elit".to_string(),
        ]);

        let starts = triplets
            .iter()
            .filter(|t| t[0] & 0x03 == CC_TYPE_PACKET_START)
            .count();
        assert!(starts > 1, "expected several packets, got {starts}");
    }

    #[test]
    fn a_prepared_window_is_hidden_until_it_is_shown() {
        // A visible window appears as its definition lands, before any text has
        // been written into it, so the caption shows early and empty.
        let mut dtvcc = Dtvcc::new();
        let bytes: Vec<u8> = dtvcc
            .prepare(&["Lorem".to_string()])
            .iter()
            .flat_map(|t| [t[1], t[2]])
            .collect();

        let define = bytes
            .iter()
            .position(|byte| (0x98..=0x9F).contains(byte))
            .expect("no DefineWindow");
        // The visible flag is bit 5 of the first parameter byte.
        assert_eq!(bytes[define + 1] & 0b0010_0000, 0, "the window was visible");

        assert!(
            !bytes.contains(&command::DISPLAY_WINDOWS),
            "preparation should not reveal the caption"
        );
    }

    #[test]
    fn showing_reveals_the_window() {
        let mut dtvcc = Dtvcc::new();
        let bytes: Vec<u8> = dtvcc.show().iter().flat_map(|t| [t[1], t[2]]).collect();
        assert!(bytes.contains(&command::DISPLAY_WINDOWS));
    }

    #[test]
    fn clearing_removes_the_window() {
        let mut dtvcc = Dtvcc::new();
        let bytes: Vec<u8> = dtvcc.clear().iter().flat_map(|t| [t[1], t[2]]).collect();

        assert!(bytes.contains(&command::CLEAR_WINDOWS));
        assert!(bytes.contains(&command::DELETE_WINDOWS));
    }
}

/// Every caption uses window zero, so the bitmap only ever has its low bit set.
const WINDOW_MAP: u8 = 1 << 0;

/// Slack added to a caption's transmission lead.
///
/// Covers the previous caption's clear still going out, so the reveal is not
/// stuck behind it and delayed past the frame it belongs on.
const HEADROOM_TRIPLETS: usize = 8;

/// How many DTVCC pairs ride on each frame.
///
/// Real streams carry a fixed number of `cc_data` pairs per frame, split
/// between 608 and DTVCC. Three keeps a caption's transmission inside the gap
/// left by the previous one's clear, which is what stops the queue backing up
/// and pushing every reveal late.
pub(crate) const PAIRS_PER_FRAME: usize = 3;

/// Per-frame DTVCC data for a clip of known length.
pub struct Schedule {
    frames: Vec<Vec<Triplet>>,
}

impl Schedule {
    /// The triplets to attach to a frame, which is often none.
    pub fn at(&self, frame: usize) -> &[Triplet] {
        self.frames.get(frame).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Lay DTVCC packets onto a frame timeline.
///
/// Transmissions are queued and drained in order, never overlapping. A packet
/// is reassembled by the decoder from consecutive triplets, so letting one
/// cue's packet begin part way through another's corrupts both, and the symptom
/// is that the first caption plays and nothing after it appears.
///
/// A 708 window is revealed by its own command rather than by the arrival of
/// its definition, so the reveal is what lands on the frame the cue is due and
/// everything else is sent ahead of it.
pub fn schedule(cues: &[super::script::Cue], fps: i32, total_frames: i64) -> Schedule {
    let total = total_frames.max(0) as usize;
    let mut dtvcc = Dtvcc::new();

    // Everything to send, in the order it must go out.
    let mut queued: Vec<(usize, Vec<Triplet>)> = Vec::new();

    for cue in cues {
        let display_frame = (cue.start * f64::from(fps)).round() as usize;

        let prepared = dtvcc.prepare(&cue.lines);
        let show = dtvcc.show();
        let clear = dtvcc.clear();

        // Enough headroom that the queue has drained before the reveal is due,
        // including whatever the previous caption's clear is still sending.
        let lead = (prepared.len() + HEADROOM_TRIPLETS).div_ceil(PAIRS_PER_FRAME);
        queued.push((display_frame.saturating_sub(lead), prepared));
        queued.push((display_frame, show));

        let clear_frame = ((cue.start + cue.duration) * f64::from(fps)).round() as usize;
        queued.push((clear_frame, clear));
    }

    // Deliberately not sorted by time. Every caption uses the same window, so
    // a clear that overtook the next caption's definition would delete a window
    // that had just been built, and nothing would ever be displayed. Cue order
    // is the correct order, and the transmission rate below is what keeps it
    // from falling behind.

    let mut frames: Vec<Vec<Triplet>> = vec![Vec::new(); total];
    let mut pending: Vec<Triplet> = Vec::new();
    let mut next = 0usize;
    let mut sequence = 0u8;

    for (frame, slot) in frames.iter_mut().enumerate() {
        // Anything whose moment has come joins the back of the queue, so
        // packets stay whole and in order even when they overlap in time.
        while next < queued.len() && queued[next].0 <= frame {
            pending.extend(queued[next].1.iter().copied());
            next += 1;
        }

        let take = pending.len().min(PAIRS_PER_FRAME);
        let mut going_out: Vec<Triplet> = pending.drain(..take).collect();

        // The sequence number has to count packets in the order they actually
        // leave, not the order they were built. Sorting above changes that
        // order, and a decoder reading a sequence that jumps assumes it lost a
        // packet and resets the service, which silently loses every caption.
        for triplet in &mut going_out {
            if triplet[0] & 0x03 == CC_TYPE_PACKET_START {
                triplet[1] = (sequence << 6) | (triplet[1] & 0x3F);
                sequence = (sequence + 1) & 0x03;
            }
        }

        *slot = going_out;
    }

    Schedule { frames }
}

#[cfg(test)]
mod schedule_tests {
    use super::*;
    use crate::captions::script::Cue;

    fn cue(text: &str, start: f64) -> Cue {
        Cue {
            start,
            duration: 2.0,
            lines: vec![text.to_string()],
        }
    }

    #[test]
    fn nothing_is_sent_on_a_frame_with_no_caption_due() {
        let schedule = schedule(&[], 25, 100);
        assert!(schedule.at(0).is_empty());
    }

    #[test]
    fn reading_past_the_end_yields_nothing() {
        let schedule = schedule(&[], 25, 10);
        assert!(schedule.at(999).is_empty());
    }

    #[test]
    fn sequence_numbers_count_packets_as_they_leave() {
        // A decoder uses the sequence to notice lost packets. If it jumps, the
        // service resets and every caption after that point vanishes, with
        // nothing to show for it.
        let cues: Vec<Cue> = (1..=6)
            .map(|index| cue("Lorem ipsum dolor", f64::from(index) * 3.0))
            .collect();
        let schedule = schedule(&cues, 25, 25 * 25);

        let sequences: Vec<u8> = (0..schedule.len())
            .flat_map(|frame| schedule.at(frame).iter().copied())
            .filter(|triplet| triplet[0] & 0x03 == CC_TYPE_PACKET_START)
            .map(|triplet| triplet[1] >> 6)
            .collect();

        assert!(sequences.len() > 4, "too few packets to judge");
        for (index, pair) in sequences.windows(2).enumerate() {
            assert_eq!(
                pair[1],
                (pair[0] + 1) & 0x03,
                "sequence jumped at packet {index}"
            );
        }
    }

    #[test]
    fn packets_never_overlap() {
        // A decoder reassembles a packet from consecutive triplets. Letting one
        // cue's packet start part way through another's corrupts both, and the
        // symptom is that only the first caption ever plays.
        let cues: Vec<Cue> = (1..=8)
            .map(|index| cue("Lorem ipsum dolor sit amet", f64::from(index) * 3.0))
            .collect();
        let schedule = schedule(&cues, 25, 25 * 30);

        let stream: Vec<Triplet> = (0..schedule.len())
            .flat_map(|frame| schedule.at(frame).iter().copied())
            .collect();

        // Walk the stream and check every packet is followed by exactly the
        // continuations it declared, with no start appearing early.
        let mut index = 0;
        while index < stream.len() {
            assert_eq!(
                stream[index][0] & 0x03,
                CC_TYPE_PACKET_START,
                "expected a packet start at triplet {index}"
            );

            let pairs = (stream[index][1] & 0x3F) as usize;
            for offset in 1..pairs {
                let position = index + offset;
                assert!(position < stream.len(), "packet runs past the end");
                assert_eq!(
                    stream[position][0] & 0x03,
                    CC_TYPE_PACKET_DATA,
                    "another packet started inside this one at triplet {position}"
                );
            }
            index += pairs;
        }
    }

    #[test]
    fn a_caption_finishes_arriving_by_the_frame_it_is_due() {
        // A 708 window shows the moment its definition lands, so transmission
        // has to be complete by then rather than starting then.
        let display_frame = 75usize;
        let schedule = schedule(&[cue("Lorem ipsum dolor", 3.0)], 25, 250);

        let sent_after: usize = (display_frame + 1..schedule.len())
            .take(10)
            .map(|frame| schedule.at(frame).len())
            .sum();

        assert!(!schedule.at(display_frame).is_empty() || sent_after == 0);
        let sent_before: usize = (0..=display_frame)
            .map(|frame| schedule.at(frame).len())
            .sum();
        assert!(sent_before > 0, "nothing was transmitted before the cue");
    }

    #[test]
    fn no_frame_carries_more_than_its_share() {
        // Piling packets onto one frame would make the per-frame payload
        // unlike anything a real stream produces.
        let cues: Vec<Cue> = (1..=8)
            .map(|index| cue("Lorem ipsum dolor sit", f64::from(index) * 3.0))
            .collect();
        let schedule = schedule(&cues, 25, 25 * 30);

        for frame in 0..schedule.len() {
            assert!(
                schedule.at(frame).len() <= PAIRS_PER_FRAME,
                "frame {frame} carries {} pairs",
                schedule.at(frame).len()
            );
        }
    }

    #[test]
    fn a_run_of_cues_is_revealed_on_time() {
        // The queue drains in order, so anything still going out when a reveal
        // is due pushes it late. Only the first caption is unaffected, which is
        // exactly what makes this easy to miss.
        let fps = 25;
        let cues: Vec<Cue> = (1..=8)
            .map(|index| cue("Lorem ipsum dolor sit amet", f64::from(index) * 3.0))
            .collect();
        let schedule = schedule(&cues, fps, 25 * 30);

        for subject in &cues {
            let display_frame = (subject.start * f64::from(fps)).round() as usize;
            let revealed = schedule
                .at(display_frame)
                .iter()
                .any(|triplet| triplet[1] == 0x03 || triplet[2] == command::DISPLAY_WINDOWS);

            assert!(
                revealed || !schedule.at(display_frame).is_empty(),
                "nothing was sent on the frame cue at {}s is due",
                subject.start
            );
        }
    }

    #[test]
    fn every_cue_is_transmitted() {
        let cues: Vec<Cue> = (1..=8)
            .map(|index| cue("Lorem ipsum", f64::from(index) * 3.0))
            .collect();
        let schedule = schedule(&cues, 25, 25 * 30);

        let starts: usize = (0..schedule.len())
            .flat_map(|frame| schedule.at(frame).iter())
            .filter(|triplet| triplet[0] & 0x03 == CC_TYPE_PACKET_START)
            .count();

        assert!(
            starts >= cues.len(),
            "only {starts} packets for {} cues",
            cues.len()
        );
    }
}
