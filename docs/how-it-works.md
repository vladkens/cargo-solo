# How cargo-solo works

The details behind the readme: what cargo-solo sets for cargo, and where each file comes from.

## What it sets

For each `--target` it sets up, cargo-solo runs cargo with:

- `CC_<target>` and `CARGO_TARGET_<TARGET>_LINKER`: `<target>-clang`, a link to cargo-solo in its cache, which runs your clang with the target's `--target` and `--sysroot` (for macOS, `-isysroot`), and to link, rustup's rust-lld (`-fuse-ld=lld`, as `ld64.lld` for macOS);
- `AR_<target>`: rustup's llvm-ar, and `RANLIB_<target>`, it as ranlib from the folder below: OpenSSL's build otherwise runs the host's `ranlib`;
- for Linux and Windows, `CXX_<target>`: `<target>-clang++`, the same link, which runs clang as g++ with libstdc++'s headers, and to link, libstdc++; unless you set `CXX_<target>` or `TARGET_CXX` (`HOST_CXX` for the host's own target, as cc reads it);
- `CMAKE_TOOLCHAIN_FILE_<target>`: a toolchain file in the cache for cmake-rs, with the target's system, its sysroot (for macOS, the SDK in `SDKROOT`), the two compilers (for C++, the one cc takes: yours when you set `CXX_<target>`, with a wrapper such as sccache as cmake's launcher; for macOS, the C one, which compiles the C++ of cmake's check), llvm-ar (also as ranlib) and llvm-nm (for macOS also llvm-objcopy as install_name_tool, without which cmake stops), and to find libraries in, besides the sysroot, cargo's build folder, where another crate's build script puts one; unless you set it or `CMAKE_TOOLCHAIN_FILE`;
- on a Windows host, `CC_<target>`, `CXX_<target>`, `AR_<target>`, `RANLIB_<target>` and the linker with `/` instead of `\`, which Windows also takes: sh, which runs a Makefile's commands, as openssl-src's under MSYS2, drops each `\`;
- on a Windows host with ninja on `PATH`, `CMAKE_GENERATOR_<target>=Ninja`, unless you chose a generator: cmake-rs would take Visual Studio, or MinGW's make for Windows;
- `PATH` with a folder in front that has rustup's llvm-ar and llvm-nm as the target's `ar`, `ranlib` and `nm`, by the names a configure script looks for with `--host`, such as `x86_64-unknown-linux-musl-ar` and `x86_64-linux-musl-ar`: the host's own, such as macOS's ar, can't read the target's objects; for Windows also llvm-ar as `x86_64-w64-mingw32-dlltool` (`dlltool.exe` on Windows), which rustc runs for `raw-dylib` links, as windows-sys's are;
- `PKG_CONFIG_LIBDIR_<target>`: the sysroot's `pkgconfig` folders, as cargo lets pkg-config run for the host's own target, where it would find the host's libraries; and `BINDGEN_EXTRA_CLANG_ARGS_<target>`: the wrapper's `--target` and `--sysroot`, as bindgen's clang otherwise reads the host's headers; each unless you set it, pkg-config's also as `TARGET_PKG_CONFIG_LIBDIR`;
- for Windows, `AWS_LC_SYS_PREBUILT_NASM_<target>=1`, unless you set it in the environment or cargo's config, so that aws-lc-sys, rustls' crypto, takes the NASM objects it ships and needs no nasm;
- for macOS, `SDKROOT`, the SDK in the cache, and `MACOSX_DEPLOYMENT_TARGET`, rustc's oldest macOS (10.12, which arm64 raises to 11), unless you set them.

So it sets no `RUSTFLAGS` or `CFLAGS`, which would replace yours.

## Where the files come from

The sysroot is the target's libc, headers and libgcc, unpacked from Debian's `.deb` files (for macOS, from Zig's source tarball, with libSystem's aliases such as `libc.tbd`, a stub of libiconv and `sys/syscall.h` without its numbers added). glibc is Debian 10's; musl Debian 13's, with 10's kernel headers and libgcc; mingw-w64 Debian 13's, with rustup's `rust-mingw`; macOS's SDK, 27's, Zig 0.17.1's source, one for both macOS targets.

Each download is tried from the archive and then from snapshot.debian.org (Zig's from ziglang.org, then a community mirror), and taken only with its pinned SHA-256. The cache is `~/Library/Caches/cargo-solo` on macOS, `~/.cache/cargo-solo` on Linux (`$XDG_CACHE_HOME`), `%LOCALAPPDATA%\cargo-solo` on Windows, or `CARGO_SOLO_CACHE`. cargo-solo writes nothing outside the cache and cargo's target folder.

libstdc++ is GCC 14's, static, so the binary needs no libstdc++ where it runs: for glibc, AlmaLinux 8's `gcc-toolset-14`, built for glibc 2.28; for musl, Alpine 3.21's; for Windows, Debian 13's mingw-w64 g++. Each is tried from the distro's server and then from kernel.org's mirror (Debian's as above). It is fetched into its own folder on the first C++ compile, so a build with no C++ fetches nothing more: 4 MB to download and 22 MB on disk for x86_64 Linux, 6.4 MB and 25 MB for aarch64 Linux, 15 MB and 21 MB for Windows. Then rustc's link gets it too, when a crate names `stdc++` for rustc, as cc does for C++. For aarch64, GCC 14's `libgcc.a` comes with it, for the outline atomics libstdc++ calls. cmake can't compile Windows resources (`.rc`): there's no windres.

## macOS

macOS's targets are set up on Linux and Windows, and run on rustc's oldest macOS: 11 on arm64, 10.12 on x86_64. Their SDK is macOS's headers and the stub of its libSystem from Zig's source, which covers what Rust and C need, with no Apple frameworks: a crate that links one, such as `security-framework` (`-framework Security`), fails with `framework not found`. For those, set `SDKROOT` to Xcode's SDK from your Mac, which cargo-solo then uses instead. There's no C++ library either (`-lc++`), so cmake builds C only.
