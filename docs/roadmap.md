# Roadmap

The maintainer's backlog, in order: each item is done as a plan in `docs/plans/` and one PR. Agents change it only when the person asks.

Done:

- [x] **Linux and Windows from any host.** `cargo solo <cargo command> --target <t>` for `x86_64` and `aarch64` `-unknown-linux-gnu` (glibc 2.28) and `-unknown-linux-musl`, and `x86_64-pc-windows-gnu`, on macOS, Linux and Windows; C through `cc`; no flags of its own. Plan: [20261116-linux-windows.md](plans/completed/20261116-linux-windows.md).
- [x] **Pins by one command.** `make pins` re-pins each package to the latest patch in its Debian release (a point release's `+deb13u2`) and `rust-mingw` to rustup's latest, from Debian's `Packages` indexes and rustup's manifest, and checks every file on every mirror, as `every_package_is_on_every_mirror` does. Run by hand when needed; no scheduled job. Then a Debian release is a line to add. Plan: [20261116-pins.md](plans/completed/20261116-pins.md).
- [x] **C++ and cmake.** `CXX_<t>` through the wrapper with GCC 14's libstdc++, static: AlmaLinux 8's `gcc-toolset-14` for glibc 2.28, Alpine 3.21's for musl, Debian 13's mingw-w64 g++ for Windows; and a cmake toolchain file per target (`CMAKE_TOOLCHAIN_FILE_<t>`), as cargo-zigbuild, cross, cargo-xwin and zijiren233/cargo-cross all have. aws-lc-sys and ghstats need neither: aws-lc-sys builds through cc unless asked for cmake. Plan: [20261116-cxx.md](plans/completed/20261116-cxx.md).
- [x] **Apple's targets from Linux and Windows.** `aarch64-apple-darwin` and `x86_64-apple-darwin` on Linux and Windows hosts: macOS's headers and libSystem's stub from Zig's source tarball (Zig never runs), the host's clang 18 or newer, rustup's `ld64.lld`. No frameworks: the user's `SDKROOT` or a macOS runner covers them. Plan: [20261116-apple.md](plans/completed/20261116-apple.md).
- [x] **Dogfooding.** vladkens/actions `rust-dist` on cargo-solo in place of cargo-zigbuild, wavo-app's lint as `cargo solo clippy`, ghstats's Dockerfile.
- [x] **Other people's projects, then 0.1.0.** Popular projects that release with cargo-zigbuild or cross, built by cargo-solo for the targets they ship, and run; then 0.1.0 on crates.io. Plan: [20261116-others.md](plans/completed/20261116-others.md).
- [x] **The target from cargo config.** With no `--target`, the targets of `build.target` in cargo's config or `CARGO_BUILD_TARGET`, as cargo-zigbuild, cargo-xwin and zijiren233/cargo-cross do.
- [x] **The user's `CC_<t>` wins.** `CC_<t>`, `AR_<t>` and the linker set only when the user hasn't, as cargo-zigbuild does.
- [x] **C++ for macOS targets.** `CXX_<t>` and cmake's C++ for `-apple-darwin` on Linux and Windows hosts, linked against macOS's own `libc++.1.dylib` as Xcode links it: libc++'s headers in the SDK, configured as Apple's, and stubs of the dylibs made from LLVM's lists of their exports. Plan: [20261124-mac-cxx.md](plans/completed/20261124-mac-cxx.md).

Next, in order:

1. [ ] **Prefetch for Docker.** `cargo solo fetch` fetches everything a target needs, the C++ library included, and works before the sources are copied in, so that a Dockerfile caches it in a layer of its own. A Docker section in the readme.
2. [ ] **As time allows**, each its own PR:
   - Linux system libraries from Debian named in `Cargo.toml`, such as `libasound2-dev` for wavo-app, as cross's `pre-build` installs them with apt. Zig can't do that.
   - Other glibc versions by cargo-zigbuild's suffix, `--target x86_64-unknown-linux-gnu.2.31`, cargo given the triple without it: the versions of Debian's releases (2.31 in 11, 2.36 in 12, 2.41 in 13). rust-dist already passes zigbuild `.2.28`. Below 2.28 on request: 2.17, for CentOS 7 and manylinux2014, from CentOS 7's RPMs on vault.centos.org, real files as Debian's are, not stubs from symbol lists as Zig's.
   - New targets: armv7, tested in Docker through QEMU; `aarch64-pc-windows-gnullvm` (rustup ships `rust-mingw` for it); riscv64.

Later, on request: `cargo solo test` and `run` through a runner (QEMU for Linux, Wine for Windows), maybe never, as CI runs a target's tests on its own OS; Windows on arm64 as a host, i686; a smaller mingw sysroot, only the import libraries a link uses; a mirror setting if Debian's archives are slow.

Not planned: Docker; a toolchain of our own, hosted anywhere, or a libc built from sources as Zig builds its; `-windows-msvc`, which cargo-xwin covers.
