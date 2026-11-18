## v0.1.0 – 2026-11-18

### Features

- `cargo solo <command> --target …` runs any cargo command with each target's C compiler, linker and libc set up; without `--target`, those of cargo's `build.target`
- Linux x86_64 and aarch64 with glibc 2.28 or musl 1.2.5, and Windows x86_64 (mingw-w64), from macOS, Linux or Windows
- macOS arm64 and x86_64 from Linux or Windows, with Rust and C but no C++ or Apple frameworks
- C and C++ dependencies through `cc` and cmake, with bindgen and pkg-config set up per target
- Each target's libc and headers fetched on first use from Debian's packages (macOS's from Zig's source), pinned by SHA-256 and cached
- `RUSTFLAGS` and `CFLAGS` stay yours: cargo-solo's flags go through its own compiler and linker wrapper

**Full Changelog**: https://github.com/vladkens/cargo-solo/commits/v0.1.0
