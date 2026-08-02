#!/usr/bin/env bash
#
# Builds a static ffmpeg for fakestream to link against, so the finished binary
# carries no ffmpeg dependency of its own.
#
# Only what fakestream uses is enabled. A default ffmpeg build takes several
# minutes; this one takes well under one, and cargo caches the result so it is
# paid once per clean checkout.
#
# Usage:
#   ./scripts/build-ffmpeg.sh
#   FFMPEG_PKG_CONFIG_PATH=$PWD/third_party/ffmpeg/lib/pkgconfig \
#     cargo build --release --no-default-features

set -euo pipefail

VERSION="8.0.1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="$ROOT/third_party/ffmpeg"
WORK="$PREFIX/build"

if [ -f "$PREFIX/lib/pkgconfig/libavcodec.pc" ]; then
  echo "static ffmpeg already built at $PREFIX"
  echo "delete that directory to rebuild"
  exit 0
fi

# Rebuilding ffmpeg after the first time also needs cargo told, or it keeps the
# link configuration its build script worked out before and quietly links the
# old one:
#
#   cargo clean

mkdir -p "$WORK"
cd "$WORK"

# x264 is built here rather than taken from the system. Homebrew ships both a
# static and a shared library in one directory, and the linker prefers the
# shared one, which would leave the finished binary depending on it.
if [ ! -f "$PREFIX/lib/libx264.a" ]; then
  echo "building x264"
  if [ ! -d x264 ]; then
    git clone -q --depth 1 https://code.videolan.org/videolan/x264.git
  fi
  (
    cd x264
    ./configure --prefix="$PREFIX" --enable-static --disable-cli --disable-opencl
    make -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)"
    make install
  )
fi

if [ ! -d "ffmpeg-$VERSION" ]; then
  echo "fetching ffmpeg $VERSION"
  curl -fsSL -o "ffmpeg-$VERSION.tar.xz" "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz"
  tar xf "ffmpeg-$VERSION.tar.xz"
fi

cd "ffmpeg-$VERSION"

# Everything fakestream needs and nothing else. --disable-programs is what makes
# this quick: we want the libraries, not the ffmpeg and ffprobe binaries.
#
# GPL is enabled for libx264, which matches this project's own licence.
#
# Component names here are the build system's, not the ones a codec goes by at
# runtime. tx3g is the codec `mov_text` and the component `movtext`, and getting
# that wrong leaves the encoder out with no complaint from configure.
#
# The list has to cover every name the tool asks for by hand, which for
# subtitles is ass, dvbsub, mov_text, subrip, ttml and webvtt, plus the ttml
# muxer that the mp4 muxer instantiates for itself.
#
# xlib and friends are disabled explicitly. --disable-everything turns off
# components, not external libraries, so ffmpeg would otherwise detect X11 and
# leave the binary depending on it for no reason.
# So ffmpeg finds the x264 built above rather than the system one.
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"

./configure \
  --prefix="$PREFIX" \
  --disable-programs \
  --disable-doc \
  --disable-shared \
  --enable-static \
  --enable-gpl \
  --enable-version3 \
  --enable-libx264 \
  --disable-everything \
  --enable-encoder=libx264,aac,dvbsub,movtext,webvtt,ttml,subrip,ass,ssa \
  --enable-decoder=h264,aac,ccaption,subrip,webvtt,ass,dvbsub,movtext \
  --enable-muxer=mpegts,mp4,mov,hls,matroska,webvtt,ttml,srt,ass,segment,stream_segment \
  --enable-demuxer=mpegts,mov,matroska,srt,webvtt,ass,lavfi \
  --enable-parser=h264,aac,dvbsub \
  --enable-protocol=file,pipe \
  --enable-filter=testsrc2,sine,aresample,aformat,scale,format,null,anull \
  --enable-bsf=h264_mp4toannexb,extract_extradata \
  --disable-xlib \
  --disable-libxcb \
  --disable-sdl2 \
  --pkg-config-flags=--static \
  --extra-cflags="-I$PREFIX/include" \
  --extra-ldflags="-L$PREFIX/lib"

make -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)"
make install

echo
echo "built static ffmpeg at $PREFIX"
echo
echo "now build fakestream against it:"
echo "  FFMPEG_PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig cargo build --release --no-default-features"
