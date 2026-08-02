# fakestream

Generates synthetic test video for people building AV players. Every output is
made from nothing, no seed files and no third-party sample clips. Captions are
generated lorem ipsum.

The point is to produce a fixture for any caption and subtitle format a real
provider might plausibly send, so a player can be tested against all of them
without needing access to a live provider.

Licensed GPL-3.0. The system ffmpeg here is built `--enable-gpl
--enable-version3 --enable-libx264`, so GPL-3.0 is the compatible choice.

## Status

Early. The generation spine and the HTTP server work, with one VOD fixture. The
caption formats proven in the spike are being ported in next.

The spike that decided the language and architecture is preserved in commit
`1012378`, and its findings are kept below because they remain the reasoning
behind the design.

## Usage

```
fakestream serve              # generate any missing fixtures, then serve them
fakestream serve --port 9000
fakestream build --dir /tmp/fixtures
```

Fixtures are generated on first run and cached, so starting the server a second
time is immediate. Point a player at the index to see every available URL.

## Why a spike

Most of the target formats are reachable from the ffmpeg command line in a
single invocation. Two are not, and those two decide the architecture.

- **DVB bitmap subtitles** cannot be generated from text by the ffmpeg CLI. It
  refuses with `Subtitle encoding currently only possible from text to text or
  bitmap to bitmap`, because it will not rasterise text into a bitmap subtitle.
  Producing one means building the bitmap rects programmatically against
  libavcodec.
- **CEA-608 and CEA-708** have no encoder in ffmpeg at all. The `-a53cc` option
  on libx264 only passes through caption side data that was already present, so
  it cannot author captions. libcaption is the purpose-built library for this.

Both of those need library-level work rather than process orchestration, so the
language choice matters more than it would for a CLI wrapper.

## Spike design

All three implementations build the same thing, or the comparison means nothing.

**Tier 1** generates a five second H.264 and AAC file entirely through libav*,
with no CLI invocation. This proves the binding builds against the installed
headers and shows the toolchain friction.

**Tier 2a** adds a DVB bitmap subtitle track carrying lorem ipsum, rendered as a
rect and fed to the DVB encoder.

**Tier 2b** injects CEA-608 and CEA-708 captions via libcaption into a video
generated in the same run.

Tier 1 runs in all three languages. Tier 2 runs only in whatever clears tier 1.

### Compared on

- Whether it builds against the installed libavcodec at all.
- How much FFI leaks into our own code rather than staying inside the binding.
- Error handling quality, particularly on the failure paths libav* is full of.
- Build reproducibility and the cross-compilation story.
- For Rust specifically, zero `unsafe` and zero `unwrap` in our own code. The
  binding crate contains unsafe internally, which FFI cannot avoid.

## Environment as spiked

Recorded because the ffmpeg version turned out to matter.

| Component | Version |
| --- | --- |
| ffmpeg | 8.0.1 |
| libavcodec | 62.11.100 |
| libavformat | 62.3.100 |
| libavutil | 60.8.100 |
| clang | 16.0.0 |
| rustc / cargo | 1.94.0 |
| go | 1.26.5 |
| cmake | 4.4.2 |

libavcodec 62 is recent. Both the Rust and Go binding libraries pin to specific
ffmpeg majors and have historically lagged a release behind, so whether they
build here at all is one of the things being measured. C uses the headers
directly and has no such exposure.

## Findings

Recorded as they land, not after.

### CEA-608 needs no caption encoder written, but it does need our code

The whole CEA path works today using libcaption's shipped example tool plus
ffmpeg for packaging, with nothing written by us. Verified end to end from
synthetic sources only.

1. ffmpeg generates a video-only FLV from `testsrc2`.
2. libcaption's `flv+srt` injects a generated lorem ipsum SRT as CEA-608 in
   H.264 SEI.
3. ffmpeg remuxes to MPEG-TS with `-c copy`, so the SEI rides along untouched.
4. ffmpeg muxes the AAC audio in afterwards, again with `-c copy`.

The result was read back with ffmpeg's `subcc` extraction, recovering both cues
with correct timings, which is the same path a player takes.

That proved CEA is achievable without writing a caption encoder. It does not
survive the single binary decision though, because it leans on a separate
process and an FLV intermediate. See the A53 section below for the route we
actually took.

### libcaption constraints found while proving that

- **`flv+srt` is single-track only.** Feeding it an FLV containing both video
  and audio produces a corrupt file that ffmpeg then refuses to remux, failing
  with `Invalid NAL unit size`. The tool says as much in passing with
  `Attempted to read next track in single-track mode`. Audio has to be muxed in
  after injection, never before.
- **The shipped injectors are FLV only**, `flv+srt` and `flv+scc`. There is no
  TS or MP4 injector, so FLV is an unavoidable intermediate unless we write our
  own driver against the library. `ts2srt` is extraction only.
- **libcaption is MIT licensed**, which is compatible with GPL-3.0.
- **It needs `-DCMAKE_POLICY_VERSION_MINIMUM=3.5` to configure**, because its
  `CMakeLists.txt` declares `cmake_minimum_required(VERSION 2.8)` and CMake 4
  removed compatibility below 3.5. Worth pinning in whatever build script we
  end up with.

### Do not verify captions with ffprobe's `closed_captions` field

It stayed unset on the finished TS even though the captions were demonstrably
present and extractable. Use `movie=file[out+subcc]` to read them back instead.

### The Go binding cannot encode subtitles, the Rust one can

Checked both against the installed ffmpeg 8.

`go-astiav` is actively maintained and states it is only compatible with ffmpeg
`n8.0`, which is exactly what is installed, so the version lag risk did not
materialise. It has no subtitle encoding of any kind though. The whole repo
mentions subtitles twice, once as the `MediaTypeSubtitle` enum value and once in
the codec id list. There is no `AVSubtitle` anywhere.

`rsmpeg` defaults to its `ffmpeg8` feature, links the system ffmpeg through
pkg-config, and wraps `avcodec_encode_subtitle` in a safe `encode_subtitle`
method with an `AVSubtitle` type. The unsafe is contained inside the crate, so
calling code needs none.

C has no exposure either way, since it uses the headers directly.

### Building ffmpeg from source is cheap, so the binary can be fully static

Measured on an M1 Max with 10 cores, ffmpeg 8.0.1.

| Step | Time |
| --- | --- |
| configure | 36.5s |
| `make -j10` | 11.2s |
| total | ~48s |

That is with `--disable-everything` and `--disable-programs`, enabling only the
codecs, muxers, demuxers and filters this tool needs, and building the libraries
rather than the `ffmpeg` and `ffprobe` binaries. A default full build is several
minutes, but we have no reason to do one.

The result is 28 MB of static archives. `ff_dvbsub_encoder` and
`avcodec_encode_subtitle` are both present in `libavcodec.a`, confirmed with
`nm`, so the DVB path survives the minimal configure. configure reports the
licence as GPL version 3 or later, matching the repo.

Two caveats. This linked homebrew's prebuilt `libx264.a`, so a fully
reproducible build adds x264 itself, which is small and should stay inside a
couple of minutes in total. And CI runners are typically slower than this
machine, so budget a few minutes there, cached.

The practical consequence is that the earlier worry about static linking turning
a fast build into a slow one was wrong. It is a sub-minute one-time cost that
cargo caches, which makes a fully self-contained executable with no runtime
dependencies the obvious choice over linking the system ffmpeg.

### DVB bitmap subtitles work in Rust, proven by round trip

The last unproven part of the design now runs. `spikes/rust` builds a paletted
bitmap caption, encodes it with ffmpeg's `dvbsub` encoder, then decodes it back
with ffmpeg's own DVB decoder and gets the same rect out.

```
non-zero payload length: 966 bytes
first bytes: [0f, 14, 00, 01, 00, 05, ...]
decoded back: 1 rect(s), display 0..30000ms
  rect 0: 480x80 at (120,456), 16 colours
```

The geometry matches what went in exactly. The leading bytes are well formed DVB
too, `0f` sync then `14` display definition, then `0f 10` for page composition.

Two details worth remembering. The encoder promoted our four colour palette to a
16 entry CLUT, which is normal since DVB CLUTs are 2, 4 or 8 bit. And the decoded
end time reads 30000ms rather than the 3000ms we set, because that is the DVB
page timeout rather than our cue duration, so display length has to be driven by
packet timing rather than by `end_display_time`.

### The full pipeline works, verified by an independent player

`spikes/rust` now generates an MPEG-TS from nothing, holding H.264 video and a
DVB subtitle track, and the system ffmpeg decodes and renders it.

```
Duration: 00:00:06.00
Stream #0:0: Video: h264 (High), yuv420p, 720x576, 25 fps
Stream #0:1: Subtitle: dvb_subtitle (dvbsub)
```

Subtitle packets land at 1.08s and 4.08s against requested times of 1.0s and
4.0s. The 0.08 offset is MPEG-TS's standard start offset and applies to the
video identically, so relative timing is exact.

Burning the subtitle over the video with `[0:v][0:s]overlay` produces
`docs/dvb-glyphs-latin.png` and `docs/dvb-glyphs-accented.png`. Reproduce with:

```
ffmpeg -copyts -i dvb.ts -filter_complex "[0:v][0:s]overlay" -ss 1.5 -frames:v 1 out.png
```

### Muxer time bases are not the ones you set

`write_header` is free to replace the time base on a stream, and MPEG-TS always
does, forcing 90kHz. Timestamps prepared in any other base are then silently
misread rather than rejected. The first working build put cues at 0.011s and
0.044s instead of 1s and 4s, because 1000 was taken as 1000/90000.

Read the time base back from the stream after `write_header` and rescale every
packet into it. This applies to video too, not just subtitles.

Related, encoded video carries its own pts and dts, and with B-frames they
differ. Overwriting dts with pts breaks the muxer's monotonic ordering and it
refuses the packet.

### Real glyph rendering works, including non-ASCII

Text is rasterised with `ab_glyph` against a bundled Noto Sans, then quantised
into the palette. Both proof images show readable antialiased captions in a box
sized to its own text, `docs/dvb-glyphs-latin.png` reading "Lorem ipsum dolor sit
amet" and `docs/dvb-glyphs-accented.png` carrying an em dash plus é, è, ü and ß.

The palette is fixed so the renderer and any visual check agree. Index 0 is
transparent, 1 the box background, 2 the border, and 3 upwards a four step
coverage ramp for antialiasing. DVB CLUTs are 2, 4 or 8 bit, so a small ramp is
all there is room for, and seven entries sits comfortably inside the 16 the
encoder allocates.

Shaping was deliberately left out. For DVB the player only blits our bitmap, so
complex scripts would be our rendering problem rather than something the fixture
tests. For the text formats the player does its own rendering and we emit UTF-8,
so a CJK or RTL fixture needs no rasterisation from us at all. If a CJK DVB
fixture is ever wanted, that is where shaping would be added.

The font is Noto Sans under OFL 1.1, bundled in `assets/fonts` with its licence,
so the finished binary depends on no system fonts.

### CEA-608 rides on A53 frame side data, no FLV and no second process

The single binary design cannot shell out to libcaption's tool, so captions go
in through ffmpeg's own path instead. `libx264.c` calls `ff_alloc_a53_sei` on
every frame, which reads `AV_FRAME_DATA_A53_CC` side data and emits the SEI
itself. That is what the `a53cc` option, on by default, actually does.

So the shape is caption text to `cc_data` triplets, attached as side data on each
AVFrame, and the encoder does the ATSC wrapping. ffmpeg wants only the raw
triplets, and adds the country code, `GA94`, type byte, count and trailing marker
around them.

Proven by extracting with an independent ffmpeg:

```
00:00:01,760 --> 00:00:03,480  Lorem ipsum dolor sit amet
00:00:04,920 --> 00:00:05,480  Consectetur adipiscing elit - ee
```

The same MPEG-TS carries both caption systems at once, DVB as a bitmap track and
608 in the video's SEI, and each decodes independently.

The spike hand-rolls the 608 byte pairs so the proof isolates the ffmpeg
plumbing rather than a C link. Production should hand that job to libcaption,
which covers roll-up, the full character map and 708 wrapping. Hand-rolling
pop-on captions turned out to be about 100 lines, so the dependency is a
convenience rather than a necessity.

### Two CEA-608 behaviours that will bite

**Rows cap at 32 characters.** The second cue above reads
`Consectetur adipiscing elit - ee`, which is exactly 32 characters, silently
truncated from 34. Caption text has to be wrapped before transmission, not after.

**Captions display late by however long they take to transmit.** One byte pair
rides per frame, so a caption is only shown once its whole pair stream has been
sent and the end-of-caption code lands. The cues above were scheduled at 1.0s and
4.0s and appeared at 1.76s and 4.92s, the difference being 19 and 23 frames of
transmission. Scheduling has to run ahead of the intended display time by the
length of the stream.

### How much unsafe this actually costs

The spike is 775 lines across five files, and every unsafe operation is confined
to two modules that expose safe APIs.

| File | Lines | Unsafe blocks | Purpose |
| --- | --- | --- | --- |
| `main.rs` | 247 | 0 | encoders, muxing, cue and caption timing |
| `subtitle.rs` | 179 | 6 | subtitle rects, packet data, rescaling |
| `text.rs` | 158 | 0 | glyph rasterising, palette |
| `cea608.rs` | 109 | 0 | 608 byte pairs, parity, control codes |
| `frame.rs` | 82 | 3 | frame pixels and A53 side data |

The muxing and orchestration code, which is where the tool will actually grow,
needs no unsafe at all.

rsmpeg's coverage of these paths is thinner than it first looked. It wraps
`avcodec_encode_subtitle` and `avcodec_decode_subtitle2` safely, but
`AVSubtitle::new()` produces an empty subtitle with no way to add rects,
`AVPacket` has no safe way to attach data, and `AVFrame` has no safe way to write
pixels or attach side data. All of those gaps land in our code.

So roughly 120 lines of audited unsafe across two modules buys the entire media
layer, and nothing outside them needs any. That is the containment Rust was
chosen for, and it is a far smaller surface than the equivalent cgo, since Go's
pointer rules would force every bitmap and pixel byte through C-allocated memory
along the whole call path rather than at one boundary.

## Conclusion

Build it in Rust, as one statically linked executable, with everything going
through libav* rather than the ffmpeg command line.

The spike answered more than it set out to. Three findings drove the decision.

**DVB was the only real discriminator, and it is settled.** Every other format
is reachable either from the ffmpeg CLI in one invocation or, for CEA, from
libcaption's shipped tool. DVB alone needs library-level access, and it now
works, verified by an independent player rather than by our own code agreeing
with itself.

**A single executable rules out the CLI.** Shelling out to ffmpeg means a second
binary whichever language is chosen, so the constraint forces the all-library
design. That in turn makes binding quality matter across the whole tool rather
than in one corner, which is where Go fell down. `go-astiav` tracks ffmpeg 8
exactly but has no subtitle support at all, so the unsafe would have sat in the
main path instead of at one boundary.

**Static linking is cheap.** A minimal ffmpeg builds in under a minute and cargo
caches it, so a self-contained binary with no runtime dependencies costs
effectively nothing.

The three-language comparison was abandoned deliberately once the evidence
decided it. Implementing tier 1 in C, Go and Rust would only have measured how
pleasant each is at spawning processes, which is not a question worth three
implementations.

### What is not yet proven

- HLS packaging driven through libavformat's muxer options rather than the CLI.
- Live serving, which is the looped-asset design described above but not yet
  built.
- Whether `link_system_ffmpeg` can be swapped for a vendored static ffmpeg build
  inside cargo without pain.
