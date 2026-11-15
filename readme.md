# cargo-solo

**Cross-compile Rust for Linux and Windows from macOS, Linux or Windows: no Zig, no Docker.**

```sh
cargo solo build --release --target x86_64-unknown-linux-gnu --target x86_64-pc-windows-gnu
```

cargo-solo uses the clang your system has and the lld rustup ships, and fetches each target's libc from Debian's packages on first use, pinned by SHA-256 and cached. Work in progress.

## License

MIT
