//! Live streams carry captions in the video SEI, generated endlessly rather
//! than from a fixed cue list. These tests drive a LiveStream directly, without
//! real-time pacing, and read the muxed bytes back.

use fakestream::captions::libcaption::Channel;
use fakestream::media::live::LiveStream;
use fakestream::media::mux::ClipSpec;
use fakestream::media::pmt::{self, CaptionService, ServiceKind};

/// A small, cheap-to-encode live spec. Captions are added per test.
fn base() -> ClipSpec {
    ClipSpec {
        width: 320,
        height: 240,
        video_bitrate: 300_000,
        duration_seconds: 0.0,
        ..ClipSpec::default()
    }
}

/// Drive a stream for a number of frames and return everything it emitted,
/// header included. No waiting, since the caller owns pacing.
fn collect(spec: ClipSpec, frames: usize) -> Vec<u8> {
    let mut stream = LiveStream::new(spec).expect("the stream should open");
    let mut bytes = stream.header().expect("header");
    for _ in 0..frames {
        bytes.extend_from_slice(&stream.next_chunk().expect("a chunk"));
    }
    bytes
}

fn line21_service() -> CaptionService {
    CaptionService {
        language: *b"eng",
        kind: ServiceKind::Line21 { field2: false },
        easy_reader: false,
        wide_aspect: true,
    }
}

fn digital_service() -> CaptionService {
    CaptionService {
        language: *b"eng",
        kind: ServiceKind::Digital { service_number: 1 },
        easy_reader: false,
        wide_aspect: true,
    }
}

#[test]
fn a_captioned_live_stream_is_whole_transport_packets() {
    let spec = ClipSpec {
        live_cea608: Some(Channel::One),
        live_cea708: true,
        ..base()
    };
    let bytes = collect(spec, 50);

    assert!(!bytes.is_empty(), "the stream produced nothing");
    assert_eq!(
        bytes.len() % 188,
        0,
        "captions left the output not a whole number of packets"
    );
}

#[test]
fn an_announced_live_stream_carries_the_608_descriptor() {
    let spec = ClipSpec {
        live_cea608: Some(Channel::One),
        announce_captions_in_pmt: true,
        ..base()
    };
    // Enough frames that several PMT copies have gone past.
    let bytes = collect(spec, 120);

    let descriptor =
        pmt::video_caption_descriptor(&bytes).expect("the live PMT should announce the captions");
    assert_eq!(
        descriptor,
        pmt::caption_service_descriptor(&[line21_service()])
    );
}

#[test]
fn an_announced_live_stream_carries_608_and_708() {
    let spec = ClipSpec {
        live_cea608: Some(Channel::One),
        live_cea708: true,
        announce_captions_in_pmt: true,
        ..base()
    };
    let bytes = collect(spec, 120);

    let descriptor =
        pmt::video_caption_descriptor(&bytes).expect("the live PMT should announce both services");
    assert_eq!(
        descriptor,
        pmt::caption_service_descriptor(&[line21_service(), digital_service()])
    );
}

#[test]
fn a_plain_live_stream_announces_nothing() {
    // The plain live stream is caption-free now, so its PMT must stay bare.
    let bytes = collect(base(), 120);
    assert_eq!(
        pmt::video_caption_descriptor(&bytes),
        None,
        "the plain live stream grew a caption descriptor"
    );
}
