# Vendored dependencies

## libcaption

CEA-608 encoding, and the CEA-708 transport that carries captions inside an SEI.
The 708 service layer is not implemented upstream, so `src/captions/cea708.rs`
provides that.

| | |
| --- | --- |
| source | <https://github.com/szatmary/libcaption> |
| commit | `e8b6261090eb3f2012427cc6b151c923f82453db` on `develop` |
| licence | MIT, see `libcaption/LICENSE.txt` |

Vendored whole and unmodified, so it can be diffed against upstream. `build.rs`
compiles only what is needed, seven files from `src/`, and nothing from
`examples/` or `unit_tests/`.

Two things to know when updating it.

Its `CMakeLists.txt` is never used. It declares `cmake_minimum_required(VERSION
2.8)` and CMake 4 has removed compatibility below 3.5, so it will not configure
without a policy override. Compiling the sources directly through the `cc` crate
avoids the problem and keeps everything inside cargo.

`src/eia608_from_utf8.c` is generated from a re2c grammar and upstream does not
commit it, shipping `eia608_from_utf8.c.cached` instead. `build.rs` stages that
cached copy rather than requiring re2c, which is what upstream's own build does
when the tool is absent.
