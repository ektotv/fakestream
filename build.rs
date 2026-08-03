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

/// Name the libraries ffmpeg needs, on the route that does not use pkg-config.
///
/// Everywhere else pkg-config names these. On Windows rusty_ffmpeg has no
/// pkg-config support at all, it is compiled out behind a cfg, so it is pointed
/// at a library directory instead and links ffmpeg's own libraries and nothing
/// else. Everything they in turn depend on is ours to name.
///
/// Two sources, because neither alone was enough. ffmpeg's own pkg-config
/// files name what it was built against, such as x264 and iconv, and stay
/// correct as that changes. The Windows system libraries are added regardless,
/// since the .pc files do not always mention them and each omission costs a
/// full CI round trip to find.
fn link_windows_dependencies() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let Some(libs) = env::var_os("FFMPEG_LIBS_DIR") else {
        return;
    };
    let libs = Path::new(&libs);

    println!("cargo:rustc-link-search=native={}", libs.display());

    let mut named = Vec::new();
    let mut searched = Vec::new();

    for entry in fs::read_dir(libs.join("pkgconfig"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pc") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };

        for line in contents.lines() {
            let Some(flags) = line
                .strip_prefix("Libs:")
                .or_else(|| line.strip_prefix("Libs.private:"))
            else {
                continue;
            };

            for flag in flags.split_whitespace() {
                if let Some(name) = flag.strip_prefix("-l") {
                    // ffmpeg's own libraries are linked by rusty_ffmpeg.
                    if !name.starts_with("av") && !name.starts_with("sw") {
                        push_unique(&mut named, name);
                    }
                } else if let Some(dir) = flag.strip_prefix("-L")
                    && !dir.contains("${")
                {
                    push_unique(&mut searched, dir);
                }
            }
        }
    }

    // Windows system libraries ffmpeg uses, whether or not its pkg-config
    // files mention them. They are stable parts of the OS, so naming them
    // unconditionally costs nothing and stops a missing one turning into
    // another round trip through CI.
    for name in [
        "advapi32", "bcrypt", "gdi32", "mfplat", "mfuuid", "ole32", "oleaut32", "psapi", "secur32",
        "shlwapi", "strmiids", "user32", "uuid", "vfw32", "ws2_32",
    ] {
        push_unique(&mut named, name);
    }

    for dir in searched {
        println!("cargo:rustc-link-search=native={dir}");
    }
    // Raw linker args rather than rustc-link-lib. Cargo places this crate's
    // rustc-link-lib flags before the dependency rlibs, but the objects that
    // need these symbols are ffmpeg's, bundled inside rusty_ffmpeg's rlib, and
    // GNU ld resolves strictly left to right. rustc-link-arg flags land at the
    // end of the link command, after the rlibs, where ld can resolve them.
    for name in &named {
        println!("cargo:rustc-link-arg=-l{name}");
    }

    // The mingw C runtime once more, after everything above. rustc names these
    // libraries itself, but earlier in the link command, and GNU ld does not
    // rescan an archive it has already passed, so the objects pulled from
    // libx264.a and ffmpeg at the end of the line found no runtime left to
    // resolve against. gcc's own default link names the runtime twice for the
    // same reason. moldname is the mapping from POSIX names such as fileno to
    // their underscored msvcrt forms, and rustc does not name it at all.
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        for name in ["moldname", "mingwex", "msvcrt", "kernel32"] {
            println!("cargo:rustc-link-arg=-l{name}");
        }
    }

    // Printed so a link failure can be diagnosed from the CI log rather than
    // by another guess at what ffmpeg wanted.
    println!("cargo:warning=linking against: {}", named.join(" "));
}

/// Push a value unless it is already present, keeping first-seen order. The
/// link command repeats libraries across .pc files, and naming one twice is
/// harmless but noisy.
fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|seen| seen == value) {
        values.push(value.to_string());
    }
}
