<div align="center">

# cargo-solo

**Cross-compile Rust with the clang you already have: no Zig, no Docker**

[<img src="https://badges.ws/crates/v/cargo-solo" alt="version" />](https://crates.io/crates/cargo-solo)
[<img src="https://badges.ws/github/license/vladkens/cargo-solo" alt="license" />](https://github.com/vladkens/cargo-solo/blob/main/LICENSE)
[<img src="https://badges.ws/badge/follow-@uid127-black" alt="follow @uid127" />](https://x.com/uid127)

</div>

`cargo solo build --target <target>` builds your Rust project, C dependencies included, for Linux and Windows from a Mac, Linux or Windows, and for macOS from Linux or Windows. It uses the clang your system has and the lld rustup ships, and fetches each target's libc from Debian's own packages (macOS's from Zig's source) on first use, pinned by SHA-256 and cached.

## 🌟 Features

- 🎯 **Linux glibc and musl, x86_64 and aarch64, and Windows**, from any of macOS, Linux and Windows
- 🍏 **macOS, arm64 and x86_64, from Linux and Windows**: Rust and C, without Apple's frameworks
- 🧱 **C dependencies through `cc`**, such as `ring` and `aws-lc-rs`, rustls' crypto, with no setup
- 🐧 **glibc 2.28**: the binary runs on Debian 10, RHEL 8 and anything newer
- 🪶 **2 to 23 MB per target**, fetched once: no Zig, no Docker image, no toolchain of ours
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

| Target | libc | From | Download | On disk |
|---|---|---|---|---|
| `x86_64-unknown-linux-gnu` | glibc 2.28 | Debian 10 | 9.4 MB | 43 MB |
| `aarch64-unknown-linux-gnu` | glibc 2.28 | Debian 10 | 7.2 MB | 34 MB |
| `x86_64-unknown-linux-musl` | musl 1.2.5 | Debian 13 and 10 headers, Rust's own libc | 2.1 MB | 10 MB |
| `aarch64-unknown-linux-musl` | musl 1.2.5 | Debian 13 and 10 headers, Rust's own libc | 2.1 MB | 10 MB |
| `x86_64-pc-windows-gnu` | mingw-w64 12 | Debian 13, rustup's `rust-mingw` | 16 MB | 210 MB |
| `aarch64-apple-darwin`, `x86_64-apple-darwin` | libSystem, macOS 27 SDK | Zig 0.17.1's source, one sysroot for both | 23 MB | 8 MB |

On a Linux host, `--target` its own `-linux-gnu` too, so the binary needs glibc 2.28, not the build machine's.

macOS's targets are set up on Linux and Windows, and run on rustc's oldest macOS: 11 on arm64, 10.12 on x86_64. Their SDK is macOS's headers and the stub of its libSystem from Zig's source, which covers what Rust and C need, with no Apple frameworks: a crate that links one, such as `security-framework` (`-framework Security`), fails with `framework not found`. For those, set `SDKROOT` to Xcode's SDK from your Mac, which cargo-solo then uses instead, or build on a macOS runner. No C++ either (`-lc++`).

## ⚙️ How it works

For each `--target` it sets up, cargo-solo runs cargo with:

- `CC_<target>` and `CARGO_TARGET_<TARGET>_LINKER`: `<target>-clang`, a link to cargo-solo in its cache, which runs your clang with the target's `--target` and `--sysroot` (for macOS, `-isysroot`), and to link, rustup's rust-lld (`-fuse-ld=lld`, as `ld64.lld` for macOS);
- `AR_<target>`: rustup's llvm-ar;
- for Windows, `PATH` with a folder in front that has llvm-ar as `x86_64-w64-mingw32-dlltool` (`dlltool.exe` on Windows), which rustc runs for `raw-dylib` links, as windows-sys's are;
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

| | **cargo-solo** | [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) | [cross](https://github.com/cross-rs/cross) |
|---|---|---|---|
| Needs | clang, rustup | Zig | Docker or Podman |
| Linux gnu and musl | ✅ glibc 2.28 | ✅ any glibc | ✅ the image's glibc |
| Windows gnu | ✅ | ✅ | ✅ |
| macOS targets | ❌ cargo itself, on a Mac | ✅ | ➖ an image you build with the SDK |
| C++, cmake | ❌ not yet | ✅ | ✅ |
| Fetches | the target's libc, 2 to 16 MB | Zig, with every target's libc | an image per target |

For `-windows-msvc`, use [cargo-xwin](https://github.com/rust-cross/cargo-xwin).

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
