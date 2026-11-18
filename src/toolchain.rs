// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! Each target's toolchain: the targets cargo-solo sets up, the host's clang and rustup's lld and
//! llvm-ar they share, the environment cargo runs with, its cmake toolchain file, and the wrapper:
//! this program run as a target's C and C++ compiler and linker, which adds the target's flags to
//! clang's.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};

use crate::packages::{self, Cxx, Libc, Package};
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
/// The flags bindgen gives its clang beside its own, by this name and the target's.
const BINDGEN: &str = "BINDGEN_EXTRA_CLANG_ARGS";
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
  /// Its libstdc++, but for Apple's: C only.
  pub cxx: Option<&'static Cxx>,
}

pub const TARGETS: [Target; 7] = [
  Target {
    rust: "x86_64-unknown-linux-gnu",
    clang: "x86_64-linux-gnu",
    libc: Libc::Glibc,
    packages: packages::GLIBC_X64,
    cxx: Some(&packages::CXX_GLIBC_X64),
  },
  Target {
    rust: "aarch64-unknown-linux-gnu",
    clang: "aarch64-linux-gnu",
    libc: Libc::Glibc,
    packages: packages::GLIBC_ARM64,
    cxx: Some(&packages::CXX_GLIBC_ARM64),
  },
  Target {
    rust: "x86_64-unknown-linux-musl",
    clang: "x86_64-linux-musl",
    libc: Libc::Musl,
    packages: packages::MUSL_X64,
    cxx: Some(&packages::CXX_MUSL_X64),
  },
  Target {
    rust: "aarch64-unknown-linux-musl",
    clang: "aarch64-linux-musl",
    libc: Libc::Musl,
    packages: packages::MUSL_ARM64,
    cxx: Some(&packages::CXX_MUSL_ARM64),
  },
  Target {
    rust: "x86_64-pc-windows-gnu",
    clang: "x86_64-w64-mingw32",
    libc: Libc::Mingw,
    packages: packages::MINGW_X64,
    cxx: Some(&packages::CXX_MINGW_X64),
  },
  Target {
    rust: "aarch64-apple-darwin",
    clang: "arm64-apple-macos",
    libc: Libc::Darwin,
    packages: packages::DARWIN,
    cxx: None,
  },
  Target {
    rust: "x86_64-apple-darwin",
    clang: "x86_64-apple-macos",
    libc: Libc::Darwin,
    packages: packages::DARWIN,
    cxx: None,
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

  /// The target whose wrapper `arg0` runs, and whether it runs as the C++ compiler:
  /// `<target>-clang` or `<target>-clang++`.
  pub fn wrapped(arg0: &OsStr) -> Option<(&'static Target, bool)> {
    let stem = Path::new(arg0).file_stem()?.to_str()?;
    match stem.strip_suffix("-clang++") {
      Some(x) => Some((Target::find(x)?, true)),
      None => Some((Target::find(stem.strip_suffix("-clang")?)?, false)),
    }
  }

  /// The name of the folder with this target's libstdc++, in the cache.
  pub fn libstdcxx(&self) -> String {
    format!("{}-libstdc++", self.clang)
  }

  /// cc's and cmake-rs's names for this target's variable `base`, in the order they read them,
  /// before `base` itself: `<base>_<target>` (cargo-solo's), `<base>_<target_u>`,
  /// `TARGET_<base>`, or `HOST_<base>` when it's the `host`'s own.
  pub fn names(&self, base: &str, host: &str) -> [String; 3] {
    let under = self.rust.replace('-', "_");
    let kind = if self.rust == host { "HOST" } else { "TARGET" };
    [format!("{base}_{}", self.rust), format!("{base}_{under}"), format!("{kind}_{base}")]
  }

  /// The environment cargo runs with for this target: the wrapper at `cc` its C compiler and its
  /// linker, which rustc takes for a C compiler driving the link by its name, `*-clang`; llvm-ar at
  /// `ar` its archiver, and as `<target>-ranlib` in `tools` its ranlib, which OpenSSL's build
  /// otherwise runs as plain `ranlib`, the host's.
  pub fn env(&self, cc: &Path, ar: &Path, tools: &Path) -> [(String, OsString); 4] {
    let upper = self.rust.to_uppercase().replace('-', "_");
    let ranlib = tools.join(exe(&format!("{}-ranlib", self.rust)));
    [
      (format!("CC_{}", self.rust), cc.into()),
      (format!("AR_{}", self.rust), ar.into()),
      (format!("RANLIB_{}", self.rust), ranlib.into()),
      (format!("CARGO_TARGET_{upper}_LINKER"), cc.into()),
    ]
  }

  /// The folders pkg-config searches for this target, its sysroot at `dir`, in place of the
  /// host's: for the host's own triple, cargo lets it run, and it would give the host's libraries.
  pub fn pkg_config(&self, dir: &Path) -> io::Result<OsString> {
    let usr = dir.join("usr");
    let dirs =
      [usr.join("lib").join(self.clang), usr.join(self.clang).join("lib"), usr.join("lib")];
    let dirs = dirs.iter().map(|x| x.join("pkgconfig")).chain([usr.join("share/pkgconfig")]);
    env::join_paths(dirs).map_err(|e| bad(format!("{}: {e}", dir.display())))
  }

  /// The flags bindgen's clang takes for this target, its sysroot at `dir`, which would otherwise
  /// parse the host's headers: the wrapper's to compile, each quoted, as bindgen splits them as a
  /// shell does. For Apple, clang takes the SDK from SDKROOT, which may be the user's.
  pub fn bindgen(&self, dir: &Path) -> OsString {
    let mut flags = self.flags(dir, Path::new(""), &["-c".into()]);
    if self.libc == Libc::Darwin {
      flags.truncate(1);
    }

    let quote = |x: &OsString| format!("'{}'", x.to_string_lossy().replace('\'', r"'\''"));
    let flags: Vec<_> = flags.iter().map(quote).collect();
    flags.join(" ").into()
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

  /// The flags the wrapper adds for C++ after [flags](Target::flags), libstdc++'s folder at `dir`:
  /// as `<target>-clang++` (`cxx`), clang as g++ with libstdc++'s headers, and to link, the folder
  /// and what GCC's g++ links for the target. As `<target>-clang`, the folder only, for a link that
  /// names `-lstdc++`, as rustc's of a crate's C++ through cc does.
  pub fn cxx_flags(&self, dir: &Path, cxx: bool, args: &[OsString]) -> Vec<OsString> {
    let Some(lib) = self.cxx else {
      return vec![];
    };

    let mut out = vec![];
    if cxx {
      out.push("--driver-mode=g++".into());
      let include = dir.join(lib.include);
      for x in [include.clone(), include.join(lib.triple), include.join("backward")] {
        out.push("-stdlib++-isystem".into());
        out.push(x.into());
      }

      // GCC's libstdc++ for Windows keeps its thread_locals, as std::call_once's, in emulated TLS.
      if self.libc == Libc::Mingw {
        out.push("-femulated-tls".into());
      }
    }

    if compiles(args) {
      return out;
    }

    out.push(cat("-L", &dir.join(lib.lib)));
    if !cxx {
      return out;
    }

    match self.libc {
      // glibc 2.28 has its threads in libpthread.
      Libc::Glibc => out.push("-pthread".into()),
      // libgcc and winpthreads static, as rustc links them, not their DLLs.
      Libc::Mingw => out.extend(["-static-libgcc".into(), "-l:libpthread.a".into()]),
      Libc::Musl | Libc::Darwin => {}
    }

    if lib.libgcc {
      out.push(dir.join("libgcc.a").into());
    }

    out
  }

  /// The cmake toolchain file for this target, its sysroot at `dir`: the system, the `tools` by
  /// cmake's names (`C_COMPILER`, `AR`), the C++ compiler cc takes, and as where to find
  /// libraries, headers and packages, never programs, the sysroot (for Apple, the SDK in SDKROOT)
  /// and cargo's build folder. Paths with `/`, as cmake reads them on Windows too.
  pub fn cmake(&self, dir: &Path, tools: &[(&str, &Path)], host: &str) -> String {
    let path = |x: &Path| x.to_string_lossy().replace('\\', "/");
    let arch = |x: &'static str| x.split('-').next().unwrap_or_default();
    // As cmake-rs names them.
    let (system, cpu) = match self.libc {
      Libc::Mingw => ("Windows", "AMD64"),
      Libc::Darwin => ("Darwin", arch(self.clang)),
      _ => ("Linux", arch(self.rust)),
    };

    let mut out = format!("# cargo-solo's for {}\n", self.rust);
    out += &format!("set(CMAKE_SYSTEM_NAME {system})\nset(CMAKE_SYSTEM_PROCESSOR {cpu})\n");
    match self.libc {
      Libc::Mingw => {
        let usr = dir.join("usr");
        out += &format!("set(CMAKE_SYSROOT \"{}\")\n", path(&usr));
        // mingw-w64's libraries are in its own folder of the sysroot.
        out += &format!("set(CMAKE_FIND_ROOT_PATH \"{}\")\n", path(&usr.join(self.clang)));
      }
      Libc::Darwin => out += CMAKE_SDK,
      _ => out += &format!("set(CMAKE_SYSROOT \"{}\")\n", path(dir)),
    }

    for (name, x) in tools {
      out += &format!("set(CMAKE_{name} \"{}\")\n", path(x));
    }

    let names = self.names("CXX", host).join(" ");
    out += &CMAKE_CXX.replace("NAMES", &names).replace("WRAPPERS", WRAPPERS);
    out += CMAKE_BUILD;
    let modes =
      [("PROGRAM", "NEVER"), ("LIBRARY", "ONLY"), ("INCLUDE", "ONLY"), ("PACKAGE", "ONLY")];
    for (kind, mode) in modes {
      out += &format!("set(CMAKE_FIND_ROOT_PATH_MODE_{kind} {mode})\n");
    }

    out
  }
}

/// The toolchain file's C++ compiler, cc's: the first of cc's variables (NAMES) set, the user's or
/// cargo-solo's wrapper, read as cc reads it: a path, or else words, the compiler and its flags,
/// after a wrapper cc knows, such as sccache, which runs it. cmake-rs passes no compiler with a
/// toolchain file, nor those flags.
const CMAKE_CXX: &str = r#"foreach(x NAMES)
  if(DEFINED ENV{${x}})
    file(TO_CMAKE_PATH "$ENV{${x}}" CMAKE_CXX_COMPILER)
    if(NOT EXISTS "${CMAKE_CXX_COMPILER}")
      string(REGEX MATCHALL "[^ \t]+" CMAKE_CXX_COMPILER "$ENV{${x}}")
    endif()
    list(LENGTH CMAKE_CXX_COMPILER _cargo_solo_n)
    if(_cargo_solo_n GREATER 1)
      set(_cargo_solo_wrappers WRAPPERS "$ENV{CC_KNOWN_WRAPPER_CUSTOM}")
      string(TOLOWER "${_cargo_solo_wrappers}" _cargo_solo_wrappers)
      list(GET CMAKE_CXX_COMPILER 0 _cargo_solo_name)
      get_filename_component(_cargo_solo_name "${_cargo_solo_name}" NAME_WE)
      string(TOLOWER "${_cargo_solo_name}" _cargo_solo_name)
      list(FIND _cargo_solo_wrappers "${_cargo_solo_name}" _cargo_solo_i)
      if(_cargo_solo_i GREATER -1)
        list(GET CMAKE_CXX_COMPILER 0 CMAKE_CXX_COMPILER_LAUNCHER)
        list(REMOVE_AT CMAKE_CXX_COMPILER 0)
      endif()
    endif()
    break()
  endif()
endforeach()
"#;

/// The wrappers cc knows, in [CMAKE_CXX].
const WRAPPERS: &str = "ccache distcc sccache icecc cachepot buildcache kache";

/// The toolchain file's SDK for Apple, the wrapper's: SDKROOT, cargo-solo's or the user's, which
/// cmake reads itself only as a path with `/`.
const CMAKE_SDK: &str = r#"file(TO_CMAKE_PATH "$ENV{SDKROOT}" CMAKE_OSX_SYSROOT)
set(CMAKE_FIND_ROOT_PATH "${CMAKE_OSX_SYSROOT}")
"#;

/// The toolchain file's folder of cargo's build scripts, three up from OUT_DIR's, to find in too:
/// another crate's build script puts what this one's cmake finds there, as libz-sys its zlib for
/// rdkafka-sys. The sysroot alone as the root would move those paths under it.
const CMAKE_BUILD: &str = r#"if(DEFINED ENV{OUT_DIR})
  file(TO_CMAKE_PATH "$ENV{OUT_DIR}/../../.." _cargo_solo_build)
  get_filename_component(_cargo_solo_build "${_cargo_solo_build}" ABSOLUTE)
  list(APPEND CMAKE_FIND_ROOT_PATH "${_cargo_solo_build}")
  unset(_cargo_solo_build)
endif()
"#;

/// Whether clang only compiles with `args`, links nothing.
fn compiles(args: &[OsString]) -> bool {
  args.iter().any(|x| ["-c", "-E", "-S", "-M", "-MM"].iter().any(|f| x == f))
}

/// Whether `args` name `-lstdc++`, or a response file of rustc's does, one per line.
fn links_stdcxx(args: &[OsString]) -> bool {
  let file = |x: &OsStr| x.to_str()?.strip_prefix('@').and_then(|x| fs::read_to_string(x).ok());
  args
    .iter()
    .any(|x| x == "-lstdc++" || file(x).is_some_and(|x| x.lines().any(|x| x.trim() == "-lstdc++")))
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

  /// Sets `target` up: its std through rustup when it's missing, its sysroot, its wrapper; and
  /// gives the environment cargo runs with for it: its [tools](tools) in front of PATH, its
  /// [C++ and cmake](Host::cxx), its sysroot for [pkg-config](Target::pkg_config) and
  /// [bindgen](Target::bindgen), for Windows aws-lc-sys's NASM objects, for Apple the
  /// [SDK and the oldest macOS](Host::darwin), each but the tools unless the user chose it in
  /// cargo's `args`, config or environment.
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
    // libstdc++'s by its name only: it's fetched on the first C++ compile.
    let libs = target.cxx.map_or(&[][..], |x| x.packages);
    let cxx_dir = packages::dir(&self.cache, &target.libstdcxx(), libs);
    let [cc, cxx] = wrapper(&self.cache.join("bin"), target, [&sysroot, &cxx_dir])?;
    let ar = self.bin.join(exe("llvm-ar"));
    let dir = tools(&self.cache.join("bin"), target, &self.bin)?;
    let mut out = target.env(&cc, &ar, &dir).to_vec();
    out.extend(darwin);
    out.extend(self.cxx(target, &sysroot, [&cc, &cxx], &dir, args)?);

    let path = env::var_os("PATH").unwrap_or_default();
    let path = env::join_paths([dir].into_iter().chain(env::split_paths(&path)));
    out.push(("PATH".into(), path.map_err(|e| bad(format!("PATH: {e}")))?));
    let here = env::current_dir()?;
    // pkg-config's names, as bindgen's but for `TARGET_<base>`, `HOST_<base>` for the host's own.
    let kind = if target.rust == self.triple { "HOST" } else { "TARGET" };
    let vars =
      [("PKG_CONFIG_LIBDIR", target.pkg_config(&sysroot)?), (BINDGEN, target.bindgen(&sysroot))];
    for (base, value) in vars {
      let name = format!("{base}_{}", target.rust);
      let under = format!("{base}_{}", target.rust.replace('-', "_"));
      if !chosen(&[base, &name, &under, &format!("{kind}_{base}")], args, &here) {
        out.push((name, value));
      }
    }

    if target.libc != Libc::Mingw {
      return Ok(out);
    }

    let nasm = format!("{PREBUILT_NASM}_{}", target.rust.replace('-', "_"));
    if !chosen(&[PREBUILT_NASM, &nasm], args, &here) {
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

  /// The environment for `target`'s C++ and cmake, with its `sysroot`, wrappers and the folder of
  /// its [tools](tools): `CXX_<t>` the wrapper as clang++, but for Apple, which has no C++
  /// library, `CMAKE_TOOLCHAIN_FILE_<t>` the target's toolchain file, and on a Windows
  /// host with ninja on PATH, Ninja for cmake's generator, where cmake-rs would take Visual Studio
  /// or MinGW's make; each unless the user chose.
  fn cxx(
    &self,
    target: &Target,
    sysroot: &Path,
    [cc, cxx]: [&Path; 2],
    tools: &Path,
    args: &[OsString],
  ) -> io::Result<Vec<(String, OsString)>> {
    let here = env::current_dir()?;
    // cc reads `CXX` for the host too, cmake-rs its own for every target.
    let free = |base: &str, plain: bool| {
      let names = target.names(base, &self.triple);
      let mut vars: Vec<_> = names.iter().map(String::as_str).collect();
      vars.extend(plain.then_some(base));
      (!chosen(&vars, args, &here)).then(|| names[0].clone())
    };

    let mut out = vec![];
    if target.cxx.is_some()
      && let Some(name) = free("CXX", false)
    {
      out.push((name, cxx.into()));
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

/// Whether the user set one of `vars` for cargo: in its environment or `args`, or outside comments
/// in a config file it reads: a `.toml` that `args` name, and `config.toml` in `.cargo`
/// of `here`, of each folder above and of CARGO_HOME. By the name alone, which is all a choice
/// needs.
fn chosen(vars: &[&str], args: &[OsString], here: &Path) -> bool {
  let name = |x: char| x.is_alphanumeric() || x == '_' || x == '-';
  let named = |x: &str| x.split(|x| !name(x)).any(|x| vars.contains(&x));
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

/// The wrappers for `target`, `<target>-clang` and `<target>-clang++`: links to this program in a
/// folder of `bin` that its path and the target's `dirs`, its sysroot's and libstdc++'s, name. So a
/// build never runs another cargo-solo, with other tables, and when either changes, cargo and cc-rs
/// see another `CC_<t>`, `CXX_<t>` and linker, and build anew.
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

/// The folder for PATH with the binutils of `target` by the names a configure script looks for
/// with `--host`: `<triple>-ar`, `-ranlib` and `-nm`, by Rust's triple and Debian's, for the host's
/// own, such as macOS's ar, can't read the target's objects. They are links to rustup's tools in
/// `llvm`: llvm-ar, which runs as ranlib by that name, and llvm-nm. For Windows, also rustc's
/// dlltool, which makes import libraries for the DLLs that `raw-dylib` links name, as
/// windows-sys's do: llvm-ar as `dlltool.exe` on Windows, `<target>-dlltool` elsewhere. For Apple,
/// llvm-objcopy as `<target>-install_name_tool`, which it runs as by that name, for cmake, which
/// stops without one.
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

/// Writes `text` to `path` when it's not there, as [place] puts a link.
fn put(path: &Path, text: &str) -> io::Result<()> {
  if fs::read_to_string(path).is_ok_and(|x| x == text) {
    return Ok(());
  }

  let dir = path.parent().unwrap_or(Path::new("."));
  fs::create_dir_all(dir)?;
  let name = path.file_name().unwrap_or_default().to_string_lossy();
  let part = dir.join(format!(".{name}-{}", std::process::id()));
  let done = fs::write(&part, text).and_then(|()| fs::rename(&part, path));
  let _ = fs::remove_file(&part);
  match done {
    Err(_) if fs::read_to_string(path).is_ok_and(|x| x == text) => Ok(()),
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

/// The clang that the wrapper for `target` runs for `args`: with them, then the target's
/// [flags](Target::flags), and as clang++ (`cxx`) or for a link with libstdc++, its
/// [C++ flags](Target::cxx_flags), libstdc++ fetched first when it's not there.
pub fn wrap(target: &Target, cxx: bool, args: Vec<OsString>) -> io::Result<Command> {
  let var = |name| {
    let hint = format!("{name} is not set: {}-clang runs under `cargo solo`", target.rust);
    env::var_os(name).ok_or_else(|| bad(hint))
  };

  let (clang, gcc_ld) = (var(CLANG)?, var(GCC_LD)?);
  let cache = packages::cache()?;
  let sdk = env::var_os(SDKROOT).filter(|x| target.libc == Libc::Darwin && !x.is_empty());
  let dir = match sdk {
    Some(x) => PathBuf::from(x),
    None => packages::dir(&cache, target.sysroot(), target.packages),
  };

  let mut flags = target.flags(&dir, Path::new(&gcc_ld), &args);
  if cxx || links_stdcxx(&args) {
    let lib = target.cxx.ok_or_else(|| bad(format!("no C++ for {}: only C", target.rust)))?;
    let dir = packages::ensure(&cache, &target.libstdcxx(), lib.packages)?;
    flags.extend(target.cxx_flags(&dir, cxx, &args));
  }

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
    let got = |x: &str| Target::wrapped(OsStr::new(x)).map(|(x, cxx)| (x.rust, cxx));
    let gnu = "x86_64-unknown-linux-gnu";
    assert_eq!(got("/cache/bin/x86_64-unknown-linux-gnu-clang"), Some((gnu, false)));
    assert_eq!(got("x86_64-unknown-linux-gnu-clang"), Some((gnu, false)));
    assert_eq!(got("/cache/bin/x86_64-unknown-linux-gnu-clang++"), Some((gnu, true)));
    let mingw = Some(("x86_64-pc-windows-gnu", true));
    assert_eq!(got("C:/cache/bin/x86_64-pc-windows-gnu-clang++.exe"), mingw);
    assert_eq!(got("/usr/bin/clang"), None);
    assert_eq!(got("/usr/bin/clang++"), None);
    assert_eq!(got("aarch64-linux-android-clang"), None);
    assert_eq!(got("x86_64-unknown-linux-gnu-ranlib"), None);
    assert_eq!(got("cargo-solo"), None);
    // On a Mac, Apple's targets are cargo's.
    let apple = (env::consts::OS != "macos").then_some(("aarch64-apple-darwin", false));
    assert_eq!(got("aarch64-apple-darwin-clang"), apple);
  }

  #[test]
  fn names_are_cc_s_and_cmake_rs_s() {
    let target = Target::find("aarch64-unknown-linux-musl").unwrap();
    let mut names =
      ["CXX_aarch64-unknown-linux-musl", "CXX_aarch64_unknown_linux_musl", "TARGET_CXX"];
    assert_eq!(target.names("CXX", "aarch64-apple-darwin"), names);
    // cc's and cmake-rs's for a build for the host itself.
    names[2] = "HOST_CXX";
    assert_eq!(target.names("CXX", "aarch64-unknown-linux-musl"), names);
  }

  #[test]
  fn env_sets_the_compiler_linker_archiver_and_ranlib() {
    let target = Target::find("aarch64-unknown-linux-musl").unwrap();
    let [cc, ar, ranlib, linker] =
      target.env(Path::new("/c/cc"), Path::new("/r/llvm-ar"), Path::new("/t"));
    assert_eq!(cc, ("CC_aarch64-unknown-linux-musl".into(), "/c/cc".into()));
    assert_eq!(ar, ("AR_aarch64-unknown-linux-musl".into(), "/r/llvm-ar".into()));
    let path = Path::new("/t").join(exe("aarch64-unknown-linux-musl-ranlib"));
    assert_eq!(ranlib, ("RANLIB_aarch64-unknown-linux-musl".into(), path.into()));
    assert_eq!(linker, ("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER".into(), "/c/cc".into()));
  }

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
  fn chosen_finds_the_vars_in_args_and_cargo_s_config() {
    let dir = env::temp_dir().join(format!("cargo-solo-chosen-{}", std::process::id()));
    let (here, config) = (dir.join("a/b"), dir.join("a/.cargo/config.toml"));
    fs::create_dir_all(dir.join("a/.cargo")).unwrap();
    fs::create_dir_all(&here).unwrap();
    let vars = ["CARGO_SOLO_TEST_CHOSEN", "CARGO_SOLO_TEST_CHOSEN_x", "CARGO_SOLO_TEST_CHOSEN_x-y"];
    let chosen = |line| chosen(&vars, &args(line), &here);
    assert!(!chosen("build"));
    assert!(chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_x=0"));
    assert!(!chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_y=0"));
    assert!(chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_x-y=0"));
    assert!(!chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_x-z=0"));

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
  fn bindgen_and_pkg_config_take_the_target_s_sysroot() {
    let dir = Path::new("/c/sr");
    let target = |name| TARGETS.iter().find(|x| x.rust == name).unwrap();
    let bindgen = |name| target(name).bindgen(dir).into_string().unwrap().replace('\\', "/");
    let gnu = "'--target=x86_64-linux-gnu' '--sysroot=/c/sr'";
    assert_eq!(bindgen("x86_64-unknown-linux-gnu"), gnu);
    let include = "'-idirafter' '/c/sr/usr/include/aarch64-linux-musl'";
    let asm = "'-idirafter' '/c/sr/usr/include/aarch64-linux-gnu'";
    let musl = format!("'--target=aarch64-linux-musl' '--sysroot=/c/sr' {include} {asm}");
    assert_eq!(bindgen("aarch64-unknown-linux-musl"), musl);
    let mingw = "'--target=x86_64-w64-mingw32' '--sysroot=/c/sr/usr'";
    assert_eq!(bindgen("x86_64-pc-windows-gnu"), mingw);
    assert_eq!(bindgen("aarch64-apple-darwin"), "'--target=arm64-apple-macos'");
    let quoted = target("x86_64-unknown-linux-gnu").bindgen(Path::new("/it's"));
    assert_eq!(quoted, r"'--target=x86_64-linux-gnu' '--sysroot=/it'\''s'");

    let dirs = target("x86_64-pc-windows-gnu").pkg_config(dir).unwrap();
    let dirs: Vec<_> =
      env::split_paths(&dirs).map(|x| x.to_str().unwrap().replace('\\', "/")).collect();
    let usr = ["lib/x86_64-w64-mingw32", "x86_64-w64-mingw32/lib", "lib", "share"];
    assert_eq!(dirs, usr.map(|x| format!("/c/sr/usr/{x}/pkgconfig")));
  }

  #[test]
  fn apple_s_targets_share_a_sysroot() {
    let apple: Vec<_> = TARGETS.iter().filter(|x| x.libc == Libc::Darwin).collect();
    assert_eq!(apple.len(), 2);
    assert!(
      apple.iter().all(|x| x.sysroot() == "macos" && x.here() == (env::consts::OS != "macos"))
    );
    assert_eq!(Target::find("x86_64-unknown-linux-gnu").unwrap().sysroot(), "x86_64-linux-gnu");
    // C only for Apple's.
    assert!(TARGETS.iter().all(|x| x.cxx.is_none() == (x.libc == Libc::Darwin)));
  }

  #[test]
  fn cxx_flags_are_g_plus_plus_s() {
    let dir = Path::new("/c/cxx");
    let flags = |name, cxx, line| {
      let target = Target::find(name).unwrap();
      let got = target.cxx_flags(dir, cxx, &args(line));
      got.iter().map(|x| x.to_str().unwrap().replace('\\', "/")).collect::<Vec<_>>().join(" ")
    };

    let gnu = "x86_64-unknown-linux-gnu";
    let include = "/c/cxx/include/c++/14";
    let headers = format!(
      "--driver-mode=g++ -stdlib++-isystem {include} -stdlib++-isystem \
       {include}/x86_64-redhat-linux -stdlib++-isystem {include}/backward"
    );
    let lib = "-L/c/cxx/lib/gcc/x86_64-redhat-linux/14";
    assert_eq!(flags(gnu, true, "-c a.cc"), headers);
    assert_eq!(flags(gnu, true, "a.o -o a"), format!("{headers} {lib} -pthread"));
    // rustc's link of a crate's C++, which names -lstdc++.
    assert_eq!(flags(gnu, false, "a.o -lstdc++ -o a"), lib);

    let musl = flags("aarch64-unknown-linux-musl", true, "a.o -o a");
    assert!(
      musl.contains("-stdlib++-isystem /c/cxx/include/c++/14.2.0/aarch64-alpine-linux-musl ")
    );
    assert!(musl.ends_with(" -L/c/cxx/lib /c/cxx/libgcc.a"));

    let mingw = flags("x86_64-pc-windows-gnu", true, "a.o -o a.exe");
    assert!(mingw.contains("/c/cxx/14-posix/include/c++/x86_64-w64-mingw32 "));
    assert!(mingw.ends_with(" -femulated-tls -L/c/cxx/14-posix -static-libgcc -l:libpthread.a"));
  }

  #[test]
  fn links_stdcxx_reads_rustc_s_response_file() {
    let file = env::temp_dir().join(format!("cargo-solo-args-{}", std::process::id()));
    fs::write(&file, "a.o\n-lstdc++\n-lc\n").unwrap();
    let mut with = args("-o a");
    with.push(cat("@", &file));
    assert!(links_stdcxx(&with));
    assert!(links_stdcxx(&args("a.o -lstdc++ -o a")));
    fs::write(&file, "a.o\n-lc\n").unwrap();
    assert!(!links_stdcxx(&with));
    assert!(!links_stdcxx(&args("a.o -lstdc -o a")));
    fs::remove_file(file).unwrap();
  }

  #[test]
  fn cmake_names_the_system_sysroot_and_tools() {
    const HOST: &str = "aarch64-apple-darwin";
    let gnu = Target::find("aarch64-unknown-linux-gnu").unwrap();
    let text = gnu.cmake(Path::new("/c/sr"), &[("C_COMPILER", Path::new("/c/b/cc"))], HOST);
    assert!(text.contains("set(CMAKE_SYSTEM_NAME Linux)\nset(CMAKE_SYSTEM_PROCESSOR aarch64)\n"));
    assert!(text.contains("set(CMAKE_SYSROOT \"/c/sr\")\nset(CMAKE_C_COMPILER \"/c/b/cc\")\n"));
    assert!(text.contains("set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)\n"));
    assert!(!text.contains("set(CMAKE_FIND_ROOT_PATH "));
    assert!(text.contains("list(APPEND CMAKE_FIND_ROOT_PATH \"${_cargo_solo_build}\")\n"));
    let names = "CXX_aarch64-unknown-linux-gnu CXX_aarch64_unknown_linux_gnu TARGET_CXX";
    assert!(text.contains(&format!("foreach(x {names})\n")));

    let mingw = Target::find("x86_64-pc-windows-gnu").unwrap();
    let text = mingw.cmake(Path::new(r"C:\c\sr"), &[("AR", Path::new(r"C:\r\llvm-ar.exe"))], HOST);
    assert!(text.contains("set(CMAKE_SYSTEM_NAME Windows)\nset(CMAKE_SYSTEM_PROCESSOR AMD64)\n"));
    assert!(text.contains("set(CMAKE_FIND_ROOT_PATH \"C:/c/sr/usr/x86_64-w64-mingw32\")\n"));
    assert!(text.contains("set(CMAKE_AR \"C:/r/llvm-ar.exe\")\n"));

    let mac = TARGETS.iter().find(|x| x.rust == "aarch64-apple-darwin").unwrap();
    let text = mac.cmake(Path::new("/c/sdk"), &[("CXX_COMPILER", Path::new("/c/b/cc"))], HOST);
    assert!(text.contains("set(CMAKE_SYSTEM_NAME Darwin)\nset(CMAKE_SYSTEM_PROCESSOR arm64)\n"));
    assert!(text.contains("file(TO_CMAKE_PATH \"$ENV{SDKROOT}\" CMAKE_OSX_SYSROOT)\n"));
    assert!(!text.contains("/c/sdk") && !text.contains("CMAKE_SYSROOT"));
  }
}
