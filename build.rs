//! Compiles the vendored libcaption directly, rather than through its CMake.
//!
//! Two reasons to skip the upstream build system. Its `CMakeLists.txt` declares
//! `cmake_minimum_required(VERSION 2.8)` and CMake 4 removed compatibility
//! below 3.5, so it needs a policy override to configure at all. And compiling
//! the sources here keeps everything inside cargo, which is what makes a single
//! self-contained binary straightforward.

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Every translation unit we need. The library also ships SCC, SRT, VTT and
/// DVTCC readers, which fakestream has no use for.
const SOURCES: &[&str] = &[
    "caption.c",
    "cea708.c",
    "eia608.c",
    "eia608_charmap.c",
    "mpeg.c",
    "utf8.c",
    "xds.c",
];

/// The UTF-8 to 608 character mapping is generated from a re2c grammar, and
/// upstream does not commit the result, shipping a `.cached` copy instead. We
/// use that copy rather than requiring re2c, which is exactly what upstream's
/// own build does when the tool is absent.
const CACHED_SOURCE: &str = "eia608_from_utf8.c.cached";

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

    // cc picks the language from the file extension, and `.cached` means
    // nothing to it, so the copy lands in OUT_DIR under a name it understands.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let generated = out_dir.join("eia608_from_utf8.c");
    let cached = sources.join(CACHED_SOURCE);
    fs::copy(&cached, &generated)
        .unwrap_or_else(|error| panic!("could not stage {}: {error}", cached.display()));
    build.file(&generated);
    println!("cargo:rerun-if-changed={}", cached.display());

    println!("cargo:rerun-if-changed={}", headers.display());
    build.compile("caption");

    link_windows_dependencies();
}

/// Name the libraries ffmpeg needs on Windows, when pkg-config is not in play.
///
/// Everywhere else pkg-config names these. On Windows rusty_ffmpeg has no
/// pkg-config support at all, it is compiled out, so it is pointed at a library
/// directory instead and links ffmpeg's own libraries and nothing else. Without
/// these the build fails with a wall of unresolved symbols naming Windows APIs
/// rather than anything in this project.
fn link_windows_dependencies() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // Only for the library-directory route. With pkg-config these come from
    // ffmpeg's own .pc files, which stay correct as its dependencies change.
    let Some(libs) = env::var_os("FFMPEG_LIBS_DIR") else {
        return;
    };

    println!(
        "cargo:rustc-link-search=native={}",
        Path::new(&libs).display()
    );
    println!("cargo:rustc-link-lib=static=x264");

    // Taken from ffmpeg's configure.
    for library in [
        "advapi32", "bcrypt", "gdi32", "mfplat", "mfuuid", "ole32", "oleaut32", "psapi", "secur32",
        "shlwapi", "strmiids", "user32", "uuid", "vfw32", "ws2_32",
    ] {
        println!("cargo:rustc-link-lib={library}");
    }
}
