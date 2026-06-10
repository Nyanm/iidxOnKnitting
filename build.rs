//! Link the vendored libopus (referenced by the static avcodec.lib) plus the Windows
//! system libraries that the static libav archives depend on. ffmpeg-sys-the-third already
//! emits the search path and link directives for avcodec/avformat/avutil/swresample from
//! FFMPEG_DIR, but it does NOT know about libopus or the Win32 system libs in that mode.

use std::env;

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");

    // libopus: avcodec.lib was built against it and carries unresolved opus_* symbols.
    println!("cargo:rustc-link-search=native={manifest}/vendor/lib");
    println!("cargo:rustc-link-lib=static=opus");

    // Win32 libraries the static libav archives reference (e.g. avutil's BCryptGenRandom).
    // Listing a lib that ends up unreferenced is harmless to the MSVC linker.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        for lib in ["bcrypt", "user32", "ole32", "ws2_32", "secur32", "advapi32", "shell32"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
}
