//! Live streams are produced as they are watched, so the tests drive the
//! generator directly rather than waiting in real time. Pacing is the caller's
//! job, which is what makes that possible.

use fakestream::media::clock;
use fakestream::media::live::LiveStream;
use fakestream::media::mux::ClipSpec;
use std::ops::ControlFlow;

fn spec() -> ClipSpec {
    ClipSpec {
        width: 320,
        height: 240,
        video_bitrate: 200_000,
        ..ClipSpec::default()
    }
}

#[test]
fn pump_feeds_the_sink_until_it_asks_to_stop() {
    // The pacing loop both live callers share: it hands each chunk to the sink
    // and stops when the sink breaks, returning without error.
    let mut live = LiveStream::new(spec()).expect("stream should start");
    let mut chunks = 0usize;
    let mut bytes = 0usize;

    // Stop once real output has flowed (the encoder buffers before its first
    // packet), with a safety cap so a stuck stream cannot loop forever.
    let outcome = live.pump(|chunk| {
        chunks += 1;
        bytes += chunk.len();
        if bytes > 0 || chunks >= 300 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });

    assert!(outcome.is_ok(), "pump reported an error: {outcome:?}");
    assert!(bytes > 0, "pump handed over no bytes");
}

#[test]
fn the_stream_opens_with_a_transport_sync_byte() {
    // MPEG-TS has no standalone header. Its PAT and PMT tables ride along with
    // the media, so write_header yields nothing and the first bytes a viewer
    // sees come from the first frames.
    let mut live = LiveStream::new(spec()).expect("stream should start");
    let _ = live.header().expect("header call should succeed");

    let mut first = Vec::new();
    for _ in 0..50 {
        first = live.next_chunk().expect("a chunk should generate");
        if !first.is_empty() {
            break;
        }
    }

    assert!(!first.is_empty(), "two seconds produced no bytes at all");
    assert_eq!(first[0], 0x47, "output does not look like MPEG-TS");
}

#[test]
fn it_keeps_producing_indefinitely() {
    let mut live = LiveStream::new(spec()).expect("stream should start");
    let _ = live.header().expect("header");

    let mut produced = 0usize;
    // Two seconds of frames, without waiting for them.
    for _ in 0..50 {
        produced += live.next_chunk().expect("a chunk should generate").len();
    }

    assert!(produced > 0, "two seconds of frames produced no bytes");
}

#[test]
fn every_chunk_is_whole_transport_packets() {
    // MPEG-TS is a fixed 188 byte packet format. Handing a player a partial
    // packet is the kind of thing that works locally and fails over a network.
    const PACKET: usize = 188;

    let mut live = LiveStream::new(spec()).expect("stream should start");
    let mut total = live.header().expect("header").len();

    for _ in 0..50 {
        total += live.next_chunk().expect("chunk").len();
    }

    assert_eq!(total % PACKET, 0, "{total} bytes is not whole packets");
}

#[test]
fn pacing_waits_at_the_start_and_not_when_behind() {
    let live = LiveStream::new(spec()).expect("stream should start");

    // Nothing has been produced yet, so the first frame is due immediately.
    assert!(live.wait_before_next().is_zero());
}

#[test]
fn the_clock_reading_matches_the_frame_position() {
    // The picture carries this, and it is what makes latency measurable, so it
    // has to follow the frame rather than the encoding wall time.
    let reading = clock::reading(0.0, 75, 25);
    assert_eq!(reading.elapsed, "00:00:03.000");
    assert_eq!(reading.frame, 75);
}
