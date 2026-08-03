#!/usr/bin/env bash
# Regenerates THIRD_PARTY_LICENSES, the licence attributions for every Rust
# crate in the binary, which --licenses embeds. Run after dependency changes;
# a release refuses to build if the committed file is stale.
#
# Needs cargo-about:
#   cargo install cargo-about --locked --features cli

set -euo pipefail
cd "$(dirname "$0")/.."

cargo about generate about.hbs -o THIRD_PARTY_LICENSES
echo "wrote THIRD_PARTY_LICENSES"
