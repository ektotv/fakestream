# fakestream

Generates synthetic test video for people building AV players, and serves it
over HTTP.

Every output is made from nothing. There are no seed files and no third-party
sample clips, so there is nothing to license and nothing to download. Caption
text is generated lorem ipsum.

It covers the caption and subtitle formats a real provider might send, in-band
CEA-608 and 708, DVB bitmaps, the text formats, HLS in both segment types, and
live streams. A player can be tested against all of them without needing a live
provider.

Licensed GPL-3.0.

## Usage

```
fakestream serve
```

Open <http://localhost:8080> for the list of streams, and point a player at any
of them.

Fixtures are generated the first time they are requested and cached afterwards,
so the first play of one takes a few seconds and every later play is immediate.
Live streams are produced as they are watched and never touch the disk.

| flag | |
| --- | --- |
| `--dir PATH` | where fixtures are cached, default `./fixtures` |
| `--port PORT` | port to listen on, default 8080 |
| `--quiet` | drop the progress bar, keeping one line per fixture |
| `--verbose` | let ffmpeg log everything, for diagnosing a bad file |

## What it produces

| path | what it tests |
| --- | --- |
| `vod/basic.mp4` | playback, seeking and AV sync, with no captions |
| `vod/cea608.ts` | CEA-608 hidden in the video, announced by nothing |
| `vod/cea608-dual.ts` | choosing between caption channels CC1 and CC2 |
| `vod/cea708.ts` | CEA-708 service data alongside 608, as a broadcast carries both |
| `vod/dvbsub.ts` | DVB bitmap subtitles on their own track |
| `vod/tx3g.mp4` | tx3g timed text, the usual subtitles inside MP4 |
| `vod/ttml.mp4` | TTML as `stpp`, what DASH and CMAF use |
| `vod/subrip.mkv` | SubRip, plain text with no styling |
| `vod/ass.mkv` | ASS, which carries styling and positioning |
| `vod/webvtt.mkv` | WebVTT away from HLS |
| `vod/multilingual.mkv` | four subtitle tracks, tagged eng, fra, spa and jpn |
| `hls/ts/master.m3u8` | HLS with MPEG-TS segments |
| `hls/fmp4/master.m3u8` | HLS with fragmented MP4 segments |
| `live/stream.ts` | endless progressive MPEG-TS, the classic IPTV shape |
| `live/hls/master.m3u8` | live HLS, a rolling window that cannot be seeked before |

Every clip beeps on each second boundary and flashes a white marker on the same
frame, so audio and video sync can be judged by eye and ear at once. Captions
are numbered, so a dropped or mistimed one is obvious. Live pictures also carry
a UTC clock, elapsed time and a frame counter, which makes end to end latency
measurable by holding a real clock next to the screen.

To generate everything up front instead of on demand:

```
fakestream build
```

## Building it

```
cargo build --release
```

The binary lands at `target/release/fakestream`.

It links the ffmpeg libraries on the machine and needs ffmpeg 8 installed to
run. A self-contained build with ffmpeg linked in is not done yet.
