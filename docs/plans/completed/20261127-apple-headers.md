# Apple's headers for C

Done: 2026-11-27

macOS's targets built on Linux and Windows get the headers of CoreFoundation, SystemConfiguration and Security, from Apple's open source, so C that includes them builds: curl-sys (`SCDynamicStoreCopySpecific.h`), rdkafka-sys through it, and libgit2 with HTTPS (`SecureTransport.h`), git2's default up to 0.20. Before, each failed for macOS from every host. x86_64's default deployment target goes to 10.13, for the aligned `new` that rocksdb's and duckdb's C++ take.

## Choice

Apple's SDK from Apple's CDN builds anything, but its licence, the Xcode and Apple SDKs Agreement, allows it only on Apple's hardware (§2.7 forbids using it on other computers "or to enable others to do so"), so it stays out, as the frameworks plan decided. Apple publishes the sources of some frameworks under open licences, headers included; cargo-solo fetches them from GitHub, pinned by SHA-256, as the packages are.

- CoreFoundation: Swift's, swift-corelibs-foundation 6.4.3 (Apache 2.0), Apple's own CoreFoundation for Swift on Linux and Windows, 2.3 MB. CF-1153.18, the last CoreFoundation of macOS's open source (2015), is too old: it lacks `CF_ASSUME_NONNULL_BEGIN`, which configd's headers take.
- Swift's CoreFoundation is set to macOS's runtime by four patches: `DEPLOYMENT_RUNTIME_SWIFT` 0, without which `CFSTR` makes Swift's class of constant strings; `CFStream`'s dispatch queues on, which Swift's build turns on and Security's headers take dispatch through; and its umbrella header without Swift's own headers, `ForSwiftFoundationOnly.h` and `CFConstantKeys.h`. A patch that doesn't fit fails, so a newer pin is checked by hand.
- SystemConfiguration: configd 1405.120.5 (APSL 2.0), its 15 public headers, as Xcode's SDK has them, 1.1 MB.
- Security: Security 61901.120.67 (APSL 2.0), its 82 public headers from `header_symlinks/`, the folders its build installs from, 35 MB to download, 1.6 MB kept. Its `oids.h` takes libDER's `DERItem`, and libDER isn't open source: the struct, two fields, is written out in its place.
- Apple's build makes an SPI unavailable in configd's and Security's headers, `SPI_AVAILABLE(macos(12.0))` becoming `API_UNAVAILABLE(macos)`, and strips bridgeOS from their availability marks, which only Apple's clang knows; cargo-solo does too, on bytes, as three of Security's headers aren't UTF-8. Zig's `os/availability.h`, as Xcode's, defines the SPI marks as nothing, so without it an SPI would be available. Then 71 of Security's 82 headers and 14 of SystemConfiguration's 15 are byte for byte Xcode's.
- CoreFoundation's stub gains what objc2 leaves out and C calls: the `va_list` forms of its variadic functions, `CFMakeCollectable`, and what its macros call, `__CFStringMakeConstantString`, `__CFRangeMake` and `CFSTR`'s class, `__CFConstantStringClassReference`. Security's gains the 99 it leaves out, functions that take a block, an XPC message or an audit token and `oids.h`'s 80 OIDs, and SystemConfiguration's its variadic `SCDynamicStoreKeyCreate`: every function and extern the three frameworks' headers declare is then in their stubs.
- Checked against Xcode's SDK on a Mac, which its licence allows, on arm64 and x86_64: of the declarations both have, CoreFoundation's and SystemConfiguration's 2252 and Security's 5312, every function and extern the real SDK makes available on macOS has the same type (`__builtin_types_compatible_p`, 1566 and 1563), and every enum value (448 and 2885) and type's size and alignment (149 and 539) match. Xcode's have more: 245 of CoreFoundation's, such as `CFFileSecurity`, `CFStringTokenizer` and `CFXMLParser`, and 16 of Security's, macOS 27's newest, such as code signing's post-quantum signatures.
- From the Apple SDK branch, which isn't published, what holds without it: the 10.13 default, `fetch` resuming a cut download (`-C -`), and links to folders made copies (a framework's `Headers`).

## Steps

- [x] `packages`: Swift's CoreFoundation, configd and Security in `DARWIN`, `.tar.gz` unpacked; `sdk` patches them; CoreFoundation's stub gains `CF_EXTRA`; `fetch` resumes.
- [x] `unpack`: `materialize` copies links to folders after the links in them.
- [x] `host`: the deployment target at least 10.13; `scripts/e2e.py` checks it.
- [x] tests/hello on macOS: C with CoreFoundation's and SystemConfiguration's headers, `CFSTR` against a string made at run time.
- [x] Unit tests: the folder links, the SDK's layout with the patched headers, an SPI made unavailable, bridgeOS stripped.
- [x] `scripts/pins.py`: the new pins bumped by hand.
- [x] readme, how-it-works.

## Not here

- IOKit's headers, which hidapi and libusb (rusb's `vendored`) include: they come from xnu, IOKitUser, IOHIDFamily and IOUSBFamily, no longer published since its 630.4.5; and hidapi also links AppKit, which has no stub.
- Other frameworks' headers (AppKit, Metal, CoreAudio), closed source: `SDKROOT` or a Mac, as before.
- The 261 declarations Xcode's headers have and these lack.

## Result

A crate with curl-sys (static, its own OpenSSL), rdkafka-sys (cmake, with curl) and git2 0.20 with HTTPS (SecureTransport) built for both Apple targets in an arm64 Linux container on this Mac (Debian trixie, clang 19, Rust 1.100), with an empty cache and no `SDKROOT`, every file fetched from its mirror, and ran here, x86_64 through Rosetta: `ok curl 8.22.0-DEV OpenSSL/3.6.3`, `ok rdkafka 2.16.0`, `ok git2 (1, 9, 7) https=true`, git2 having cloned a repository from GitHub over HTTPS. Each loads only `/usr/lib/` and `/System/Library/Frameworks/`; minimum macOS 11.0 and 10.13. `Security.h`, `SecureTransport.h` and `SystemConfiguration.h` compile with `-Wall` for both, warning, as with Xcode, only of what's deprecated or newer than the deployment target. `scripts/e2e.py build` in the same container built tests/hello and tests/cxx for every target, and `run` here ran all but Windows'. The SDK takes 22 MB on disk, 39 MB more to download. The crates runs for `aarch64-apple-darwin` (https://github.com/vladkens/cargo-solo/actions/runs/45930855352) and `x86_64-apple-darwin` (https://github.com/vladkens/cargo-solo/actions/runs/45934385766), as before Security's headers: all 20 built on Linux and ran on macOS, curl, rdkafka and git2 among them; on Windows all but jemalloc and lmdb, which fail there for every target, as before.
