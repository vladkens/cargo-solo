// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! Sets the host up for a target: finds clang and rustup's tools, adds the target's std, fetches
//! its sysroot, links the wrapper into the cache, and gives cargo's environment for it.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};

use crate::packages::{self, Libc};
use crate::toolchain::{CLANG, GCC_LD, SDKROOT, TARGETS, Target, exe, slash};
use crate::{bad, chosen, run};

/// aws-lc-sys, rustls' crypto, assembles x86_64 Windows' code with nasm, which a host seldom has;
/// this lets it take the objects it ships instead, when there's no nasm.
const PREBUILT_NASM: &str = "AWS_LC_SYS_PREBUILT_NASM";
/// The flags bindgen gives its clang beside its own, by this name and the target's.
const BINDGEN: &str = "BINDGEN_EXTRA_CLANG_ARGS";
/// The oldest macOS an Apple binary runs on, which rustc and cc-rs read: without it, cc-rs off a
/// Mac passes 10.7 for x86_64.
const DEPLOYMENT_TARGET: &str = "MACOSX_DEPLOYMENT_TARGET";

/// The tools every target shares: the host's clang, and rustup's lld and llvm-ar.
pub struct Host {
  clang: PathBuf,
  /// rustc's sysroot, each target's std in its `lib/rustlib/<target>`.
  sysroot: PathBuf,
  /// rustup's tools for the host, `lib/rustlib/<host>/bin` in rustc's sysroot.
  bin: PathBuf,
  /// The host's target, by Rust's name.
  triple: String,
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

    let triple = host.to_string();
    Ok(Host { clang: clang()?, sysroot, bin, triple, cache: packages::cache()? })
  }

  /// The environment the wrapper reads, the same for every target.
  pub fn env(&self) -> [(&'static str, OsString); 3] {
    [
      (CLANG, self.clang.clone().into()),
      (GCC_LD, self.bin.join("gcc-ld").into()),
      ("CARGO_SOLO_CACHE", self.cache.clone().into()),
    ]
  }

  /// Sets `target` up (its std, sysroot and wrapper) and gives cargo's environment for it: its
  /// [tools] first in PATH, [C++ and cmake](Host::cxx), pkg-config, bindgen, aws-lc's NASM objects
  /// for Windows, [Apple's](Host::darwin); each but the tools unless the user chose it.
  pub fn setup(&self, target: &Target, args: &[OsString]) -> io::Result<Vec<(String, OsString)>> {
    let here = env::current_dir()?;
    let set = |vars: &[&str]| chosen(vars, args, &here);
    // The target's first name for `base`, unless the user set one of them, or `base` when `plain`.
    let free = |base: &str, plain: bool| {
      let names = target.names(base, &self.triple);
      let mut vars: Vec<_> = names.iter().map(String::as_str).collect();
      vars.extend(plain.then_some(base));
      (!set(&vars)).then(|| names[0].clone())
    };

    // First, as it checks the tools before any download.
    let darwin = match target.libc {
      Libc::Darwin => self.darwin(target, set)?,
      _ => vec![],
    };

    if !self.sysroot.join("lib/rustlib").join(target.rust).join("lib").is_dir() {
      rustup(&self.sysroot, &["target", "add", target.rust])?;
    }

    let sysroot = packages::ensure(&self.cache, target.sysroot(), target.packages)?;
    // libstdc++'s by its name only: it's fetched on the first C++ compile.
    let libs = target.cxx.map_or(&[][..], |x| x.packages);
    let cxx_dir = packages::dir(&self.cache, &target.libstdcxx(), libs);
    let [cc, cxx] = wrapper(&self.cache.join("bin"), target, [&sysroot, &cxx_dir])?;
    let ar = self.bin.join(exe("llvm-ar"));
    let dir = tools(&self.cache.join("bin"), target, &self.bin)?;
    let mut out = target.env(&cc, &ar, &dir).to_vec();
    out.extend(darwin);
    out.extend(self.cxx(target, &sysroot, [&cc, &cxx], &dir, free)?);

    let path = env::var_os("PATH").unwrap_or_default();
    let path = env::join_paths([dir].into_iter().chain(env::split_paths(&path)));
    out.push(("PATH".into(), path.map_err(|e| bad(format!("PATH: {e}")))?));
    let vars =
      [("PKG_CONFIG_LIBDIR", target.pkg_config(&sysroot)?), (BINDGEN, target.bindgen(&sysroot))];
    for (base, value) in vars {
      out.extend(free(base, true).map(|x| (x, value)));
    }

    if target.libc != Libc::Mingw {
      return Ok(out);
    }

    let nasm = format!("{PREBUILT_NASM}_{}", target.rust.replace('-', "_"));
    if !set(&[PREBUILT_NASM, &nasm]) {
      out.push((nasm, "1".into()));
    }

    Ok(out)
  }

  /// For Apple's `target`: [SDKROOT], its sysroot, and [DEPLOYMENT_TARGET], the oldest of rustc's
  /// for Apple, each unless the user chose it. Neither has a name per target, so one value serves
  /// both: rustc and clang raise x86_64's 10.12 to arm64's minimum, 11.0.
  fn darwin(
    &self,
    target: &Target,
    set: impl Fn(&[&str]) -> bool,
  ) -> io::Result<Vec<(String, OsString)>> {
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

    let mut out = vec![];
    if !set(&[SDKROOT]) {
      let sdk = packages::dir(&self.cache, target.sysroot(), target.packages);
      out.push((SDKROOT.into(), sdk.into()));
    }

    if set(&[DEPLOYMENT_TARGET]) {
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

  /// The environment for `target`'s C++ and cmake: `CXX_<t>` the wrapper as clang++ (not for Apple,
  /// with no C++ library), `CMAKE_TOOLCHAIN_FILE_<t>`, and on Windows with ninja, Ninja over Visual
  /// Studio or MinGW's make; each unless the user chose.
  fn cxx(
    &self,
    target: &Target,
    sysroot: &Path,
    [cc, cxx]: [&Path; 2],
    tools: &Path,
    free: impl Fn(&str, bool) -> Option<String>,
  ) -> io::Result<Vec<(String, OsString)>> {
    let mut out = vec![];
    // Not plain `CXX`: cc reads it for the host too.
    if target.cxx.is_some()
      && let Some(name) = free("CXX", false)
    {
      out.push((name, slash(cxx)));
    }

    // For Apple, cmake's C++ compiler is the C one, which compiles the C++ that needs no library,
    // as cmake's check of its C++ compiler does.
    let cxx = if target.cxx.is_some() { cxx } else { cc };

    if let Some(name) = free("CMAKE_TOOLCHAIN_FILE", true) {
      let (ar, nm) = (self.bin.join(exe("llvm-ar")), self.bin.join(exe("llvm-nm")));
      // The host's ranlib, as macOS's, can't read the target's objects.
      let ranlib = tools.join(exe(&format!("{}-ranlib", target.rust)));
      let name_tool = tools.join(exe(&format!("{}-install_name_tool", target.rust)));
      let mut tools = vec![
        ("C_COMPILER", cc),
        ("CXX_COMPILER", cxx),
        ("ASM_COMPILER", cc),
        ("AR", &ar),
        ("RANLIB", &ranlib),
        ("NM", &nm),
      ];
      if target.libc == Libc::Darwin {
        tools.push(("INSTALL_NAME_TOOL", &name_tool));
      }

      let text = target.cmake(sysroot, &tools, &self.triple);
      let bin = self.cache.join("bin");
      let path = folder(&bin, OsStr::new(&text)).join(format!("{}.cmake", target.rust));
      put(&path, &text)?;
      out.push((name, path.into()));
    }

    if cfg!(windows) && which("ninja").is_some() {
      out.extend(free("CMAKE_GENERATOR", true).map(|x| (x, "Ninja".into())));
    }

    Ok(out)
  }
}

/// The major version in `clang --version`'s `text`.
fn major(text: &str) -> Option<u32> {
  text.split("clang version ").nth(1)?.split('.').next()?.parse().ok()
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

/// `<target>-clang` and `-clang++`, links to this program in a folder of `bin` named by its path
/// and the target's `dirs`: a build never runs another cargo-solo, and when either changes, cargo
/// and cc-rs see another compiler and build anew.
fn wrapper(bin: &Path, target: &Target, dirs: [&Path; 2]) -> io::Result<[PathBuf; 2]> {
  let me = env::current_exe()?.canonicalize()?;
  let mut key = me.as_os_str().to_owned();
  for dir in dirs {
    key.push(dir);
  }

  let dir = folder(bin, &key);
  let out = ["clang", "clang++"].map(|x| dir.join(exe(&format!("{}-{x}", target.rust))));
  for path in &out {
    place(&me, path)?;
  }

  Ok(out)
}

/// A folder for PATH with rustup's tools in `llvm` by the names a configure script's `--host` looks
/// for, as the host's can't read the target's objects; for Windows rustc's dlltool, for
/// `raw-dylib`; for Apple `install_name_tool`, without which cmake stops.
fn tools(bin: &Path, target: &Target, llvm: &Path) -> io::Result<PathBuf> {
  let (ar, nm) = (llvm.join(exe("llvm-ar")), llvm.join(exe("llvm-nm")));
  let objcopy = llvm.join(exe("llvm-objcopy"));
  let tools = [("ar", &ar), ("ranlib", &ar), ("nm", &nm)];
  let triples = [target.rust, target.clang];
  let mut names: Vec<_> =
    triples.iter().flat_map(|t| tools.map(|(x, to)| (to, format!("{t}-{x}")))).collect();

  match target.libc {
    Libc::Mingw if cfg!(windows) => names.push((&ar, "dlltool".into())),
    Libc::Mingw => names.push((&ar, format!("{}-dlltool", target.clang))),
    Libc::Darwin => names.push((&objcopy, format!("{}-install_name_tool", target.rust))),
    _ => {}
  }

  let dir = folder(bin, llvm.as_os_str());
  for (to, name) in names {
    place(to, &dir.join(exe(&name)))?;
  }

  Ok(dir)
}

/// A folder of `bin` that `key` names.
fn folder(bin: &Path, key: &OsStr) -> PathBuf {
  let hash = packages::hex(&hmac_sha256::Hash::hash(key.as_encoded_bytes()));
  bin.join(hash.get(..12).unwrap_or(&hash))
}

/// Makes `path` a link to the program `to`, anew when it is another's.
fn place(to: &Path, path: &Path) -> io::Result<()> {
  swap(path, |x| link(to, x), || runs(path, to))
}

/// Writes `text` to `path` when it's not there, as [place] puts a link.
fn put(path: &Path, text: &str) -> io::Result<()> {
  swap(path, |x| fs::write(x, text), || fs::read_to_string(path).is_ok_and(|x| x == text))
}

/// Puts what `make` makes at `path`, unless it's `there`: made beside it and renamed over it, so
/// that a build running with the old one never misses it.
fn swap(
  path: &Path,
  make: impl FnOnce(&Path) -> io::Result<()>,
  there: impl Fn() -> bool,
) -> io::Result<()> {
  let dir = path.parent().unwrap_or(Path::new("."));
  fs::create_dir_all(dir)?;
  if there() {
    return Ok(());
  }

  let name = path.file_name().unwrap_or_default().to_string_lossy();
  let part = dir.join(format!(".{name}-{}", std::process::id()));
  let _ = fs::remove_file(&part);
  let done = make(&part).and_then(|()| fs::rename(&part, path));
  let _ = fs::remove_file(&part);
  match done {
    // Windows renames nothing over a program that runs: another run's, if it is the same.
    Err(_) if there() => Ok(()),
    x => x,
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn tools_are_llvm_s_by_the_target_s_names() {
    let dir = env::temp_dir().join(format!("cargo-solo-tools-{}", std::process::id()));
    let llvm = dir.join("llvm");
    fs::create_dir_all(&llvm).unwrap();
    let (ar, nm) = (llvm.join(exe("llvm-ar")), llvm.join(exe("llvm-nm")));
    let objcopy = llvm.join(exe("llvm-objcopy"));
    fs::write(&ar, "ar").unwrap();
    fs::write(&nm, "nm").unwrap();
    fs::write(&objcopy, "objcopy").unwrap();

    let musl = tools(&dir, Target::find("x86_64-unknown-linux-musl").unwrap(), &llvm).unwrap();
    for (to, name) in [(&ar, "x86_64-unknown-linux-musl-ar"), (&ar, "x86_64-linux-musl-ranlib")] {
      assert!(runs(&musl.join(exe(name)), to), "{name}");
    }

    assert!(runs(&musl.join(exe("x86_64-linux-musl-nm")), &nm));
    assert!(!musl.join(exe("x86_64-w64-mingw32-dlltool")).exists());

    let mingw = tools(&dir, Target::find("x86_64-pc-windows-gnu").unwrap(), &llvm).unwrap();
    let dlltool = if cfg!(windows) { "dlltool" } else { "x86_64-w64-mingw32-dlltool" };
    assert_eq!(mingw, musl);
    assert!(runs(&mingw.join(exe(dlltool)), &ar));
    assert!(runs(&mingw.join(exe("x86_64-w64-mingw32-ar")), &ar));
    assert!(!mingw.join(exe("aarch64-apple-darwin-install_name_tool")).exists());

    let apple = TARGETS.iter().find(|x| x.rust == "aarch64-apple-darwin").unwrap();
    let apple = tools(&dir, apple, &llvm).unwrap();
    assert!(runs(&apple.join(exe("aarch64-apple-darwin-install_name_tool")), &objcopy));
    fs::remove_dir_all(&dir).unwrap();
  }

  #[test]
  fn major_reads_clang_s_version() {
    assert_eq!(major("Debian clang version 14.0.6\nTarget: aarch64-unknown-linux-gnu"), Some(14));
    assert_eq!(major("Ubuntu clang version 18.1.3 (1ubuntu1)"), Some(18));
    assert_eq!(major("clang version 19.1.5\nTarget: x86_64-pc-windows-msvc"), Some(19));
    assert_eq!(major("gcc (GCC) 14.2.0"), None);
  }
}
