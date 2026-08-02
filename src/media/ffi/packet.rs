//! Packet construction and timestamp handling.

use super::FfiError;
use rsmpeg::avcodec::AVPacket;
use rsmpeg::ffi;

/// Wrap encoded bytes in a packet ffmpeg owns.
pub fn from_bytes(bytes: &[u8]) -> Result<AVPacket, FfiError> {
    let mut packet = AVPacket::new();

    // SAFETY: av_new_packet allocates the buffer the packet then owns and frees
    // through its own unref, so ownership stays with ffmpeg throughout.
    unsafe {
        let raw = packet.as_mut_ptr();
        if ffi::av_new_packet(raw, bytes.len() as i32) < 0 {
            return Err(FfiError::OutOfMemory);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), (*raw).data, bytes.len());
    }

    Ok(packet)
}

/// Send a packet to a stream without touching its timestamps.
///
/// Encoded video carries its own pts and dts, and with B-frames those differ.
/// Overwriting dts with pts breaks the muxer's monotonic ordering and it will
/// reject the packet.
pub fn route(packet: &mut AVPacket, stream_index: i32) {
    // SAFETY: a scalar write into a packet this process owns.
    unsafe {
        (*packet.as_mut_ptr()).stream_index = stream_index;
    }
}

/// Set stream and timing together, for streams whose timing we generate.
pub fn stamp(packet: &mut AVPacket, stream_index: i32, pts: i64, duration: i64) {
    // SAFETY: scalar writes into a packet this process owns.
    unsafe {
        let raw = packet.as_mut_ptr();
        (*raw).stream_index = stream_index;
        (*raw).pts = pts;
        (*raw).dts = pts;
        (*raw).duration = duration;
    }
}

/// Convert a packet's timestamps between time bases.
///
/// Necessary because `write_header` may replace the time base you asked for.
/// MPEG-TS always does, forcing 90kHz, and timestamps left in another base are
/// then misread rather than rejected.
pub fn rescale(packet: &mut AVPacket, from: ffi::AVRational, to: ffi::AVRational) {
    // SAFETY: ffmpeg's own arithmetic over a packet this process owns.
    unsafe {
        ffi::av_packet_rescale_ts(packet.as_mut_ptr(), from, to);
    }
}
