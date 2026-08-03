# fakestream

Synthetic test video for people building AV players, generated from nothing
and served over HTTP.

Testing a player against real providers is slow and incomplete. Accounts and
contracts gate the streams, the awkward cases only appear when the schedule
happens to produce them, and nothing about the content is under your control.
Sample files scattered around the internet cover the gaps badly, with unclear
licensing and no way to know what a file is supposed to contain. fakestream
replaces both. One binary serves every stream shape a real provider might
send, on localhost, with content designed so that player bugs identify
themselves:

- A beep sounds on every second boundary and the picture flashes a white
  marker on the same frame, so drift between audio and video is visible and
  audible at once.
- Captions and subtitles are numbered, so a dropped or mistimed cue is a fact
  rather than a feeling.
- Live pictures carry a UTC clock, elapsed time and a frame counter, so end
  to end latency can be measured by holding a real clock next to the screen,
  and a stalled or looping player is obvious.
- Caption channels and subtitle languages carry visibly different text, so a
  player rendering the wrong track betrays itself immediately.

Every output is made from nothing. There are no seed files and no third-party
sample clips, so there is nothing to license and nothing to download beyond
the tool itself. Caption text is generated lorem ipsum.

## Install

Each release ships a self-contained binary per platform, with ffmpeg and x264
built in. Nothing else needs installing.

### macOS (Apple Silicon) and Linux (x86_64)

```sh
curl -fsSL https://raw.githubusercontent.com/ektotv/fakestream/main/scripts/install.sh | sh
```

The script detects the platform, downloads the latest release, checks it
against the release's `SHA256SUMS`, and unpacks it into the current directory.
It never writes outside that directory; putting the binary on your PATH is a
separate step it prints and leaves to you:

```sh
sudo install fakestream-v*/fakestream /usr/local/bin/
```

### Windows (x86_64)

```bat
curl.exe -fsSL -o install.bat https://raw.githubusercontent.com/ektotv/fakestream/main/scripts/install.bat
.\install.bat
```

Two lines rather than one joined with `&&`, which Windows PowerShell does not
accept, and `curl.exe` rather than `curl`, which PowerShell aliases to
something that takes different flags. Written this way it pastes into cmd and
any PowerShell alike.

### Manually

Download the archive for your platform from the
[latest release](https://github.com/ektotv/fakestream/releases/latest), check
it against the release's `SHA256SUMS`, unpack it, and run the binary inside.

On macOS the binary is not signed or notarised. Downloads made with a browser
carry a quarantine flag that blocks it with "cannot be opened"; either allow
it under System Settings, Privacy & Security, or clear the flag:

```sh
xattr -d com.apple.quarantine fakestream
```

Downloads made with `curl`, including the install script above, carry no flag
and run as they are.

Whichever way it arrived, every binary reports exactly what it is:

```
$ fakestream --version
fakestream 0.1.0 (986e444, built 2026-08-03)
```

## Serving streams

```
fakestream serve
```

Running `fakestream` with no command does the same. Open
<http://localhost:8080> for the list of streams with their purposes, and point
a player at any of them.

VOD fixtures are generated the first time they are requested and cached
afterwards, so the first play of one takes a few seconds and every later play
is immediate. Live streams are produced in real time as they are watched. The
progressive stream ends when its viewer disconnects; the live HLS writer is
shared by every viewer and stops about a minute after the last request for
anything in its playlist.

Every request is logged with a UTC timestamp, the client address and the
response, which turns the server into a diagnostic tool in itself; a player
that stops polling a live playlist, or fetches segments it should have
cached, shows up in the log.

```
12:34:56.789Z  127.0.0.1:60447  GET 200 /live/hls/stream0.m3u8 2ms
```

## Generating up front

```
fakestream build
```

Generates every VOD fixture into the cache without serving, which takes a
couple of minutes. Useful ahead of a demo or on a machine that will serve
offline. Live streams have nothing to build, they are produced as watched.

## Options

Both commands take the same flags:

| flag | |
| --- | --- |
| `--dir PATH` | where fixtures are cached, default `./fixtures` |
| `--port PORT` | port to listen on, default 8080 (serve only) |
| `--quiet` | drop the progress bar, keeping one line per fixture |
| `--verbose` | let ffmpeg log everything, for diagnosing a bad file |
| `-v`, `--version` | print the version, commit and build date |
| `-h`, `--help` | print usage |

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
| `hls/ts/master.m3u8` | HLS with MPEG-TS segments and a WebVTT rendition |
| `hls/fmp4/master.m3u8` | HLS with fragmented MP4 segments |
| `live/stream.ts` | endless progressive MPEG-TS, the classic IPTV shape |
| `live/hls/master.m3u8` | live HLS, a rolling window that cannot be seeked before |

Two gaps are known and deliberate. HLS carries one subtitle language, which is
a limit of ffmpeg's HLS muxer rather than a choice, and the multilingual
Matroska fixture covers language selection meanwhile. Live streams carry
CEA-608 but not 708.

## Building from source

For a binary that carries everything with it and needs nothing installed:

```sh
./scripts/build-ffmpeg.sh
FFMPEG_PKG_CONFIG_PATH=$PWD/third_party/ffmpeg/lib/pkgconfig \
  cargo build --release --no-default-features
```

The first step builds a static ffmpeg and x264 into `third_party/ffmpeg`,
which takes about a minute and is only done once. The binary lands at
`target/release/fakestream` and links nothing beyond the operating system's
own libraries.

For development, linking the ffmpeg already on the machine is quicker:

```sh
cargo build
```

That needs ffmpeg 8 installed.

## Licence

GPL-3.0.
