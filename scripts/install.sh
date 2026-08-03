#!/bin/sh
# Downloads the latest fakestream release for this machine, verifies it
# against the release's SHA256SUMS, and unpacks it into the current directory.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ektotv/fakestream/main/scripts/install.sh | sh

set -eu

REPO="ektotv/fakestream"

case "$(uname -s)" in
  Darwin) os="macos" ;;
  Linux) os="linux" ;;
  *)
    echo "no prebuilt binary for $(uname -s); see the README for building from source" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  arm64 | aarch64) arch="arm64" ;;
  x86_64) arch="x86_64" ;;
  *)
    echo "no prebuilt binary for $(uname -m); see the README for building from source" >&2
    exit 1
    ;;
esac

platform="${os}-${arch}"
case "$platform" in
  macos-arm64 | linux-x86_64) ;;
  *)
    echo "no prebuilt binary for $platform; see the README for building from source" >&2
    exit 1
    ;;
esac

# The latest tag, read from where the releases/latest page redirects to,
# which ends /releases/tag/vX.Y.Z. No API, so no JSON to parse and no rate
# limit to hit.
tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")
tag="${tag##*/}"
case "$tag" in
  v*) ;;
  *)
    echo "could not find the latest release" >&2
    exit 1
    ;;
esac

archive="fakestream-${tag}-${platform}.tar.gz"
base="https://github.com/$REPO/releases/download/$tag"

echo "downloading fakestream $tag for $platform"
curl -fsSL -O "$base/$archive"
curl -fsSL -O "$base/SHA256SUMS"

# Verified before unpacking. sha256sum on Linux, shasum on macOS; the sums
# file lists all platforms, so the two archives not downloaded are ignored.
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c --ignore-missing SHA256SUMS
else
  shasum -a 256 -c --ignore-missing SHA256SUMS
fi

tar xzf "$archive"
rm "$archive" SHA256SUMS

echo
echo "unpacked fakestream-${tag}-${platform}/"
echo "run it:"
echo "  ./fakestream-${tag}-${platform}/fakestream"
echo
echo "or put it on your PATH:"
echo "  sudo install fakestream-${tag}-${platform}/fakestream /usr/local/bin/"
