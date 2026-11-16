# Roadmap

The maintainer's backlog, in order: each item is done as a plan in `docs/plans/` and one PR. Agents change it only when the person asks.

- [x] **Linux and Windows from any host.** `cargo solo <cargo command> --target <t>` for `x86_64` and `aarch64` `-unknown-linux-gnu` (glibc 2.28) and `-unknown-linux-musl`, and `x86_64-pc-windows-gnu`, on macOS, Linux and Windows; C through `cc`; no flags of its own. Plan: [20261116-linux-windows.md](plans/completed/20261116-linux-windows.md).
- [ ] **Pins by one command.** `make pins` re-pins each package to the latest patch in its Debian release (a point release's `+deb13u2`) and `rust-mingw` to rustup's latest, from Debian's `Packages` indexes and rustup's manifest, and checks every file on every mirror, as `every_package_is_on_every_mirror` does. Run by hand when needed; no scheduled job. Then a Debian release is a line to add.
- [ ] **Dogfooding and 0.1.0.** vladkens/actions `rust-dist` on cargo-solo in place of cargo-zigbuild, wavo-app's lint as `cargo solo clippy`, ghstats's Dockerfile. Then, with the person's approval, the repo public and 0.1.0 on crates.io.
- [ ] **C++ and cmake.** `CXX_<t>` through the wrapper with Debian's libstdc++, and a cmake toolchain file per target (`CMAKE_TOOLCHAIN_FILE_<t>`), as cargo-zigbuild, cross, cargo-xwin and zijiren233/cargo-cross all have. For aws-lc-sys through cmake, and ghstats.
- [ ] **The target from cargo config.** With no `--target`, set up the one in `build.target` of `.cargo/config.toml` or `CARGO_BUILD_TARGET`, as cargo-zigbuild, cargo-xwin and zijiren233/cargo-cross do; such a build now runs with the host's C compiler.
- [ ] **The user's `CC_<t>` wins.** Set `CC_<t>`, `AR_<t>` and the linker only when the user hasn't, as cargo-zigbuild does.

Each of these changes the CLI:

- [ ] **Other glibc versions** by cargo-zigbuild's suffix, `--target x86_64-unknown-linux-gnu.2.31`, cargo given the triple without it: the versions of Debian's releases (2.31 in 11, 2.36 in 12, 2.41 in 13). rust-dist already passes zigbuild `.2.28`.
- [ ] **Extra Debian packages in a sysroot,** such as `libasound2-dev` for wavo-app, named in `[package.metadata.solo]`, as cross's `pre-build` installs them with apt. Zig can't do that.
- [ ] **`cargo solo test` and `run` through a runner:** QEMU for Linux, Wine for Windows, as cross and zijiren233/cargo-cross do. Maybe never: CI runs a target's tests on its own OS.

Later, on request: `aarch64-pc-windows-gnullvm` (rustup ships `rust-mingw` for it), Windows on arm64 as a host, i686, armv7 and riscv64; a smaller mingw sysroot, only the import libraries a link uses; a mirror setting if Debian's archives are slow.

Not planned: Docker; a toolchain of our own, hosted anywhere; Apple's targets (cargo itself on a Mac; Apple's SDK can't be shipped); `-windows-msvc`, which cargo-xwin covers.
