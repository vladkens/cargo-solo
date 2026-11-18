<div align="center">

# cargo-solo

**Cross-compile Rust with the clang you already have: no Zig, no Docker**

[<img src="https://badges.ws/crates/v/cargo-solo" alt="version" />](https://crates.io/crates/cargo-solo)
[<img src="https://badges.ws/github/license/vladkens/cargo-solo" alt="license" />](https://github.com/vladkens/cargo-solo/blob/main/LICENSE)
[<img src="https://badges.ws/badge/follow-@uid127-black" alt="follow @uid127" />](https://x.com/uid127)

</div>

`cargo solo build --target <target>` builds your Rust project, C dependencies included, for Linux and Windows from a Mac, Linux or Windows, and for macOS from Linux or Windows. It uses the clang your system has and the lld rustup ships, and fetches each target's libc from Debian's own packages (macOS's from Zig's source) on first use, pinned by SHA-256 and cached.

## 🤔 Why

`rustup target add` gives you Rust's std for another OS, but no C compiler, linker or libc for it, and nearly every project needs them: rustls' crypto, bundled SQLite and zstd are C. So cross-compiling takes Zig (cargo-zigbuild), Docker (cross), or a runner per OS, whose binary needs that runner's new glibc. cargo-solo needs none of them: your clang, rustup's lld and llvm-ar, and a few MB of Debian's own packages per target. [How it compares](#-compared).

## 🌟 Features

- 🎯 **Linux glibc and musl, x86_64 and aarch64, and Windows**, from any of macOS, Linux and Windows
- 🍏 **macOS, arm64 and x86_64, from Linux and Windows**: Rust and C, without Apple's frameworks
- 🧱 **C dependencies through `cc`**, such as `ring` and `aws-lc-rs`, rustls' crypto, with no setup
- 🐧 **glibc 2.28**: the binary runs on Debian 10, RHEL 8 and anything newer
- 🪶 **3 to 23 MB per target**, fetched once: no Zig, no Docker image, no toolchain of ours
- 🔐 **Nothing of ours hosted**: every file is Debian's, rustup's or Zig's, pinned by SHA-256
- 🧰 **Your flags stay yours**: `RUSTFLAGS` and `CFLAGS` work as without it

## ⚡ Quick start

```sh
cargo install --locked cargo-solo
cargo solo build --release --target x86_64-unknown-linux-gnu --target x86_64-pc-windows-gnu
```

It needs clang (on a Mac, `xcode-select --install`; on Linux, the distro's `clang`; on Windows, `winget install LLVM.LLVM`), 18 or newer for macOS, and rustup. cargo-solo adds the target and `llvm-tools` through rustup when they're missing.

Any cargo command works, and several `--target`s in one run, as cargo takes them:

```sh
cargo solo clippy --target aarch64-unknown-linux-musl
cargo solo build --target aarch64-apple-darwin --target x86_64-unknown-linux-musl
```

A target cargo-solo doesn't set up goes to cargo as is: on a Mac, `aarch64-apple-darwin` above, built with Xcode's SDK.

## 🎯 Targets

| Target | libc | Download | On disk |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | glibc 2.28 | 9.4 MB | 43 MB |
| `aarch64-unknown-linux-gnu` | glibc 2.28 | 7.2 MB | 34 MB |
| `x86_64-unknown-linux-musl` | musl 1.2.5 | 4.4 MB | 13 MB |
| `aarch64-unknown-linux-musl` | musl 1.2.5 | 2.9 MB | 10 MB |
| `x86_64-pc-windows-gnu` | mingw-w64 12 | 16 MB | 210 MB |
| `aarch64-apple-darwin` | libSystem | 23 MB | 8 MB |
| `x86_64-apple-darwin` | libSystem | the same | the same |

glibc is Debian 10's; musl Debian 13's, with 10's kernel headers and libgcc; mingw-w64 Debian 13's, with rustup's `rust-mingw`; macOS's SDK, 27's, Zig 0.17.1's source, one for both. On a Linux host, `--target` its own `-linux-gnu` too, so the binary needs glibc 2.28, not the build machine's.

macOS's targets are set up on Linux and Windows, and run on rustc's oldest macOS: 11 on arm64, 10.12 on x86_64. Their SDK is macOS's headers and the stub of its libSystem from Zig's source, which covers what Rust and C need, with no Apple frameworks: a crate that links one, such as `security-framework` (`-framework Security`), fails with `framework not found`. For those, set `SDKROOT` to Xcode's SDK from your Mac, which cargo-solo then uses instead, or build on a macOS runner. No C++ either (`-lc++`).

## ⚙️ How it works

For each `--target` it sets up, cargo-solo runs cargo with:

- `CC_<target>` and `CARGO_TARGET_<TARGET>_LINKER`: `<target>-clang`, a link to cargo-solo in its cache, which runs your clang with the target's `--target` and `--sysroot` (for macOS, `-isysroot`), and to link, rustup's rust-lld (`-fuse-ld=lld`, as `ld64.lld` for macOS);
- `AR_<target>`: rustup's llvm-ar;
- `PATH` with a folder in front that has rustup's llvm-ar and llvm-nm as the target's `ar`, `ranlib` and `nm`, by the names a configure script looks for with `--host`, such as `x86_64-unknown-linux-musl-ar` and `x86_64-linux-musl-ar`: the host's own, such as macOS's ar, can't read the target's objects; for Windows also llvm-ar as `x86_64-w64-mingw32-dlltool` (`dlltool.exe` on Windows), which rustc runs for `raw-dylib` links, as windows-sys's are;
- for Windows, `AWS_LC_SYS_PREBUILT_NASM_<target>=1`, unless you set it in the environment or cargo's config, so that aws-lc-sys, rustls' crypto, takes the NASM objects it ships and needs no nasm;
- for macOS, `SDKROOT`, the SDK in the cache, and `MACOSX_DEPLOYMENT_TARGET`, rustc's oldest macOS (10.12, which arm64 raises to 11), unless you set them.

So it sets no `RUSTFLAGS` or `CFLAGS`, which would replace yours. The sysroot is the target's libc, headers and libgcc, unpacked from Debian's `.deb` files (for macOS, from Zig's source tarball, with libSystem's aliases such as `libc.tbd` and a stub of libiconv added) into the cache: `~/Library/Caches/cargo-solo` on macOS, `~/.cache/cargo-solo` on Linux (`$XDG_CACHE_HOME`), `%LOCALAPPDATA%\cargo-solo` on Windows, or `CARGO_SOLO_CACHE`. Each download is tried from the archive and then from snapshot.debian.org (Zig's from ziglang.org, then a community mirror), and taken only with its pinned SHA-256. cargo-solo writes nothing outside the cache and cargo's target folder.

## 🚀 GitHub Actions

```yaml
jobs:
  build:
    runs-on: ubuntu-latest # or macos-latest, windows-latest
    steps:
      - uses: actions/checkout@v7
      - uses: actions/cache@v6
        with:
          path: ~/.cache/cargo-solo # ~/Library/Caches/cargo-solo, ~\AppData\Local\cargo-solo
          key: cargo-solo-${{ runner.os }}-0.1.0
      - run: cargo install --locked cargo-solo
      - run: cargo solo build --release --target x86_64-unknown-linux-gnu --target aarch64-unknown-linux-musl --target x86_64-pc-windows-gnu
```

## 🆚 Compared

The usual ways to cross-compile, and what each costs:

- **A runner per OS**: a job per target, and the binary needs the runner's glibc. `ubuntu-latest` has 2.43, and a binary built on 2.34 or newer doesn't start on RHEL 8 or Debian 11.
- **[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)**: Zig, a second compiler with its own libc, a 55 to 100 MB download. Its glibc is stubs, which [don't always behave as the real one](https://github.com/rust-cross/cargo-zigbuild#specify-glibc-version).
- **[cross](https://github.com/cross-rs/cross)**: Docker or Podman and an image per target. On a Mac that's a Linux VM, and the images are x86_64 only, emulated on Apple silicon. Their glibc is Ubuntu 24.04's, 2.39.
- **A distro's cross GCC**, such as `gcc-aarch64-linux-gnu`: Linux only, the distro's glibc, and `CC_<target>` and the linker set by hand.

| | **cargo-solo** | [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) | [cross](https://github.com/cross-rs/cross) |
|---|---|---|---|
| Needs | clang, rustup | Zig | Docker or Podman |
| glibc | 2.28, Debian 10's | any, Zig's stubs | 2.39, or 2.17 (CentOS) |
| musl, Windows gnu | ✅ | ✅ | ✅ |
| On Apple silicon | native | native | x86_64, emulated |
| Apple targets | ➖ no frameworks | ➖ no frameworks | ➖ your own image |
| C++, cmake | ❌ not yet | ✅ | ✅ |

The first build of a crate with `ring` on GitHub's `ubuntu-latest` (x86_64, 4 CPUs): what each tool fetches or builds, and how much longer it takes than the next build. rustup's std for each target, which every tool needs, is left out. The web server is for Debian and Alpine (`x86_64-unknown-linux-gnu`, `-musl`), the CLI for Linux x86_64 and arm64 and Windows (both `-unknown-linux-musl`, `x86_64-pc-windows-gnu`).

| First build | cargo-solo | cargo-zigbuild | cross |
|---|---|---|---|
| Once per machine | llvm-tools, 41 MB | Zig, 57 MB, 412 MB unpacked | Docker |
| Web server | 14 MB, 57 MB unpacked, +4 s | 69 MB of libc built, +29 s | 4.7 GB of images, +53 s |
| CLI | 24 MB, 234 MB unpacked, +4 s | 105 MB of libc built, +78 s | 9.4 GB of images, +86 s |

Take another tool for glibc below 2.28 (cargo-zigbuild's `.2.17`, cross's CentOS images), C++ and cmake until cargo-solo has them, a target's tests through QEMU (cross), and `-windows-msvc` ([cargo-xwin](https://github.com/rust-cross/cargo-xwin)).

## 🗺️ Roadmap

- Crates built through cmake and C++ (`libstdc++`).
- Other glibc versions, each a Debian release.
- More Debian `-dev` packages in a sysroot, such as ALSA's.
- `aarch64-pc-windows-gnullvm`, i686, armv7 and riscv64.

## 🤖 How it's made

cargo-solo is built with AI agents following [agents.md](agents.md): I choose what to build, review the changes and test them on real machines. Before agents, I wrote software by hand for years ([macmon](https://github.com/vladkens/macmon), [twscrape](https://github.com/vladkens/twscrape), [ecloop](https://github.com/vladkens/ecloop)).

## 🤝 Contributing

Pull requests are welcome, including ones made with agents: point yours at [agents.md](agents.md).

## 📝 License

Distributed under the [MIT License](LICENSE).
