<div align="center">

# cargo-solo

**Cross-compile Rust with clang: no Zig, no Docker**

[<img src="https://badges.ws/crates/v/cargo-solo" alt="version" />](https://crates.io/crates/cargo-solo)
[<img src="https://badges.ws/github/license/vladkens/cargo-solo" alt="license" />](https://github.com/vladkens/cargo-solo/blob/main/LICENSE)
[<img src="https://badges.ws/badge/follow-@uid127-black" alt="follow @uid127" />](https://x.com/uid127)

</div>

`cargo solo build --target <target>` builds your Rust project, C and C++ dependencies included, for Linux, Windows and macOS from any of them. It runs clang with the target's libc, which it fetches on first use: a few MB per target, pinned by SHA-256 and cached.

## 🤔 Why

`rustup target add` gives you Rust's std for another OS, but no C compiler, linker or libc for it, and most projects have C inside: `ring`, SQLite, zstd. So cross-compiling usually takes Zig (cargo-zigbuild), Docker (cross), or a CI runner per OS. cargo-solo takes clang, which, unlike gcc, compiles for every target from one install, and adds only the target's libc. [How it compares](#-compared).

## 🌟 Features

- 🎯 **Linux (glibc, musl), Windows and macOS**, from any of them
- 🧱 **C and C++ dependencies**: `ring`, `aws-lc-rs`, SQLite, crates built with cmake
- 🐧 **glibc 2.28**: the binary runs on Debian 10, RHEL 8 and newer
- 🪶 **3 to 23 MB per target**, fetched once: no Zig, no Docker image
- 🔐 **Nothing of ours hosted**: the distros' and Zig's own files, pinned by SHA-256
- 🧰 **Your flags stay yours**: `RUSTFLAGS` and `CFLAGS` work as without it

## ⚡ Quick start

```sh
cargo install --locked cargo-solo
cargo solo build --release --target x86_64-unknown-linux-gnu --target x86_64-pc-windows-gnu
```

It needs rustup and clang, 18 or newer for macOS targets. clang doesn't come with Rust, so install it once:

- macOS: `xcode-select --install`
- Linux: your distro's `clang` package
- Windows: `winget install LLVM.LLVM`

Crates built with cmake also need cmake, and on Windows ninja: `winget install Kitware.CMake Ninja-build.Ninja`. The Rust target and `llvm-tools` cargo-solo adds through rustup itself.

Any cargo command works, with one or more `--target`s:

```sh
cargo solo clippy --target aarch64-unknown-linux-musl
cargo solo build --target aarch64-apple-darwin --target x86_64-unknown-linux-musl
```

Without `--target`, it sets up the targets of `build.target` in cargo's config, which cargo builds for then. A target cargo-solo doesn't set up goes to cargo as is, such as `aarch64-apple-darwin` on a Mac.

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

On a Linux host, `--target` its own `-linux-gnu` too, so the binary needs glibc 2.28, not the build machine's.

macOS targets, built on Linux or Windows, get Rust and C but no C++ and no Apple frameworks: a crate that links one, such as `security-framework`, fails. For those, set `SDKROOT` to Xcode's SDK from a Mac, or build on a Mac. The binary runs on macOS 11 on arm64, 10.12 on x86_64.

## ⚙️ How it works

cargo-solo doesn't change cargo or rustc: it runs cargo with a few environment variables set.

1. On a target's first use, it fetches the target's libc and headers into its cache.
2. It points `CC_<target>`, `CXX_<target>` and the linker to clang, set up for the target.
3. Build scripts (`cc`, cmake, bindgen) read these, so C and C++ dependencies build too.

It sets no `RUSTFLAGS` or `CFLAGS` and writes nothing outside its cache and `target/`. Every variable it sets, and where each file comes from: [docs/how-it-works.md](docs/how-it-works.md).

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

- **A runner per OS**: a job per target, and the binary needs the runner's glibc, 2.43 on `ubuntu-latest`
- **[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)**: Zig, with clang and libcs inside; its glibc is [stubs](https://github.com/rust-cross/cargo-zigbuild#specify-glibc-version)
- **[cross](https://github.com/cross-rs/cross)**: Docker and an image per target, x86_64 only, emulated on Apple silicon
- **A distro's cross GCC**: Linux only, the distro's glibc, the compiler and linker set by hand

| | **cargo-solo** | [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) | [cross](https://github.com/cross-rs/cross) |
|---|---|---|---|
| Needs | clang, rustup | Zig, rustup | Docker or Podman |
| glibc | 2.28, Debian 10's | any, Zig's stubs | 2.39, or 2.17 (CentOS) |
| musl, Windows gnu | ✅ | ✅ | ✅ |
| On Apple silicon | native | native | x86_64, emulated |
| Apple targets | ➖ no frameworks | ➖ no frameworks | ➖ your own image |
| C++, cmake | ✅ but for macOS | ✅ | ✅ |

Take another tool for glibc below 2.28 (cargo-zigbuild's `.2.17`, cross's CentOS images), C++ for macOS (cargo-zigbuild), a target's tests through QEMU (cross), and `-windows-msvc` ([cargo-xwin](https://github.com/rust-cross/cargo-xwin)).

### First build

On CI without a cache, every build is a first one: the tool fetches or compiles each target's libc. Seconds the first build takes and then the next one, for a crate with `ring` on GitHub's `ubuntu-latest` (x86_64, 4 CPUs):

| | cargo-solo | cargo-zigbuild | cross |
|---|---|---|---|
| Linux gnu, musl | 12, then 8 | 34, then 5 | 70, then 18 |
| musl ×2, Windows | 14, then 10 | 85, then 6 | 110, then 25 |
| First build fetches | 14 or 24 MB | nothing, compiles libc | 4.7 or 9.4 GB |
| Installed once | llvm-tools, 41 MB | Zig, 57 MB | Docker |

The targets are `x86_64-unknown-linux-gnu` and `-musl`, then `x86_64` and `aarch64-unknown-linux-musl` with `x86_64-pc-windows-gnu`. cargo-zigbuild's next build is the fastest: Zig keeps the libc it compiled in its own cache.

## 🗺️ Roadmap

- Other glibc versions, each a Debian release.
- More Debian `-dev` packages in a sysroot, such as ALSA's.
- `aarch64-pc-windows-gnullvm`, i686, armv7 and riscv64.

## 🤖 How it's made

cargo-solo is built with AI agents following [agents.md](agents.md): I choose what to build, review the changes and test them on real machines. Before agents, I wrote software by hand for years ([macmon](https://github.com/vladkens/macmon), [twscrape](https://github.com/vladkens/twscrape), [ecloop](https://github.com/vladkens/ecloop)).

## 🤝 Contributing

Pull requests are welcome, including ones made with agents: point yours at [agents.md](agents.md).

## 📝 License

Distributed under the [MIT License](LICENSE).
