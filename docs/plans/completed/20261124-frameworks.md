# Apple's frameworks for Rust

Done: 2026-11-24

macOS's targets built on Linux and Windows link the frameworks Rust's common crates use, with no Apple SDK: CoreFoundation (chrono's time zone through iana-time-zone), Security (native-tls and rustls-platform-verifier through security-framework), SystemConfiguration (reqwest's system proxies), IOKit (sysinfo) and CoreServices (notify's FSEvents). Before, each failed with `framework not found`, so most network CLIs didn't link for macOS off a Mac.

## Choice

A link needs only a framework's `.tbd`: the names it exports and its install name. Rust's crates bind frameworks by hand (`extern` blocks), so they need no headers. The names come from objc2's crates (`objc2-core-foundation` and the others, 0.3.2, MIT, Apache-2.0 or Zlib), Rust's bindings generated from the SDK's headers: on crates.io, pinned by SHA-256 as the packages are, 0.9 MB for the five. Each crate's `extern "C"` blocks are read for the names of its functions and statics and each `link_name` (Security's `$LEGACYMAC` on x86_64); `translation-config.toml` names its framework. objc2 skips C's variadic functions: core-foundation-sys's six are listed by hand.

- Coverage checked against what the crates that link them declare: security-framework-sys, system-configuration-sys and fsevent-sys all in objc2's; core-foundation-sys all but the six variadic.
- Rejected: stubs made at link time from the undefined names, as `-undefined dynamic_lookup` (flat lookup, a typo found only at run time, and every link through a wrapper of the linker); and Apple's SDK from Apple's CDN, which its licence allows only on a Mac.
- The legal footing is Zig's: its `libSystem.tbd`, which cargo-solo already takes, is names from Apple's stubs; ours are names from permissively licensed crates. No file of Apple's is fetched or shipped.
- Install names are the frameworks' own (`/System/Library/Frameworks/X.framework/Versions/A/X`), which dyld finds in its shared cache on every macOS; an umbrella's subframework's names, as FSEvents' in CoreServices, resolve through the umbrella's re-exports.

## Steps

- [x] `.crate` files unpacked as `.apk`s are (gzip, tar); `Package::objc2`, five rows in `DARWIN`, unpacked into `objc2/` and dropped once read.
- [x] `sdk`: `System/Library/Frameworks/X.framework/X.tbd` for each, for `x86_64`, `arm64` and `arm64e`, where ld64.lld looks under clang's `-isysroot`. The user's `SDKROOT` still wins.
- [x] Unit tests: the parser (comments, strings, `fn` pointers, `'static`, `link_name`), the SDK's layout.
- [x] tests/hello on macOS: the time zone, Security's random bytes, SystemConfiguration's proxies; `scripts/e2e.py` allows `/System/Library/Frameworks/`.
- [x] readme, how-it-works, agents.md's scope, the roadmap.

## Not here

- C that includes a framework's headers (curl-sys, rdkafka-sys, git2's libgit2 with Security): no headers.
- Other frameworks (AppKit, Foundation, Metal, CoreAudio) and `-lobjc`: objc2 has crates for most, a row each when a crate needs one.
- Availability: objc2's bindings are every Apple OS's, with no mark of which has what, so a stub also lists names macOS lacks, such as tvOS' `kSecUseUserIndependentKeychain`. Code that calls one links here and fails at load ("Symbol not found"), where Xcode's SDK fails the link; a crate that builds on a Mac never calls one.
- `scripts/pins.py` doesn't re-pin the crates: one version for all five, bumped by hand.

## Result

A crate with chrono and reqwest 0.12 (native-tls, blocking) built for both Apple targets in an arm64 Linux container on this Mac (Debian trixie, clang 19, rustc 1.100.0) in 27 s, and run here: both printed the time zone (`tz Europe/Sofia +0200`) and `https 200 OK` from `https://example.com`, x86_64 through Rosetta. Each loads Security, SystemConfiguration and CoreFoundation by their framework paths, with libSystem and libiconv; minimum macOS 11.0 and 10.12, the arm64 one ad-hoc signed. tests/hello built the same way ran too. With #21's C++ merged in, duckdb (bundled, C++ and CoreFoundation) built for `aarch64-apple-darwin` the same way and ran here: `ok duckdb 10`, loading libc++, libc++abi and CoreFoundation. The crates run for `aarch64-apple-darwin` (https://github.com/vladkens/cargo-solo/actions/runs/45635474836): git2 (libgit2 with Security) now builds on Linux and Windows and runs on macOS, where it failed before; boring, rocksdb and duckdb wait for C++, curl and rdkafka need the frameworks' headers, and on Windows jemalloc and lmdb fail as before, for every target. The SDK grew from 8.1 to 8.4 MB on disk, the stubs 146 KB.
