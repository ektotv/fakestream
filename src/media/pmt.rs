//! Announcing CEA captions in the PMT.
//!
//! ffmpeg carries CEA-608 and 708 data in the video's SEI but never tells the
//! PMT they are there. Some players only look for captions a stream announces,
//! so they find nothing in an ffmpeg TS. This module writes the one thing that
//! is missing: an ATSC A/65 caption_service_descriptor (tag 0x86) in the video
//! stream's descriptor loop, added to an already muxed transport stream.
//!
//! The descriptor layout is ATSC A/65; CTA-708-E Annex E.3 is what says it
//! belongs in the PMT. The rewrite parses the PAT to find the PMT, splices the
//! descriptor into the video elementary stream's loop, then fixes the section
//! length and CRC. Every byte spliced in is taken back from the packet's
//! trailing stuffing, so packets stay exactly 188 bytes and the file length
//! never changes.

/// ATSC caption_service_descriptor tag.
pub const CAPTION_SERVICE_DESCRIPTOR_TAG: u8 = 0x86;

/// TS packet size, fixed by the standard.
const PACKET: usize = 188;

/// stream_type values that name a video elementary stream. The descriptor
/// belongs on the video stream, since that is what actually carries the SEI.
/// H.264, which is what these clips use, is 0x1B.
const VIDEO_STREAM_TYPES: &[u8] = &[0x01, 0x02, 0x10, 0x1B, 0x24, 0x42, 0xD1, 0xEA];

/// One announced caption service.
#[derive(Debug, Clone, Copy)]
pub struct CaptionService {
    /// ISO 639.2/B three letter language code, for example `*b"eng"`.
    pub language: [u8; 3],
    pub kind: ServiceKind,
    /// Captions written for beginning readers, slower and simpler. Off for a
    /// normal service.
    pub easy_reader: bool,
    /// Authored for 16:9 rather than 4:3.
    pub wide_aspect: bool,
}

/// Whether a service is line 21 (CEA-608) or digital (CEA-708).
#[derive(Debug, Clone, Copy)]
pub enum ServiceKind {
    /// CEA-608 carried as line 21 data. `field2` false is field 1 (CC1, CC2),
    /// true is field 2 (CC3, CC4).
    Line21 { field2: bool },
    /// CEA-708 DTVCC. `service_number` is 1 to 63; service 1 is the primary
    /// caption service.
    Digital { service_number: u8 },
}

/// Anything that stops the rewrite from happening safely. Every case is loud
/// rather than corrupting the stream silently.
#[derive(Debug, PartialEq, Eq)]
pub enum PmtError {
    /// The buffer is not a whole number of 188 byte packets.
    NotTransportStream,
    /// A PSI packet was structured in a way this simple parser does not handle.
    Malformed,
    /// No PAT was found, so the PMT PID is unknown.
    NoProgramTable,
    /// The PAT named no program, so there is no PMT to edit.
    NoProgram,
    /// No PMT packet was found on the PID the PAT pointed at.
    NoMapTable,
    /// The PMT names no elementary stream to attach the descriptor to.
    NoStreams,
    /// The PMT section is split across packets, which this rewriter does not
    /// follow. The small PMTs ffmpeg writes never do this.
    SpansPackets,
    /// The packet has too little stuffing to fit the descriptor.
    NoRoom,
}

/// Build a caption_service_descriptor announcing every given service.
///
/// Returns the raw descriptor bytes, tag and length included, ready to splice
/// into a PMT descriptor loop.
pub fn caption_service_descriptor(services: &[CaptionService]) -> Vec<u8> {
    let count = services.len();
    let mut out = Vec::with_capacity(3 + count * 6);
    out.push(CAPTION_SERVICE_DESCRIPTOR_TAG);
    // descriptor_length counts every byte after it: the count byte plus six per
    // service.
    out.push((1 + count * 6) as u8);
    // Three reserved ones, then number_of_services in the low five bits.
    out.push(0xE0 | (count as u8 & 0x1F));

    for service in services {
        out.extend_from_slice(&service.language);

        // digital_cc (top bit), a reserved one, then either the six bit service
        // number for 708, or five reserved ones and the line 21 field bit for
        // 608.
        let selector = match service.kind {
            ServiceKind::Digital { service_number } => 0xC0 | (service_number & 0x3F),
            ServiceKind::Line21 { field2 } => 0x7E | u8::from(field2),
        };
        out.push(selector);

        // easy_reader, wide_aspect_ratio, then reserved ones filling the rest.
        let mut flags = 0x3F;
        if service.easy_reader {
            flags |= 0x80;
        }
        if service.wide_aspect {
            flags |= 0x40;
        }
        out.push(flags);
        out.push(0xFF);
    }

    out
}

/// Add `descriptor` to the video stream's loop in every PMT of a muxed TS.
///
/// Edits `ts` in place and returns how many PMT copies were rewritten. ffmpeg
/// repeats the PMT through the stream, so a whole file has many copies and all
/// must agree.
pub fn announce_captions(ts: &mut [u8], descriptor: &[u8]) -> Result<usize, PmtError> {
    if ts.is_empty() || !ts.len().is_multiple_of(PACKET) {
        return Err(PmtError::NotTransportStream);
    }

    let pmt_pid = find_pmt_pid(ts)?;

    let mut rewritten = 0;
    for packet in ts.chunks_mut(PACKET) {
        if packet_pid(packet) == pmt_pid
            && payload_unit_start(packet)
            && rewrite_pmt(packet, descriptor)?
        {
            rewritten += 1;
        }
    }

    if rewritten == 0 {
        return Err(PmtError::NoMapTable);
    }
    Ok(rewritten)
}

/// Read back the caption_service_descriptor from the video stream of the first
/// PMT, tag and length included. None when no PMT carries one. Mirrors
/// [`announce_captions`], for inspection and tests.
pub fn video_caption_descriptor(ts: &[u8]) -> Option<Vec<u8>> {
    if ts.is_empty() || !ts.len().is_multiple_of(PACKET) {
        return None;
    }
    let pmt_pid = find_pmt_pid(ts).ok()?;
    ts.chunks(PACKET)
        .filter(|packet| packet_pid(packet) == pmt_pid && payload_unit_start(packet))
        .find_map(pmt_caption_descriptor)
}

/// The caption_service_descriptor in one PMT packet's video stream, if present.
fn pmt_caption_descriptor(packet: &[u8]) -> Option<Vec<u8>> {
    let section = section_start(packet)?;
    if *packet.get(section)? != 0x02 {
        return None;
    }
    let length = section_length(packet, section)?;
    let section_end = section + 3 + length;
    if section_end > packet.len() {
        return None;
    }
    let program_info_length =
        (((packet[section + 10] as usize) & 0x0F) << 8) | packet[section + 11] as usize;
    let streams_start = section + 12 + program_info_length;
    let streams_end = section_end - 4;

    let stream = find_video_stream(packet, streams_start, streams_end).ok()?;
    let es_info_length =
        (((packet[stream + 3] as usize) & 0x0F) << 8) | packet[stream + 4] as usize;
    let loop_start = stream + 5;
    let loop_end = loop_start + es_info_length;

    let mut at = loop_start;
    while at + 2 <= loop_end {
        let tag = packet[at];
        let length = packet[at + 1] as usize;
        if tag == CAPTION_SERVICE_DESCRIPTOR_TAG {
            return packet.get(at..at + 2 + length).map(<[u8]>::to_vec);
        }
        at += 2 + length;
    }
    None
}

/// The 13 bit PID a TS packet belongs to.
fn packet_pid(packet: &[u8]) -> u16 {
    (((packet[1] as u16) & 0x1F) << 8) | packet[2] as u16
}

/// Whether this packet begins a PSI section (the payload unit start indicator).
fn payload_unit_start(packet: &[u8]) -> bool {
    packet[1] & 0x40 != 0
}

/// Where the payload begins, stepping over an adaptation field if present.
/// None when the packet is not a TS packet or carries no payload.
fn payload_start(packet: &[u8]) -> Option<usize> {
    if packet[0] != 0x47 {
        return None;
    }
    let control = (packet[3] >> 4) & 0x3;
    let has_payload = control & 0x1 != 0;
    let has_adaptation = control & 0x2 != 0;
    if !has_payload {
        return None;
    }
    let start = if has_adaptation {
        5 + packet[4] as usize
    } else {
        4
    };
    (start < packet.len()).then_some(start)
}

/// Read a section's `section_length`, the 12 bit count of bytes that follow it.
fn section_length(packet: &[u8], section: usize) -> Option<usize> {
    let hi = *packet.get(section + 1)? as usize & 0x0F;
    let lo = *packet.get(section + 2)? as usize;
    Some((hi << 8) | lo)
}

/// Read the section start of a PSI packet, stepping over the pointer field.
fn section_start(packet: &[u8]) -> Option<usize> {
    let payload = payload_start(packet)?;
    let pointer = *packet.get(payload)? as usize;
    let section = payload + 1 + pointer;
    (section < packet.len()).then_some(section)
}

/// Find the PMT PID by reading the first program out of the PAT.
fn find_pmt_pid(ts: &[u8]) -> Result<u16, PmtError> {
    for packet in ts.chunks(PACKET) {
        if packet_pid(packet) != 0 || !payload_unit_start(packet) {
            continue;
        }
        let section = section_start(packet).ok_or(PmtError::Malformed)?;
        // table_id 0x00 is the PAT.
        if *packet.get(section).ok_or(PmtError::Malformed)? != 0x00 {
            continue;
        }
        let length = section_length(packet, section).ok_or(PmtError::Malformed)?;
        // Programs sit between the eight byte header and the four byte CRC.
        let programs_start = section + 8;
        let programs_end = section + 3 + length - 4;
        if programs_end > packet.len() || programs_end < programs_start {
            return Err(PmtError::Malformed);
        }

        let mut at = programs_start;
        while at + 4 <= programs_end {
            let program_number = ((packet[at] as u16) << 8) | packet[at + 1] as u16;
            let pid = (((packet[at + 2] as u16) & 0x1F) << 8) | packet[at + 3] as u16;
            // Program 0 is the network PID, not a program map.
            if program_number != 0 {
                return Ok(pid);
            }
            at += 4;
        }
        return Err(PmtError::NoProgram);
    }
    Err(PmtError::NoProgramTable)
}

/// Splice the descriptor into one PMT packet. Returns whether it changed
/// anything: a PMT that already carries the descriptor is left alone so a
/// second pass is a no-op.
fn rewrite_pmt(packet: &mut [u8], descriptor: &[u8]) -> Result<bool, PmtError> {
    let section = section_start(packet).ok_or(PmtError::Malformed)?;
    // table_id 0x02 is the PMT. Other tables can share this PID.
    if *packet.get(section).ok_or(PmtError::Malformed)? != 0x02 {
        return Ok(false);
    }
    let length = section_length(packet, section).ok_or(PmtError::Malformed)?;
    let section_end = section + 3 + length;
    if section_end > packet.len() {
        return Err(PmtError::SpansPackets);
    }

    let program_info_length =
        (((packet[section + 10] as usize) & 0x0F) << 8) | packet[section + 11] as usize;
    let streams_start = section + 12 + program_info_length;
    let streams_end = section_end - 4;

    let target = find_video_stream(packet, streams_start, streams_end)?;
    let es_info_length =
        (((packet[target + 3] as usize) & 0x0F) << 8) | packet[target + 4] as usize;
    let loop_start = target + 5;
    let loop_end = loop_start + es_info_length;

    if has_descriptor(packet, loop_start, loop_end, CAPTION_SERVICE_DESCRIPTOR_TAG) {
        return Ok(false);
    }

    splice(packet, loop_end, section, section_end, target, descriptor)?;
    Ok(true)
}

/// Pick the elementary stream the descriptor goes on: the first video stream,
/// or the first stream of any kind if none looks like video. Returns the offset
/// of that stream's five byte entry.
fn find_video_stream(packet: &[u8], start: usize, end: usize) -> Result<usize, PmtError> {
    let mut first = None;
    let mut video = None;
    let mut at = start;
    while at + 5 <= end {
        if first.is_none() {
            first = Some(at);
        }
        let stream_type = packet[at];
        if video.is_none() && VIDEO_STREAM_TYPES.contains(&stream_type) {
            video = Some(at);
        }
        let es_info_length = (((packet[at + 3] as usize) & 0x0F) << 8) | packet[at + 4] as usize;
        at += 5 + es_info_length;
    }
    video.or(first).ok_or(PmtError::NoStreams)
}

/// Whether a descriptor loop already contains a descriptor with this tag.
fn has_descriptor(packet: &[u8], start: usize, end: usize, tag: u8) -> bool {
    let mut at = start;
    while at + 2 <= end {
        if packet[at] == tag {
            return true;
        }
        at += 2 + packet[at + 1] as usize;
    }
    false
}

/// Insert `descriptor` at `insert_at`, taking the room back from the packet's
/// trailing stuffing, then fix the two lengths and the CRC.
fn splice(
    packet: &mut [u8],
    insert_at: usize,
    section: usize,
    section_end: usize,
    stream: usize,
    descriptor: &[u8],
) -> Result<(), PmtError> {
    let added = descriptor.len();
    // The bytes after the section are stuffing (0xFF). We keep the packet the
    // same size, so the last `added` of them are pushed off the end.
    if section_end + added > packet.len() {
        return Err(PmtError::NoRoom);
    }

    let tail = packet.len() - added;
    packet.copy_within(insert_at..tail, insert_at + added);
    packet[insert_at..insert_at + added].copy_from_slice(descriptor);

    let new_es_info =
        (((packet[stream + 3] as usize) & 0x0F) << 8 | packet[stream + 4] as usize) + added;
    packet[stream + 3] = (packet[stream + 3] & 0xF0) | ((new_es_info >> 8) & 0x0F) as u8;
    packet[stream + 4] = (new_es_info & 0xFF) as u8;

    let new_length = section_length(packet, section).ok_or(PmtError::Malformed)? + added;
    packet[section + 1] = (packet[section + 1] & 0xF0) | ((new_length >> 8) & 0x0F) as u8;
    packet[section + 2] = (new_length & 0xFF) as u8;

    let new_section_end = section + 3 + new_length;
    let crc = crc32_mpeg2(&packet[section..new_section_end - 4]);
    packet[new_section_end - 4..new_section_end].copy_from_slice(&crc.to_be_bytes());
    Ok(())
}

/// The CRC-32 MPEG-2 tables use, big endian and with no reflection or final
/// inversion. Running it across a section plus its own CRC yields zero, which
/// is how a decoder checks the table.
fn crc32_mpeg2(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append a correct CRC to a PSI section, the way a muxer would.
    fn with_crc(mut section: Vec<u8>) -> Vec<u8> {
        let crc = crc32_mpeg2(&section);
        section.extend_from_slice(&crc.to_be_bytes());
        section
    }

    /// Wrap a PSI section in a single 188 byte packet on `pid`, stuffed to size.
    fn psi_packet(pid: u16, section: &[u8]) -> Vec<u8> {
        let mut packet = vec![0xFFu8; PACKET];
        packet[0] = 0x47;
        // Payload unit start set, PID in the low bits of bytes 1 and 2.
        packet[1] = 0x40 | ((pid >> 8) as u8 & 0x1F);
        packet[2] = (pid & 0xFF) as u8;
        // Payload only, continuity counter zero.
        packet[3] = 0x10;
        // Pointer field, then the section.
        packet[4] = 0x00;
        packet[5..5 + section.len()].copy_from_slice(section);
        packet
    }

    fn pat_section(pmt_pid: u16) -> Vec<u8> {
        // table_id, syntax flags and length placeholder, then the fixed header.
        let mut body = vec![0x00, 0xB0, 0x00, 0x00, 0x01, 0xC1, 0x00, 0x00];
        // One program, number 1, pointing at the PMT PID.
        body.push(0x00);
        body.push(0x01);
        body.push(0xE0 | ((pmt_pid >> 8) as u8 & 0x1F));
        body.push((pmt_pid & 0xFF) as u8);
        set_section_length(&mut body);
        with_crc(body)
    }

    fn pmt_section(streams: &[(u8, u16)]) -> Vec<u8> {
        let mut body = vec![0x02, 0xB0, 0x00, 0x00, 0x01, 0xC1, 0x00, 0x00];
        // PCR PID and an empty program info loop.
        body.push(0xE1);
        body.push(0x00);
        body.push(0xF0);
        body.push(0x00);
        for &(stream_type, pid) in streams {
            body.push(stream_type);
            body.push(0xE0 | ((pid >> 8) as u8 & 0x1F));
            body.push((pid & 0xFF) as u8);
            // Empty ES info loop.
            body.push(0xF0);
            body.push(0x00);
        }
        set_section_length(&mut body);
        with_crc(body)
    }

    /// Fill in section_length: everything after the length field, CRC included.
    fn set_section_length(body: &mut [u8]) {
        let length = body.len() - 3 + 4;
        body[1] = (body[1] & 0xF0) | ((length >> 8) & 0x0F) as u8;
        body[2] = (length & 0xFF) as u8;
    }

    #[test]
    fn descriptor_matches_the_standard_layout() {
        // A 608 field 1 service and the primary 708 service, both English and
        // wide aspect. Bytes worked out by hand from ATSC A/65.
        let services = [
            CaptionService {
                language: *b"eng",
                kind: ServiceKind::Line21 { field2: false },
                easy_reader: false,
                wide_aspect: true,
            },
            CaptionService {
                language: *b"eng",
                kind: ServiceKind::Digital { service_number: 1 },
                easy_reader: false,
                wide_aspect: true,
            },
        ];
        let bytes = caption_service_descriptor(&services);
        assert_eq!(
            bytes,
            vec![
                0x86, 0x0D, 0xE2, // tag, length 13, reserved + two services
                b'e', b'n', b'g', 0x7E, 0x7F, 0xFF, // 608 field 1
                b'e', b'n', b'g', 0xC1, 0x7F, 0xFF, // 708 service 1
            ]
        );
    }

    #[test]
    fn a_section_and_its_crc_check_to_zero() {
        let section = with_crc(vec![
            0x02, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0xE1, 0x00, 0xF0, 0x00,
        ]);
        assert_eq!(crc32_mpeg2(&section), 0);
    }

    #[test]
    fn the_descriptor_lands_in_the_video_stream() {
        let pmt_pid = 0x1000;
        // Video (H.264) then audio (MPEG-1 audio).
        let mut ts = pat_packet_and_pmt(pmt_pid, &[(0x1B, 0x0100), (0x0F, 0x0101)]);
        let descriptor = caption_service_descriptor(&[CaptionService {
            language: *b"eng",
            kind: ServiceKind::Digital { service_number: 1 },
            easy_reader: false,
            wide_aspect: true,
        }]);

        let count = announce_captions(&mut ts, &descriptor).expect("rewrite");
        assert_eq!(count, 1, "one PMT copy should have been rewritten");
        assert_eq!(ts.len(), 2 * PACKET, "packets must stay 188 bytes");

        let pmt = &ts[PACKET..];
        assert!(video_stream_has_descriptor(pmt, pmt_pid, &descriptor));
        assert_eq!(section_crc_check(pmt), 0, "the fixed CRC must verify");
    }

    #[test]
    fn a_second_pass_changes_nothing() {
        let pmt_pid = 0x1000;
        let mut ts = pat_packet_and_pmt(pmt_pid, &[(0x1B, 0x0100), (0x0F, 0x0101)]);
        let descriptor = caption_service_descriptor(&[CaptionService {
            language: *b"eng",
            kind: ServiceKind::Line21 { field2: false },
            easy_reader: false,
            wide_aspect: false,
        }]);

        announce_captions(&mut ts, &descriptor).expect("first pass");
        let after_first = ts.clone();
        // The second pass finds the descriptor already there and does no work,
        // so nothing on the PMT PID counts as rewritten.
        assert_eq!(
            announce_captions(&mut ts, &descriptor),
            Err(PmtError::NoMapTable)
        );
        assert_eq!(ts, after_first, "a settled stream must not drift");
    }

    #[test]
    fn the_descriptor_reads_back() {
        let pmt_pid = 0x1000;
        let mut ts = pat_packet_and_pmt(pmt_pid, &[(0x1B, 0x0100), (0x0F, 0x0101)]);
        assert_eq!(video_caption_descriptor(&ts), None, "nothing to read yet");

        let descriptor = caption_service_descriptor(&[CaptionService {
            language: *b"eng",
            kind: ServiceKind::Digital { service_number: 1 },
            easy_reader: false,
            wide_aspect: true,
        }]);
        announce_captions(&mut ts, &descriptor).expect("rewrite");

        assert_eq!(
            video_caption_descriptor(&ts).as_deref(),
            Some(descriptor.as_slice())
        );
    }

    #[test]
    fn a_buffer_that_is_not_packets_is_refused() {
        let mut junk = vec![0u8; 100];
        assert_eq!(
            announce_captions(&mut junk, &[0x86, 0x02, 0xE0, 0x00]),
            Err(PmtError::NotTransportStream)
        );
    }

    /// Two packets: a PAT then its PMT.
    fn pat_packet_and_pmt(pmt_pid: u16, streams: &[(u8, u16)]) -> Vec<u8> {
        let mut ts = psi_packet(0x0000, &pat_section(pmt_pid));
        ts.extend_from_slice(&psi_packet(pmt_pid, &pmt_section(streams)));
        ts
    }

    /// Re-parse a PMT packet and confirm the video stream now carries exactly
    /// the descriptor we inserted.
    fn video_stream_has_descriptor(packet: &[u8], _pid: u16, descriptor: &[u8]) -> bool {
        let section = section_start(packet).unwrap();
        let length = section_length(packet, section).unwrap();
        let section_end = section + 3 + length;
        let program_info_length =
            (((packet[section + 10] as usize) & 0x0F) << 8) | packet[section + 11] as usize;
        let mut at = section + 12 + program_info_length;
        let streams_end = section_end - 4;
        while at + 5 <= streams_end {
            let stream_type = packet[at];
            let es_info_length =
                (((packet[at + 3] as usize) & 0x0F) << 8) | packet[at + 4] as usize;
            let loop_start = at + 5;
            let loop_end = loop_start + es_info_length;
            if VIDEO_STREAM_TYPES.contains(&stream_type) {
                return packet[loop_start..loop_end]
                    .windows(descriptor.len())
                    .any(|window| window == descriptor);
            }
            at += 5 + es_info_length;
        }
        false
    }

    /// Run the CRC over the whole PMT section including its CRC, which a valid
    /// table checks to zero.
    fn section_crc_check(packet: &[u8]) -> u32 {
        let section = section_start(packet).unwrap();
        let length = section_length(packet, section).unwrap();
        crc32_mpeg2(&packet[section..section + 3 + length])
    }
}
