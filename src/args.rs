// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! The command line: `cargo solo <cargo command> [args]`, the `--target`s in it, and the help.

use std::ffi::OsString;

use crate::toolchain::TARGETS;

/// What `cargo solo` was asked.
#[derive(Debug, PartialEq)]
pub enum Cmd {
  /// `-h`, `--help` or nothing.
  Help,
  /// `-V` or `--version`.
  Version,
  /// A cargo command with its args as given, and the `--target`s in them, in order.
  Cargo { args: Vec<OsString>, targets: Vec<String> },
}

/// The command in `args`, the program's name first: cargo runs a subcommand as `cargo-solo solo …`,
/// and `cargo-solo …` run by hand works the same.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Cmd {
  let mut args: Vec<OsString> = args.into_iter().skip(1).collect();
  if args.first().is_some_and(|x| x == "solo") {
    args.remove(0);
  }

  match args.first().map(|x| x.to_str()) {
    None | Some(Some("-h" | "--help")) => return Cmd::Help,
    Some(Some("-V" | "--version")) => return Cmd::Version,
    _ => {}
  }

  let targets = targets(&args);
  Cmd::Cargo { args, targets }
}

/// The values of `--target x` and `--target=x` in `args`, up to a `--`, after which the args are
/// the program's.
fn targets(args: &[OsString]) -> Vec<String> {
  let mut out = vec![];
  let mut it = args.iter().map(|x| x.to_str());
  while let Some(arg) = it.next() {
    match arg {
      Some("--") => break,
      Some("--target") => out.extend(it.next().flatten().map(String::from)),
      Some(x) => out.extend(x.strip_prefix("--target=").map(String::from)),
      None => {}
    }
  }

  out
}

/// The help, with the cache folder `cache`.
pub fn help(cache: &str) -> String {
  let ours = || TARGETS.iter().filter(|x| x.here());
  let width = ours().map(|x| x.rust.len()).max().unwrap_or(0);
  let targets: String =
    ours().map(|x| format!("  {:width$}  {}\n", x.rust, x.libc.version())).collect();

  format!(
    "Cross-compiles Rust with the host's clang and rustup's lld: no Zig, no Docker.

Usage: cargo solo <cargo command> [args]...

  cargo solo build --release --target x86_64-unknown-linux-gnu --target x86_64-pc-windows-gnu
  cargo solo clippy --target aarch64-unknown-linux-musl

Each of these --targets gets its C compiler, linker and libc, and, but for macOS, its C++ compiler
and cmake toolchain file; any other goes to cargo as is:
{targets}
Needs clang, and rustup to add a target or llvm-tools when missing; cmake for crates built with it,
and on Windows ninja. Each target's libc comes from Debian's packages (Apple's SDK from Zig's
source) on first use, its libstdc++ on the first C++ compile, cached in {cache} (CARGO_SOLO_CACHE).
"
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  fn cmd(line: &str) -> Cmd {
    parse(line.split_whitespace().map(OsString::from))
  }

  fn cargo(args: &str, targets: &[&str]) -> Cmd {
    let args = args.split_whitespace().map(OsString::from).collect();
    Cmd::Cargo { args, targets: targets.iter().map(|x| x.to_string()).collect() }
  }

  #[test]
  fn parse_takes_the_cargo_command() {
    let t = "x86_64-unknown-linux-gnu";
    assert_eq!(cmd("cargo-solo solo build --release"), cargo("build --release", &[]));
    assert_eq!(cmd("cargo-solo build --target x"), cargo("build --target x", &["x"]));
    assert_eq!(
      cmd(&format!("cargo-solo solo build --target {t} --target=aarch64-apple-darwin")),
      cargo(
        &format!("build --target {t} --target=aarch64-apple-darwin"),
        &[t, "aarch64-apple-darwin"]
      )
    );
    // After `--` the args are the program's, `cargo solo run` passes them on.
    let run = "run --target=a -- --target b";
    assert_eq!(cmd(&format!("cargo-solo solo {run}")), cargo(run, &["a"]));
    assert_eq!(cmd("cargo-solo solo build --target"), cargo("build --target", &[]));
  }

  #[test]
  fn parse_keeps_help_and_version_of_cargo_commands() {
    assert_eq!(cmd("cargo-solo"), Cmd::Help);
    assert_eq!(cmd("cargo-solo solo"), Cmd::Help);
    assert_eq!(cmd("cargo-solo solo --help"), Cmd::Help);
    assert_eq!(cmd("cargo-solo solo -h"), Cmd::Help);
    assert_eq!(cmd("cargo-solo solo -V"), Cmd::Version);
    assert_eq!(cmd("cargo-solo solo --version"), Cmd::Version);
    assert_eq!(cmd("cargo-solo solo build --help"), cargo("build --help", &[]));
  }

  #[test]
  fn help_lists_every_target_of_this_host() {
    let text = help("/tmp/cache");
    for target in &TARGETS {
      assert_eq!(text.contains(target.rust), target.here(), "{}", target.rust);
    }

    assert!(text.contains("/tmp/cache"));
  }
}
