// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! Each target's toolchain: the targets cargo-solo sets up, the host's clang and rustup's lld and
//! llvm-ar they share, the environment cargo runs with, and the wrapper: this program run as a
//! target's C compiler and linker, which adds the target's flags to clang's.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};

use crate::packages::{self, Libc, Package};
use crate::{bad, run};

/// The wrapper's clang, set by cargo-solo.
const CLANG: &str = "CARGO_SOLO_CLANG";
/// The folder of rustup's `ld.lld`, which runs rust-lld as GNU ld, for clang's `-B`; set by
/// cargo-solo.
const GCC_LD: &str = "CARGO_SOLO_GCC_LD";
/// aws-lc-sys, rustls' crypto, assembles x86_64 Windows' code with nasm, which a host seldom has;
/// this lets it take the objects it ships instead, when there's no nasm.
const PREBUILT_NASM: &str = "AWS_LC_SYS_PREBUILT_NASM";
/// Apple's SDK, which build scripts read, and the wrapper: cargo-solo's, or the user's, with its
/// frameworks.
const SDKROOT: &str = "SDKROOT";
/// The oldest macOS an Apple binary runs on, which rustc and cc-rs read: without it, cc-rs off a
/// Mac passes 10.7 for x86_64.
const DEPLOYMENT_TARGET: &str = "MACOSX_DEPLOYMENT_TARGET";

// MARK: Targets

/// A target cargo-solo sets up.
#[derive(Debug)]
pub struct Target {
  pub rust: &'static str,
  /// clang's `--target`; for Linux and Windows, Debian's multiarch name for it too.
  pub clang: &'static str,
  pub libc: Libc,
  pub packages: &'static [Package],
}

pub const TARGETS: [Target; 7] = [
  Target {
    rust: "x86_64-unknown-linux-gnu",
    clang: "x86_64-linux-gnu",
    libc: Libc::Glibc,
    packages: packages::GLIBC_X64,
  },
  Target {
    rust: "aarch64-unknown-linux-gnu",
    clang: "aarch64-linux-gnu",
    libc: Libc::Glibc,
    packages: packages::GLIBC_ARM64,
  },
  Target {
    rust: "x86_64-unknown-linux-musl",
    clang: "x86_64-linux-musl",
    libc: Libc::Musl,
    packages: packages::MUSL_X64,
  },
  Target {
    rust: "aarch64-unknown-linux-musl",
    clang: "aarch64-linux-musl",
    libc: Libc::Musl,
    packages: packages::MUSL_ARM64,
  },
  Target {
    rust: "x86_64-pc-windows-gnu",
    clang: "x86_64-w64-mingw32",
    libc: Libc::Mingw,
    packages: packages::MINGW_X64,
  },
  Target {
    rust: "aarch64-apple-darwin",
    clang: "arm64-apple-macos",
    libc: Libc::Darwin,
    packages: packages::DARWIN,
  },
  Target {
    rust: "x86_64-apple-darwin",
    clang: "x86_64-apple-macos",
    libc: Libc::Darwin,
    packages: packages::DARWIN,
  },
];

impl Target {
  /// The target Rust calls `name`, when cargo-solo sets it up on this host.
  pub fn find(name: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|x| x.rust == name && x.here())
  }

  /// Whether cargo-solo sets it up on this host: on a Mac, Apple's targets are cargo's own, with
  /// Xcode's SDK.
  pub fn here(&self) -> bool {
    self.libc != Libc::Darwin || env::consts::OS != "macos"
  }

  /// Its sysroot's folder name in the cache: Apple's targets share one, as `SDKROOT` names one.
  pub fn sysroot(&self) -> &'static str {
    match self.libc {
      Libc::Darwin => "macos",
      _ => self.clang,
    }
  }

  /// The target whose wrapper `arg0` runs: `<target>-clang`.
  pub fn wrapped(arg0: &OsStr) -> Option<&'static Target> {
    let stem = Path::new(arg0).file_stem()?.to_str()?;
    Target::find(stem.strip_suffix("-clang")?)
  }

  /// The environment cargo runs with for this target: the wrapper at `cc` its C compiler and its
  /// linker, which rustc takes for a C compiler driving the link by its name, `*-clang`; llvm-ar at
  /// `ar` its archiver.
  pub fn env(&self, cc: &Path, ar: &Path) -> [(String, OsString); 3] {
    let upper = self.rust.to_uppercase().replace('-', "_");
    [
      (format!("CC_{}", self.rust), cc.into()),
      (format!("AR_{}", self.rust), ar.into()),
      (format!("CARGO_TARGET_{upper}_LINKER"), cc.into()),
    ]
  }

  /// The flags the wrapper puts after clang's `args`, so that they win over the `--target` cc-rs
  /// passes: the target and the sysroot at `dir`, and to link, rustup's lld from `gcc_ld`, which
  /// clang runs as `ld64.lld` for Apple's targets.
  pub fn flags(&self, dir: &Path, gcc_ld: &Path, args: &[OsString]) -> Vec<OsString> {
    let mut out = vec![format!("--target={}", self.clang).into()];
    match self.libc {
      Libc::Mingw => out.push(cat("--sysroot=", &dir.join("usr"))),
      // An SDK, as Xcode's clang takes it: for Apple, a -isysroot that cc-rs passes would win over
      // a --sysroot.
      Libc::Darwin => out.extend(["-isysroot".into(), dir.into()]),
      _ => out.push(cat("--sysroot=", dir)),
    }

    if self.libc == Libc::Musl {
      // ring passes -nostdlibinc for musl, which drops the sysroot's headers; the kernel's asm/ is
      // Debian's, under glibc's name.
      let include = dir.join("usr/include");
      out.push("-idirafter".into());
      out.push(include.join(self.clang).into());
      out.push("-idirafter".into());
      out.push(include.join(self.clang.replace("-musl", "-gnu")).into());
    }

    // An unused -fuse-ld when only compiling is a warning, an error under -Werror.
    let compiles = args.iter().any(|x| ["-c", "-E", "-S", "-M", "-MM"].iter().any(|f| x == f));
    if compiles {
      return out;
    }

    out.push("-fuse-ld=lld".into());
    out.push(cat("-B", gcc_ld));
    // rustc passes gcc's -no-pie with -static for aarch64 musl, which clang warns is unused, and
    // the warning shows in every build as the linker's.
    out.push("-Qunused-arguments".into());
    if self.libc == Libc::Musl {
      // For what clang links itself, as a configure script's test: the crt objects, libc and
      // libgcc are in musl's Debian folder, which clang doesn't search, and libgcc is static, as
      // with Debian's musl-gcc. Rust links with its own.
      let lib = dir.join("usr/lib").join(self.clang);
      out.push(cat("-B", &lib));
      out.push(cat("-L", &lib));
      out.push("-static-libgcc".into());
    }

    out
  }
}

fn cat(flag: &str, path: &Path) -> OsString {
  let mut out = OsString::from(flag);
  out.push(path);
  out
}

// MARK: Host

/// The tools every target shares: the host's clang, and rustup's lld and llvm-ar.
pub struct Host {
  clang: PathBuf,
  /// rustc's sysroot, each target's std in its `lib/rustlib/<target>`.
  sysroot: PathBuf,
  /// rustup's tools for the host, `lib/rustlib/<host>/bin` in rustc's sysroot.
  bin: PathBuf,
  cache: PathBuf,
}

impl Host {
  /// Finds the tools, adding rustup's `llvm-tools` when it's missing.
  pub fn find() -> io::Result<Self> {
    // As cargo picks it, but for `build.rustc` in its config files.
    let rustc = env::var_os("RUSTC").or_else(|| env::var_os("CARGO_BUILD_RUSTC"));
    let rustc = rustc.unwrap_or_else(|| "rustc".into());
    let text = |args: &[&str]| -> io::Result<String> {
      let out = run(Command::new(&rustc).args(args))?;
      Ok(String::from_utf8_lossy(&out).trim().to_string())
    };

    let sysroot = PathBuf::from(text(&["--print", "sysroot"])?);
    let info = text(&["-vV"])?;
    let host = info.lines().find_map(|x| x.strip_prefix("host: "));
    let host = host.ok_or_else(|| bad("`rustc -vV` names no host"))?;
    let bin = sysroot.join("lib/rustlib").join(host).join("bin");
    if !bin.join(exe("llvm-ar")).is_file() {
      rustup(&sysroot, &["component", "add", "llvm-tools"])?;
    }

    if !bin.join("gcc-ld").join(exe("ld.lld")).is_file() {
      return Err(bad(format!("no rust-lld in {}", bin.display())));
    }

    Ok(Host { clang: clang()?, sysroot, bin, cache: packages::cache()? })
  }

  /// The environment the wrapper reads, the same for every target.
  pub fn env(&self) -> [(&'static str, OsString); 3] {
    [
      (CLANG, self.clang.clone().into()),
      (GCC_LD, self.bin.join("gcc-ld").into()),
      ("CARGO_SOLO_CACHE", self.cache.clone().into()),
    ]
  }

  /// Sets `target` up: its std through rustup when it's missing, its sysroot, its wrapper; and
  /// gives the environment cargo runs with for it: for Windows, rustc's dlltool in front of PATH
  /// and aws-lc-sys's NASM objects, for Apple, the [SDK and the oldest macOS](Host::darwin), unless
  /// the user chose in cargo's `args`, config or environment.
  pub fn setup(&self, target: &Target, args: &[OsString]) -> io::Result<Vec<(String, OsString)>> {
    // First, as it checks the tools before any download.
    let darwin = match target.libc {
      Libc::Darwin => self.darwin(target, args)?,
      _ => vec![],
    };

    if !self.sysroot.join("lib/rustlib").join(target.rust).join("lib").is_dir() {
      rustup(&self.sysroot, &["target", "add", target.rust])?;
    }

    let sysroot = packages::ensure(&self.cache, target.sysroot(), target.packages)?;
    let cc = wrapper(&self.cache.join("bin"), target, &sysroot)?;
    let ar = self.bin.join(exe("llvm-ar"));
    let mut out = target.env(&cc, &ar).to_vec();
    out.extend(darwin);
    if target.libc != Libc::Mingw {
      return Ok(out);
    }

    let dir = dlltool(&self.cache.join("bin"), target, &ar)?;
    let path = env::var_os("PATH").unwrap_or_default();
    let path = env::join_paths([dir].into_iter().chain(env::split_paths(&path)));
    out.push(("PATH".into(), path.map_err(|e| bad(format!("PATH: {e}")))?));
    let nasm = format!("{PREBUILT_NASM}_{}", target.rust.replace('-', "_"));
    if !chosen(&[PREBUILT_NASM, &nasm], args, &env::current_dir()?) {
      out.push((nasm, "1".into()));
    }

    Ok(out)
  }

  /// For Apple's `target`: [SDKROOT], its sysroot, and [DEPLOYMENT_TARGET], the oldest of rustc's
  /// for Apple, each unless the user chose it. Neither has a name per target, so one value serves
  /// both: rustc and clang raise x86_64's 10.12 to arm64's minimum, 11.0.
  fn darwin(&self, target: &Target, args: &[OsString]) -> io::Result<Vec<(String, OsString)>> {
    let gcc_ld = self.bin.join("gcc-ld");
    if !gcc_ld.join(exe("ld64.lld")).is_file() {
      return Err(bad(format!("no ld64.lld in {}", gcc_ld.display())));
    }

    // The SDK's TargetConditionals.h fails on an older clang: "KernelKit requires
    // -fdefine_target_os_macros".
    let text = run(Command::new(&self.clang).arg("--version"))?;
    let text = String::from_utf8_lossy(&text);
    if major(&text).is_some_and(|x| x < 18) {
      let line = text.lines().next().unwrap_or_default();
      return Err(bad(format!("Apple's targets need clang 18 or newer, not {line}")));
    }

    let here = env::current_dir()?;
    let mut out = vec![];
    if !chosen(&[SDKROOT], args, &here) {
      let sdk = packages::dir(&self.cache, target.sysroot(), target.packages);
      out.push((SDKROOT.into(), sdk.into()));
    }

    if chosen(&[DEPLOYMENT_TARGET], args, &here) {
      return Ok(out);
    }

    let rustc = self.sysroot.join("bin").join(exe("rustc"));
    let mut vers = vec![];
    for apple in TARGETS.iter().filter(|x| x.libc == Libc::Darwin) {
      let line = ["--print", "deployment-target", "--target", apple.rust];
      let text = run(Command::new(&rustc).args(line))?;
      let text = String::from_utf8_lossy(&text);
      let ver = text.trim().split_once('=').map(|x| x.1.to_string());
      vers.push(ver.ok_or_else(|| bad(format!("rustc's deployment target: {text}")))?);
    }

    let key = |x: &String| x.split('.').map(|x| x.parse().unwrap_or(0)).collect::<Vec<u32>>();
    out.extend(vers.into_iter().min_by_key(key).map(|x| (DEPLOYMENT_TARGET.into(), x.into())));
    Ok(out)
  }
}

/// Whether the user set one of `vars` for cargo: in its environment or `args`, or outside comments
/// in a config file it reads: a `.toml` that `args` name, and `config.toml` in `.cargo`
/// of `here`, of each folder above and of CARGO_HOME. By the name alone, which is all a choice
/// needs.
fn chosen(vars: &[&str], args: &[OsString], here: &Path) -> bool {
  let named =
    |x: &str| x.split(|x: char| !x.is_alphanumeric() && x != '_').any(|x| vars.contains(&x));
  let args: Vec<_> = args.iter().map(|x| x.to_string_lossy()).collect();
  if vars.iter().any(|x| env::var_os(x).is_some()) || args.iter().any(|x| named(x)) {
    return true;
  }

  let given = args.iter().map(|x| here.join(x.strip_prefix("--config=").unwrap_or(x)));
  let given = given.filter(|x| x.extension().is_some_and(|x| x == "toml"));
  let home = env::var_os("CARGO_HOME").map(PathBuf::from);
  let home = home.or_else(|| env::home_dir().map(|x| x.join(".cargo")));
  let dirs = here.ancestors().map(|x| x.join(".cargo")).chain(home);
  let files = given.chain(dirs.flat_map(|x| [x.join("config.toml"), x.join("config")]));
  let mut texts = files.filter_map(|x| fs::read_to_string(x).ok());
  texts.any(|x| x.lines().any(|x| named(code(x))))
}

/// The major version in `clang --version`'s `text`.
fn major(text: &str) -> Option<u32> {
  text.split("clang version ").nth(1)?.split('.').next()?.parse().ok()
}

/// A TOML `line` without its comment, from the first `#` outside quotes.
fn code(line: &str) -> &str {
  let (mut quote, mut escaped) = (None, false);
  for (i, c) in line.char_indices() {
    match (quote, c) {
      _ if escaped => escaped = false,
      (Some('"'), '\\') => escaped = true,
      (None, '#') => return &line[..i],
      (None, '"' | '\'') => quote = Some(c),
      (Some(q), _) if c == q => quote = None,
      _ => {}
    }
  }

  line
}

/// clang from PATH, or on Windows from LLVM's folder, which its installer doesn't add to PATH
/// unasked.
fn clang() -> io::Result<PathBuf> {
  let llvm = env::var_os("ProgramFiles").filter(|_| cfg!(windows));
  let llvm = llvm.map(|x| PathBuf::from(x).join("LLVM/bin/clang.exe")).filter(|x| x.is_file());
  if let Some(path) = which("clang").or(llvm) {
    return Ok(path);
  }

  let hint = match env::consts::OS {
    "macos" => "install the Command Line Tools: `xcode-select --install`",
    "windows" => "install LLVM: `winget install LLVM.LLVM`",
    _ => "install the distro's clang: `apt install clang`, `dnf install clang`",
  };

  Err(bad(format!("clang not found: {hint}")))
}

fn which(name: &str) -> Option<PathBuf> {
  env::split_paths(&env::var_os("PATH")?).map(|x| x.join(exe(name))).find(|x| x.is_file())
}

fn exe(name: &str) -> String {
  format!("{name}{}", env::consts::EXE_SUFFIX)
}

/// Runs `rustup` with `args` for the toolchain of rustc's `sysroot`, which `RUSTC` may name over
/// rustup's active one; its output the user's.
fn rustup(sysroot: &Path, args: &[&str]) -> io::Result<()> {
  let name = sysroot.file_name().unwrap_or_default().to_string_lossy();
  let line = format!("rustup {} --toolchain {name}", args.join(" "));
  match Command::new("rustup").args(args).args(["--toolchain", &name]).status() {
    Ok(x) if x.success() => Ok(()),
    Ok(x) => Err(bad(format!("`{line}` failed ({x})"))),
    Err(e) if e.kind() == io::ErrorKind::NotFound => {
      Err(bad(format!("rustup not found, and cargo-solo needs `{line}`")))
    }
    Err(e) => Err(bad(format!("{line}: {e}"))),
  }
}

// MARK: Wrapper

/// The wrapper for `target`, `<target>-clang`: a link to this program in a folder of `bin` that its
/// path and the target's `sysroot` name. So a build never runs another cargo-solo, with other
/// tables, and when the sysroot changes, cargo and cc-rs see another `CC_<t>` and linker, and build
/// anew.
fn wrapper(bin: &Path, target: &Target, sysroot: &Path) -> io::Result<PathBuf> {
  let me = env::current_exe()?.canonicalize()?;
  let mut key = me.as_os_str().to_owned();
  key.push(sysroot);
  let path = folder(bin, &key).join(exe(&format!("{}-clang", target.rust)));
  place(&me, &path)?;
  Ok(path)
}

/// The folder for PATH with rustc's dlltool for Windows `target`, which makes import libraries for
/// the DLLs that `raw-dylib` links name, as windows-sys's do: a link to llvm-ar at `ar`, which runs
/// as llvm-dlltool by the name rustc looks for, `dlltool.exe` on Windows, `<target>-dlltool`
/// elsewhere.
fn dlltool(bin: &Path, target: &Target, ar: &Path) -> io::Result<PathBuf> {
  let name = if cfg!(windows) { exe("dlltool") } else { format!("{}-dlltool", target.clang) };
  let dir = folder(bin, ar.as_os_str());
  place(ar, &dir.join(name))?;
  Ok(dir)
}

/// A folder of `bin` that `key` names.
fn folder(bin: &Path, key: &OsStr) -> PathBuf {
  let hash = packages::hex(&hmac_sha256::Hash::hash(key.as_encoded_bytes()));
  bin.join(&hash[..12])
}

/// Makes `path` a link to the program `to`, anew when it is another's.
fn place(to: &Path, path: &Path) -> io::Result<()> {
  let dir = path.parent().unwrap_or(Path::new("."));
  fs::create_dir_all(dir)?;
  if runs(path, to) {
    return Ok(());
  }

  // Made beside it and renamed over it, so that a build running with the old one never misses it.
  let name = path.file_stem().unwrap_or_default().to_string_lossy();
  let part = dir.join(format!(".{name}-{}", std::process::id()));
  let _ = fs::remove_file(&part);
  link(to, &part)?;
  let done = fs::rename(&part, path);
  let _ = fs::remove_file(&part);
  match done {
    // Windows renames nothing over a program that runs: another run's link, if it is this one.
    Err(_) if runs(path, to) => Ok(()),
    Err(e) => Err(e),
    Ok(()) => Ok(()),
  }
}

#[cfg(unix)]
fn link(to: &Path, path: &Path) -> io::Result<()> {
  std::os::unix::fs::symlink(to, path)
}

/// Windows makes symlinks only for admins: a hard link, or a copy on another volume.
#[cfg(windows)]
fn link(to: &Path, path: &Path) -> io::Result<()> {
  fs::hard_link(to, path).or_else(|_| fs::copy(to, path).map(drop))
}

/// Whether the link at `path` runs the program `to`: links to it, or on Windows has its size and
/// time, as a hard link or a copy does.
fn runs(path: &Path, to: &Path) -> bool {
  #[cfg(unix)]
  return fs::read_link(path).is_ok_and(|x| x == to);
  #[cfg(windows)]
  {
    let meta = |x: &Path| fs::metadata(x).and_then(|x| Ok((x.len(), x.modified()?))).ok();
    meta(path).is_some_and(|x| meta(to) == Some(x))
  }
}

/// The clang that the wrapper for `target` runs for `args`: with them, then the target's
/// [flags](Target::flags).
pub fn wrap(target: &Target, args: Vec<OsString>) -> io::Result<Command> {
  let var = |name| {
    let hint = format!("{name} is not set: {}-clang runs under `cargo solo`", target.rust);
    env::var_os(name).ok_or_else(|| bad(hint))
  };

  let (clang, gcc_ld) = (var(CLANG)?, var(GCC_LD)?);
  let sdk = env::var_os(SDKROOT).filter(|x| target.libc == Libc::Darwin && !x.is_empty());
  let dir = match sdk {
    Some(x) => PathBuf::from(x),
    None => packages::dir(&packages::cache()?, target.sysroot(), target.packages),
  };

  let flags = target.flags(&dir, Path::new(&gcc_ld), &args);
  let mut cmd = Command::new(clang);
  cmd.args(args).args(flags);
  Ok(cmd)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn args(line: &str) -> Vec<OsString> {
    line.split_whitespace().map(OsString::from).collect()
  }

  #[test]
  fn targets_are_rustc_s() {
    let out = run(Command::new("rustc").args(["--print", "target-list"])).unwrap();
    let list = String::from_utf8(out).unwrap();
    for target in &TARGETS {
      assert!(list.lines().any(|x| x == target.rust), "{}", target.rust);
    }
  }

  #[test]
  fn wrapped_reads_the_wrapper_s_name() {
    let got = |x: &str| Target::wrapped(OsStr::new(x)).map(|x| x.rust);
    let gnu = Some("x86_64-unknown-linux-gnu");
    assert_eq!(got("/cache/bin/x86_64-unknown-linux-gnu-clang"), gnu);
    assert_eq!(got("x86_64-unknown-linux-gnu-clang"), gnu);
    assert_eq!(got("C:/cache/bin/x86_64-pc-windows-gnu-clang.exe"), Some("x86_64-pc-windows-gnu"));
    assert_eq!(got("/usr/bin/clang"), None);
    assert_eq!(got("aarch64-linux-android-clang"), None);
    assert_eq!(got("cargo-solo"), None);
    // On a Mac, Apple's targets are cargo's.
    let apple = (env::consts::OS != "macos").then_some("aarch64-apple-darwin");
    assert_eq!(got("aarch64-apple-darwin-clang"), apple);
  }

  #[test]
  fn env_sets_the_compiler_linker_and_archiver() {
    let target = Target::find("aarch64-unknown-linux-musl").unwrap();
    let [cc, ar, linker] = target.env(Path::new("/c/cc"), Path::new("/r/llvm-ar"));
    assert_eq!(cc, ("CC_aarch64-unknown-linux-musl".into(), "/c/cc".into()));
    assert_eq!(ar, ("AR_aarch64-unknown-linux-musl".into(), "/r/llvm-ar".into()));
    assert_eq!(linker, ("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER".into(), "/c/cc".into()));
  }

  #[test]
  fn chosen_finds_the_vars_in_args_and_cargo_s_config() {
    let dir = env::temp_dir().join(format!("cargo-solo-chosen-{}", std::process::id()));
    let (here, config) = (dir.join("a/b"), dir.join("a/.cargo/config.toml"));
    fs::create_dir_all(dir.join("a/.cargo")).unwrap();
    fs::create_dir_all(&here).unwrap();
    let vars = ["CARGO_SOLO_TEST_CHOSEN", "CARGO_SOLO_TEST_CHOSEN_x"];
    let chosen = |line| chosen(&vars, &args(line), &here);
    assert!(!chosen("build"));
    assert!(chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_x=0"));
    assert!(!chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_y=0"));

    let comments = "[env]\n# CARGO_SOLO_TEST_CHOSEN = \"0\"\nA = 1 # CARGO_SOLO_TEST_CHOSEN\n";
    fs::write(&config, comments).unwrap();
    assert!(!chosen("build"));

    fs::write(here.join("my.toml"), "[env]\nCARGO_SOLO_TEST_CHOSEN_x = \"0\"\n").unwrap();
    assert!(chosen("build --config my.toml"));
    assert!(chosen("build --config=my.toml"));

    fs::write(&config, r##"env = { A = "\"#", B = '#', CARGO_SOLO_TEST_CHOSEN = "0" }"##).unwrap();
    assert!(chosen("build"));
    fs::remove_dir_all(&dir).unwrap();
  }

  #[test]
  fn major_reads_clang_s_version() {
    assert_eq!(major("Debian clang version 14.0.6\nTarget: aarch64-unknown-linux-gnu"), Some(14));
    assert_eq!(major("Ubuntu clang version 18.1.3 (1ubuntu1)"), Some(18));
    assert_eq!(major("clang version 19.1.5\nTarget: x86_64-pc-windows-msvc"), Some(19));
    assert_eq!(major("gcc (GCC) 14.2.0"), None);
  }

  #[test]
  fn flags_link_with_lld_only_when_linking() {
    let (dir, gcc_ld) = (Path::new("/c/sr"), Path::new("/r/gcc-ld"));
    let flags = |name, line| {
      let target = TARGETS.iter().find(|x| x.rust == name).unwrap();
      let got = target.flags(dir, gcc_ld, &args(line));
      got.iter().map(|x| x.to_str().unwrap().replace('\\', "/")).collect::<Vec<_>>().join(" ")
    };

    let gnu = "x86_64-unknown-linux-gnu";
    let sysroot = "--target=x86_64-linux-gnu --sysroot=/c/sr";
    let link = "-fuse-ld=lld -B/r/gcc-ld -Qunused-arguments";
    assert_eq!(flags(gnu, "-c a.c -o a.o"), sysroot);
    assert_eq!(flags(gnu, "-E -dM x.c"), sysroot);
    assert_eq!(flags(gnu, "a.o -o a -lgcc_s"), format!("{sysroot} {link}"));
    assert_eq!(flags(gnu, "@/tmp/linker-args"), format!("{sysroot} {link}"));

    let include = "-idirafter /c/sr/usr/include/aarch64-linux-musl";
    let asm = "-idirafter /c/sr/usr/include/aarch64-linux-gnu";
    let musl = format!("--target=aarch64-linux-musl --sysroot=/c/sr {include} {asm}");
    assert_eq!(flags("aarch64-unknown-linux-musl", "-c a.c"), musl);
    let lib =
      "-B/c/sr/usr/lib/aarch64-linux-musl -L/c/sr/usr/lib/aarch64-linux-musl -static-libgcc";
    assert_eq!(flags("aarch64-unknown-linux-musl", "a.c -o a"), format!("{musl} {link} {lib}"));

    let mingw = format!("--target=x86_64-w64-mingw32 --sysroot=/c/sr/usr {link}");
    assert_eq!(flags("x86_64-pc-windows-gnu", "a.o -o a.exe"), mingw);

    let mac = "--target=arm64-apple-macos -isysroot /c/sr";
    assert_eq!(flags("aarch64-apple-darwin", "-c a.c -isysroot /x"), mac);
    assert_eq!(flags("aarch64-apple-darwin", "a.o -o a -lSystem"), format!("{mac} {link}"));
    let x64 = format!("--target=x86_64-apple-macos -isysroot /c/sr {link}");
    assert_eq!(flags("x86_64-apple-darwin", "a.o -o a"), x64);
  }

  #[test]
  fn apple_s_targets_share_a_sysroot() {
    let apple: Vec<_> = TARGETS.iter().filter(|x| x.libc == Libc::Darwin).collect();
    assert_eq!(apple.len(), 2);
    assert!(
      apple.iter().all(|x| x.sysroot() == "macos" && x.here() == (env::consts::OS != "macos"))
    );
    assert_eq!(Target::find("x86_64-unknown-linux-gnu").unwrap().sysroot(), "x86_64-linux-gnu");
  }
}
