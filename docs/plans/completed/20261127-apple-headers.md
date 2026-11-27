# Apple's headers for C

Done: 2026-11-27

macOS's targets built on Linux and Windows get the headers of CoreFoundation and SystemConfiguration, from Apple's open source, so C that includes them builds: curl-sys (`SCDynamicStoreCopySpecific.h`), and rdkafka-sys through it. Before, both failed for macOS from every host. x86_64's default deployment target goes to 10.13, for the aligned `new` that rocksdb's and duckdb's C++ take.

## Choice

Apple's SDK from Apple's CDN builds anything, but its licence, the Xcode and Apple SDKs Agreement, allows it only on Apple's hardware (§2.7 forbids using it on other computers "or to enable others to do so"), so it stays out, as the frameworks plan decided. Apple publishes the sources of some frameworks under open licences, headers included; cargo-solo fetches them from GitHub, pinned by SHA-256, as the packages are.

- CoreFoundation: Swift's, swift-corelibs-foundation 6.4.3 (Apache 2.0), Apple's own CoreFoundation for Swift on Linux and Windows, 2.3 MB. CF-1153.18, the last CoreFoundation of macOS's open source (2015), is too old: it lacks `CF_ASSUME_NONNULL_BEGIN`, which configd's headers take.
- Swift's CoreFoundation is set to macOS's runtime by three patches: `DEPLOYMENT_RUNTIME_SWIFT` 0, without which `CFSTR` makes Swift's class of constant strings, and its umbrella header without Swift's own headers, `ForSwiftFoundationOnly.h` and `CFConstantKeys.h`. A patch that doesn't fit fails, so a newer pin is checked by hand.
- SystemConfiguration: configd 1405.120.5 (APSL 2.0), its 15 public headers, as Xcode's SDK has them, 1.1 MB. Apple's build strips bridgeOS from their availability marks, which only Apple's clang knows; cargo-solo does too.
- CoreFoundation's stub gains what objc2 leaves out and C calls: the `va_list` forms of its variadic functions, `CFMakeCollectable`, and what its macros call, `__CFStringMakeConstantString`, `__CFRangeMake` and `CFSTR`'s class, `__CFConstantStringClassReference`.
- Checked against Xcode's SDK on a Mac, which its licence allows: of the declarations both have, 1149 functions and externs have the same types on arm64 and x86_64 (`__builtin_types_compatible_p`), and 386 enum values and 131 types' sizes and alignments match; Xcode's have 249 declarations more, such as `CFFileSecurity`, `CFStringTokenizer`, `CFFileDescriptor` and `CFXMLParser`.
- From the Apple SDK branch, which isn't published, what holds without it: the 10.13 default, `fetch` resuming a cut download (`-C -`), and links to folders made copies (a framework's `Headers`).

## Steps

- [x] `packages`: Swift's CoreFoundation and configd in `DARWIN`, `.tar.gz` unpacked; `sdk` patches them; CoreFoundation's stub gains `CF_EXTRA`; `fetch` resumes.
- [x] `unpack`: `materialize` copies links to folders after the links in them.
- [x] `host`: the deployment target at least 10.13; `scripts/e2e.py` checks it.
- [x] tests/hello on macOS: C with CoreFoundation's and SystemConfiguration's headers, `CFSTR` against a string made at run time.
- [x] Unit tests: the folder links, the SDK's layout with the patched headers.
- [x] `scripts/pins.py`: the new pins bumped by hand.
- [x] readme, how-it-works.

## Not here

- Security's headers: its source is a 35 MB tarball, with its headers spread over its folders. So C that includes them fails (curl's `apple-sectrust`), and so does SystemConfiguration's umbrella header, `SystemConfiguration.h`, which takes them through `SCPreferences.h`, as do `SCNetworkConfiguration.h` and `SCNetworkConnection.h`.
- Other frameworks' headers (AppKit, Metal, CoreAudio), closed source: `SDKROOT` or a Mac, as before.
- The 249 declarations Xcode's CoreFoundation has and Swift's lacks.

## Result

A crate with curl-sys (static, its own OpenSSL) and rdkafka-sys (cmake, with curl) built for both Apple targets in an arm64 Linux container on this Mac (Debian trixie, clang 19, Rust 1.100), with an empty cache and no `SDKROOT`, every file fetched from its mirror, and ran here, x86_64 through Rosetta: `ok curl 8.22.0-DEV OpenSSL/3.6.3`, `ok rdkafka 2.16.0`. Each loads only `/usr/lib/` and `/System/Library/Frameworks/`; minimum macOS 11.0 and 10.13. CoreFoundation's headers and SystemConfiguration's that don't take Security's compiled with `-Wall -Wextra -Werror` for both. `scripts/e2e.py build` in the same container built tests/hello and tests/cxx for every target, and `run` here ran all but Windows'. The SDK takes 21 MB on disk, 3.5 MB more to download. The crates run for `aarch64-apple-darwin` (https://github.com/vladkens/cargo-solo/actions/runs/45922300965): all 20 built on Linux and ran on macOS, curl and rdkafka among them; on Windows all but jemalloc and lmdb, which fail there for every target, as before.
