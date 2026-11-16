# Linux and Windows from any host

Done: 2026-11-16, PR #1

`cargo solo <cargo command> --target <t>` builds a Rust project, C dependencies through `cc` included, for `x86_64` and `aarch64` `-unknown-linux-gnu` and `-unknown-linux-musl`, and `x86_64-pc-windows-gnu`, on macOS, Linux and Windows. The basic case only: no config and no flags of its own.

## Why

vladkens/actions `rust-dist` built Linux and Windows with zijiren233/cargo-cross, whose prebuilt GCC toolchains return 404 since 2026-11-06; it moved to cargo-zigbuild as the stopgap (actions PR #14). The person doesn't want Zig in the chain; cross needs Docker.

## Decisions

- `cargo solo <command> [args]`, as cargo-xwin runs: each `--target` in the args that cargo-solo knows is set up (several allowed, as cargo takes them), and the command goes to cargo as is. Other targets, Apple's included, go to cargo untouched.
- The host's clang compiles C and drives the link: Apple's on macOS, `clang` on PATH on Linux, LLVM's on Windows. rustup's rust-lld links (`-fuse-ld=lld -B<rl>/gcc-ld`), its llvm-ar archives. A missing target or `llvm-tools` is added through rustup.
- No `RUSTFLAGS` or `CFLAGS`, which would replace the user's: `CC_<t>` and `CARGO_TARGET_<T>_LINKER` point at `<cache>/bin/<t>-clang`, a link to cargo-solo that runs clang with the caller's args, then the target's `--target`, `--sysroot` and, to link, lld's. `AR_<t>` is llvm-ar.
- Sysroots from Debian's packages, pinned by SHA-256 in the code: glibc 2.28 from Debian 10 (archive.debian.org, the floor rust-dist uses), musl 1.2.5's headers from Debian 13 (Rust ships musl's `libc.a`) and the kernel's from Debian 10, mingw-w64 12 from Debian 13 with libgcc from rustup's `rust-mingw`. Each download falls back to snapshot.debian.org.
- The cache: `~/Library/Caches/cargo-solo`, `~/.cache/cargo-solo` (`$XDG_CACHE_HOME`), `%LOCALAPPDATA%\cargo-solo`, or `CARGO_SOLO_CACHE`. A folder per sysroot, unpacked aside and renamed into place when complete; symlinks become hard links or copies.
- Dependencies, approved by the person: `tar`, `lzma-rs` (a `.deb`'s `data.tar.xz`), `hmac-sha256`. Downloads through the system's `curl`, as shipo does; the arguments parsed by hand.

## Tasks

- [x] 1. The command: the `--target`s in the args (`--target x`, `--target=x`), cargo run in place of cargo-solo, its exit code passed on; the target table.
- [x] 2. Linux sysroots: downloads over the mirrors checked by SHA-256, the `.deb`'s ar and `data.tar.xz`, only headers and libraries kept, symlinks resolved inside the sysroot, the cache.
- [x] 3. The wrapper: the per-target clang link as the C compiler and the linker, llvm-ar, rustup's target and `llvm-tools` when missing, a clear error without clang or rustup.
- [x] 4. windows-gnu: mingw-w64 from Debian, libgcc from `rust-mingw`.
- [x] 5. The end-to-end test: `tests/hello` (`ring`, C through `cc`, a thread) built for every target and run in its OS (Debian 10 and Alpine in Docker, Windows natively), the glibc floor checked; `make e2e`.
- [x] 6. CI on `ubuntu-latest`, `macos-latest` and `windows-latest`: checks and tests, the e2e build on each, its binaries run on Linux and Windows.
- [x] 7. `readme.md` and `--help`.

## Result

All five targets build on `macos-latest`, `ubuntu-latest` and `windows-latest`, and every host's binaries run: the four Linux ones in Debian 10 and Alpine (arm64 through QEMU), the `.exe` on Windows. The `-linux-gnu` binaries need glibc 2.28 at most, the musl ones are static, the `.exe` imports only Windows' DLLs. A sysroot is 2 to 16 MB to download. Every package is on each of its mirrors.
