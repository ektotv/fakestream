//! Compiles the vendored libcaption directly, rather than through its CMake.
//!
//! Two reasons to skip the upstream build system. Its `CMakeLists.txt` declares
//! `cmake_minimum_required(VERSION 2.8)` and CMake 4 removed compatibility
//! below 3.5, so it needs a policy override to configure at all. And compiling
//! the sources here keeps everything inside cargo, which is what makes a single
//! self-contained binary straightforward.

use std::path::Path;

/// Every translation unit we need. The library also ships SCC, SRT, VTT and
/// DVTCC readers, which fakestream has no use for.
const SOURCES: &[&str] = &[
    "caption.c",
    "cea708.c",
    "eia608.c",
    "eia608_charmap.c",
    "eia608_from_utf8.c",
    "mpeg.c",
    "utf8.c",
    "xds.c",
];

fn main() {
    let root = Path::new("third_party/libcaption");
    let sources = root.join("src");
    // Sources include their headers unqualified, so the header directory itself
    // is the include path rather than the library root.
    let headers = root.join("caption");

    let mut build = cc::Build::new();
    build.include(&headers).warnings(false);

    for name in SOURCES {
        build.file(sources.join(name));
        println!("cargo:rerun-if-changed={}", sources.join(name).display());
    }

    // Our own allocation shim, kept beside the Rust that uses it.
    let shim = Path::new("src/captions/libcaption/shim.c");
    build.file(shim);
    println!("cargo:rerun-if-changed={}", shim.display());

    println!("cargo:rerun-if-changed={}", headers.display());
    build.compile("caption");
}
