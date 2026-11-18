// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! The sysroots: each target's packages from the distros, rustup and Zig, pinned by SHA-256,
//! fetched by the system's `curl` on first use and unpacked into the cache.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs, io};

use crate::unpack::{copy, cpio, gunzip, materialize, member, payload, unpack, xz};
use crate::{bad, run};

/// Bumped when unpacking changes, so that every sysroot is unpacked anew into another folder.
const FORMAT: u32 = 2;

// MARK: Packages

/// The C library of a sysroot, which decides the flags clang gets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Libc {
  Glibc,
  Musl,
  Mingw,
  Darwin,
}

impl Libc {
  /// The library and its version, for the help.
  pub fn version(self) -> &'static str {
    match self {
      Libc::Glibc => "glibc 2.28",
      Libc::Musl => "musl 1.2.5",
      Libc::Mingw => "mingw-w64 12.0",
      Libc::Darwin => "libSystem, SDK 27.0",
    }
  }
}

/// A file of a distro or rustup, tried from each mirror in turn: the SHA-256 decides.
#[derive(Debug)]
pub struct Package {
  pub mirrors: &'static [&'static str],
  /// The path under each mirror, of a `.deb`, an `.rpm`, an `.apk` or a `.tar.xz`.
  pub path: &'static str,
  pub sha256: &'static str,
  /// The folder in the archive the files are taken from; of those, the ones whose path there
  /// starts with one of `keep` go under `into` in the sysroot.
  pub root: &'static str,
  pub keep: &'static [&'static str],
  pub into: &'static str,
}

/// What a sysroot keeps of a Debian package: the headers and the libraries, and the folder of
/// mingw-w64's headers, which its include folder links into.
const DEB_KEEP: &[&str] = &[
  "lib/",
  "lib64/",
  "usr/include/",
  "usr/lib/",
  "usr/share/mingw-w64/",
  "usr/x86_64-w64-mingw32/",
];

/// Debian 10 buster, frozen on archive.debian.org since 2024-03-31, and snapshot.debian.org's copy.
const BUSTER: &[&str] = &[
  "https://archive.debian.org/debian",
  "https://snapshot.debian.org/archive/debian-archive/20240331T102506Z/debian",
];

/// Debian 13 trixie, and snapshot.debian.org as of when every package here was current: one that a
/// point release replaces stays there.
const TRIXIE: &[&str] =
  &["https://deb.debian.org/debian", "https://snapshot.debian.org/archive/debian/20260510T204219Z"];

const RUSTUP: &[&str] = &["https://static.rust-lang.org"];

/// Zig's releases on ziglang.org, and a community mirror Zig lists, which serves the same files.
const ZIG: &[&str] = &["https://ziglang.org/download/0.17.1", "https://pkg.hexops.org/zig"];

/// AlmaLinux 8.10's AppStream, RHEL 8's rebuilt, and kernel.org's copy: 8's last release, whose
/// packages stay until 8's end in 2029.
const ALMA8: &[&str] = &[
  "https://repo.almalinux.org/almalinux/8.10/AppStream",
  "https://mirrors.edge.kernel.org/almalinux/8.10/AppStream",
];

/// Alpine 3.21's main, and kernel.org's copy: out of support since 2026-11-01, so its packages no
/// longer change.
const ALPINE: &[&str] = &[
  "https://dl-cdn.alpinelinux.org/alpine/v3.21/main",
  "https://mirrors.edge.kernel.org/alpine/v3.21/main",
];

impl Package {
  const fn deb(
    mirrors: &'static [&'static str],
    path: &'static str,
    sha256: &'static str,
  ) -> Package {
    Package { mirrors, path, sha256, root: "", keep: DEB_KEEP, into: "" }
  }

  const fn zig(root: &'static str, keep: &'static [&'static str], into: &'static str) -> Package {
    Package {
      mirrors: ZIG,
      path: "zig-0.17.1.tar.xz",
      sha256: "1e89bf06610b769a8c77676c6c8d54b093a0db60f4891f2f91014829012f0e24",
      root,
      keep,
      into,
    }
  }

  const fn only(
    self,
    root: &'static str,
    keep: &'static [&'static str],
    into: &'static str,
  ) -> Package {
    Package { root, keep, into, ..self }
  }

  /// Where `path`, a file of its archive, goes in the sysroot; `None` when it's not kept.
  pub fn place(&self, path: &str) -> Option<PathBuf> {
    let rest = path.strip_prefix(self.root)?;
    // glibc's charset modules, loaded at run time only: 7 of its 12 MB.
    if !self.keep.iter().any(|x| rest.starts_with(x)) || rest.contains("/gconv/") {
      return None;
    }

    Some(PathBuf::from(format!("{}{rest}", self.into)))
  }
}

/// The kernel's headers, Linux 4.19's, for x86_64: glibc's and musl's.
const LINUX_X64: Package = Package::deb(
  BUSTER,
  "pool/main/l/linux/linux-libc-dev_4.19.249-2_amd64.deb",
  "c512f30f5abe413464b79ed23b54958df8cc104446c3ae93d83d28e23728a571",
);

/// [LINUX_X64] for aarch64.
const LINUX_ARM64: Package = Package::deb(
  BUSTER,
  "pool/main/l/linux/linux-libc-dev_4.19.249-2_arm64.deb",
  "e9a92d71d62655f673433c345a03fecaa4292f93c6572b28fd3d16dcfd1cc7f6",
);

/// GCC 8's crtbegin.o and libgcc for x86_64: glibc's and musl's.
const LIBGCC_X64: Package = Package::deb(
  BUSTER,
  "pool/main/g/gcc-8/libgcc-8-dev_8.3.0-6_amd64.deb",
  "a52d5216858c7185bc245e52ad95a6ad515d087bc25e8556df06c5626b41a837",
);

/// [LIBGCC_X64] for aarch64.
const LIBGCC_ARM64: Package = Package::deb(
  BUSTER,
  "pool/main/g/gcc-8/libgcc-8-dev_8.3.0-6_arm64.deb",
  "d8385c0106d4ac559ada75b162aecb9d507a67da3ac8742eb32258f501d07c7e",
);

/// What a musl sysroot takes of [LIBGCC_X64]: libgcc static, with no glibc's libgcc_s.
const MUSL_LIBGCC: &[&str] = &["crtbegin", "crtend", "libgcc.a", "libgcc_eh.a"];

/// glibc 2.28 for x86_64: libc, the kernel's headers, and GCC's crtbegin.o, libgcc and libgcc_s.
pub const GLIBC_X64: &[Package] = &[
  Package::deb(
    BUSTER,
    "pool/main/g/glibc/libc6_2.28-10+deb10u1_amd64.deb",
    "0ee0f0bfcd7b59ffda1dbc76ef58fa79512913204f2a336206467bfbf45bc69f",
  ),
  Package::deb(
    BUSTER,
    "pool/main/g/glibc/libc6-dev_2.28-10+deb10u1_amd64.deb",
    "76b8413920d91a9a4289f99839933d0d30095e0ec12b05bb4f1a7f27e5ef4a55",
  ),
  LINUX_X64,
  LIBGCC_X64,
  Package::deb(
    BUSTER,
    "pool/main/g/gcc-8/libgcc1_8.3.0-6_amd64.deb",
    "b1bb7611f3372732889d502cb1d09fe572b5fbb5288a4a8b1ed0363fecc3555a",
  ),
];

/// [GLIBC_X64] for aarch64.
pub const GLIBC_ARM64: &[Package] = &[
  Package::deb(
    BUSTER,
    "pool/main/g/glibc/libc6_2.28-10+deb10u1_arm64.deb",
    "09f5e92b1527cdc06b757499bb4d97ddf1b064fc3d163f978e5de18b131cd330",
  ),
  Package::deb(
    BUSTER,
    "pool/main/g/glibc/libc6-dev_2.28-10+deb10u1_arm64.deb",
    "04097716018288cef8b82339faa1a9847838b89f39574b5b169f4657da8bf269",
  ),
  LINUX_ARM64,
  LIBGCC_ARM64,
  Package::deb(
    BUSTER,
    "pool/main/g/gcc-8/libgcc1_8.3.0-6_arm64.deb",
    "2851ac25d12958586c035de5ec4f2fc17272dec48f776dd0dd24c62f62674fd9",
  ),
];

/// musl 1.2.5 for x86_64, the one Rust bundles, Debian's in trixie: its headers, and its libc.a and
/// crt objects for what clang links itself, as a configure script's test (Rust links its own,
/// `self-contained`); the kernel's headers; GCC's crtbegin.o and libgcc, in musl's folder.
pub const MUSL_X64: &[Package] = &[
  Package::deb(
    TRIXIE,
    "pool/main/m/musl/musl-dev_1.2.5-3.1~deb13u1_amd64.deb",
    "a3f7d47e452129bb65aae201d6c115abf4d5d3efb136b38b7c6b4fa03ec065e9",
  ),
  LINUX_X64,
  LIBGCC_X64.only("usr/lib/gcc/x86_64-linux-gnu/8/", MUSL_LIBGCC, "usr/lib/x86_64-linux-musl/"),
];

/// [MUSL_X64] for aarch64.
pub const MUSL_ARM64: &[Package] = &[
  Package::deb(
    TRIXIE,
    "pool/main/m/musl/musl-dev_1.2.5-3.1~deb13u1_arm64.deb",
    "384ea029e3694baac696d3993d93246f9a4b3741c493697e1ceb7256d8820748",
  ),
  LINUX_ARM64,
  LIBGCC_ARM64.only("usr/lib/gcc/aarch64-linux-gnu/8/", MUSL_LIBGCC, "usr/lib/aarch64-linux-musl/"),
];

/// mingw-w64 for x86_64: the headers, the CRT with its crtbegin.o, the import libraries; and
/// libgcc, which Debian has only in its 38 MB cross GCC, from rustup's rust-mingw: what rustc links
/// windows-gnu with on a Windows host.
pub const MINGW_X64: &[Package] = &[
  Package::deb(
    TRIXIE,
    "pool/main/m/mingw-w64/mingw-w64-common_12.0.0-5_all.deb",
    "97dce5d0d8aff1cade4786ad4ef5078345477a598829afcac2e7f2fbc5399990",
  ),
  Package::deb(
    TRIXIE,
    "pool/main/m/mingw-w64/mingw-w64-x86-64-dev_12.0.0-5_all.deb",
    "0bf89cf7454cccb49cd2a46a6f7b33896f6ae3a1a2fe33e02c40be6155bbb385",
  ),
  Package {
    mirrors: RUSTUP,
    path: "dist/2026-11-12/rust-mingw-1.100.0-x86_64-pc-windows-gnu.tar.xz",
    sha256: "5491d2ad03b0f124307c8b2c4ac840b65e738a348f4381975f0e638b012e4ae9",
    root: "rust-mingw-1.100.0-x86_64-pc-windows-gnu/rust-mingw/lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained/",
    keep: &["libgcc"],
    into: "usr/x86_64-w64-mingw32/lib/",
  },
];

/// Apple's SDK for both arches, laid out as Xcode's is, from Zig's source: macOS's headers, and
/// libSystem's `.tbd`, the text stub that lists what it exports, which is all a link needs. Not
/// Zig's `SDKSettings.json`, which clang warns of on every file: it lacks a key clang requires.
pub const DARWIN: &[Package] = &[
  Package::zig("zig-0.17.1/lib/libc/include/any-darwin-any/", &[""], "usr/include/"),
  Package::zig("zig-0.17.1/lib/libc/darwin/", &["libSystem.tbd"], "usr/lib/"),
];

/// The libraries an SDK has as links to libSystem, which has them all: the libc crate links `-lc`
/// and `-lm`.
const LIBSYSTEM_ALIASES: &[&str] =
  &["c", "dbm", "dl", "gcc_s.1", "info", "m", "mx", "mx.A", "poll", "proc", "pthread", "rpcsvc"];

/// A stub of macOS's libiconv, which the libc crate links for Apple's targets and Zig's source
/// doesn't have: the functions of `iconv.h`.
const LIBICONV: &str = "--- !tapi-tbd
tbd-version:     4
targets:         [ x86_64-macos, arm64-macos, arm64e-macos ]
install-name:    '/usr/lib/libiconv.2.dylib'
current-version: 7
compatibility-version: 7
exports:
  - targets:         [ x86_64-macos, arm64-macos, arm64e-macos ]
    symbols:         [ __libiconv_version, _iconv, _iconv_canonicalize, _iconv_close,
                       _iconv_open, _iconv_open_into, _iconvctl, _iconvlist,
                       _libiconv_set_relocation_prefix ]
...
";

/// A target's libstdc++, GCC 14's, linked statically: its packages, unpacked into a folder of their
/// own on the first C++ compile, and where they put its headers and libraries there.
#[derive(Debug)]
pub struct Cxx {
  pub packages: &'static [Package],
  /// The headers, `include/c++/<version>`.
  pub include: &'static str,
  /// GCC's name for the target: the folder of `include` with the target's own headers.
  pub triple: &'static str,
  /// The folder of `libstdc++.a`.
  pub lib: &'static str,
  /// Whether the folder has GCC 14's `libgcc.a`, for what clang++ links itself.
  pub libgcc: bool,
}

/// GCC 14's libgcc for aarch64: GCC 14's libstdc++ calls aarch64's outline atomics
/// (`__aarch64_ldadd4_acq_rel`), which GCC 8's libgcc lacks. rustc links its own,
/// compiler_builtins'.
const LIBGCC14_ARM64: Package = Package {
  root: "usr/lib/gcc/aarch64-linux-gnu/14/",
  keep: &["libgcc.a"],
  ..Package::deb(
    TRIXIE,
    "pool/main/g/gcc-14/libgcc-14-dev_14.2.0-19_arm64.deb",
    "64b8ebc7182a69aa9525df6eab5e7eb849b947e350d821549f8a8a3e9b19e5ba",
  )
};

/// libstdc++ for glibc 2.28 on x86_64: gcc-toolset-14's, AlmaLinux 8's, built for glibc 2.28. Not
/// its `libstdc++.so`, a linker script into the host's `/usr/lib64`, nor `libstdc++_nonshared.a`,
/// which goes with it.
pub const CXX_GLIBC_X64: Cxx = Cxx {
  packages: &[Package {
    mirrors: ALMA8,
    path: "x86_64/os/Packages/gcc-toolset-14-libstdc++-devel-14.2.1-11.el8_10.x86_64.rpm",
    sha256: "a5306cbaa5f3223de9320374aeac00dbddee817e11bab3c8217e33a131eca0a1",
    root: "opt/rh/gcc-toolset-14/root/usr/",
    keep: &[
      "include/c++/14/",
      "lib/gcc/x86_64-redhat-linux/14/libstdc++.a",
      "lib/gcc/x86_64-redhat-linux/14/libstdc++fs.a",
      "lib/gcc/x86_64-redhat-linux/14/libsupc++.a",
    ],
    into: "",
  }],
  include: "include/c++/14",
  triple: "x86_64-redhat-linux",
  lib: "lib/gcc/x86_64-redhat-linux/14",
  libgcc: false,
};

/// [CXX_GLIBC_X64] for aarch64.
pub const CXX_GLIBC_ARM64: Cxx = Cxx {
  packages: &[
    Package {
      mirrors: ALMA8,
      path: "aarch64/os/Packages/gcc-toolset-14-libstdc++-devel-14.2.1-11.el8_10.aarch64.rpm",
      sha256: "c70b6f1a0c67d4798e488772ce55dbceef0268a5ea551da83829965bd5f5235f",
      root: "opt/rh/gcc-toolset-14/root/usr/",
      keep: &[
        "include/c++/14/",
        "lib/gcc/aarch64-redhat-linux/14/libstdc++.a",
        "lib/gcc/aarch64-redhat-linux/14/libstdc++fs.a",
        "lib/gcc/aarch64-redhat-linux/14/libsupc++.a",
      ],
      into: "",
    },
    LIBGCC14_ARM64,
  ],
  include: "include/c++/14",
  triple: "aarch64-redhat-linux",
  lib: "lib/gcc/aarch64-redhat-linux/14",
  libgcc: true,
};

/// What a musl target keeps of Alpine's libstdc++-dev.
const ALPINE_CXX: &[&str] =
  &["include/c++/", "lib/libstdc++.a", "lib/libstdc++fs.a", "lib/libsupc++.a"];

/// libstdc++ for musl on x86_64: Alpine's, built for musl.
pub const CXX_MUSL_X64: Cxx = Cxx {
  packages: &[Package {
    mirrors: ALPINE,
    path: "x86_64/libstdc++-dev-14.2.0-r4.apk",
    sha256: "8a2c14b49e5b3d75ec32d6f62966df8b1abad200ab9479172805ddc0f221e1df",
    root: "usr/",
    keep: ALPINE_CXX,
    into: "",
  }],
  include: "include/c++/14.2.0",
  triple: "x86_64-alpine-linux-musl",
  lib: "lib",
  libgcc: false,
};

/// [CXX_MUSL_X64] for aarch64.
pub const CXX_MUSL_ARM64: Cxx = Cxx {
  packages: &[
    Package {
      mirrors: ALPINE,
      path: "aarch64/libstdc++-dev-14.2.0-r4.apk",
      sha256: "b347c2689cb1815510419dd68918c20649f48b0a7e665fea4cd8155dfa7adc9c",
      root: "usr/",
      keep: ALPINE_CXX,
      into: "",
    },
    LIBGCC14_ARM64,
  ],
  include: "include/c++/14.2.0",
  triple: "aarch64-alpine-linux-musl",
  lib: "lib",
  libgcc: true,
};

/// libstdc++ for x86_64 Windows: Debian's mingw-w64 g++, posix threads, only its headers and static
/// libraries: no `libstdc++.dll.a`, which would link the DLL.
pub const CXX_MINGW_X64: Cxx = Cxx {
  packages: &[Package {
    root: "usr/lib/gcc/x86_64-w64-mingw32/",
    keep: &[
      "14-posix/include/c++/",
      "14-posix/libstdc++.a",
      "14-posix/libstdc++fs.a",
      "14-posix/libsupc++.a",
    ],
    ..Package::deb(
      TRIXIE,
      "pool/main/g/gcc-mingw-w64/g++-mingw-w64-x86-64-posix_14.2.0-19+27+b1_amd64.deb",
      "cfa1773c2d0184819050138e587a81606d04a5f92e278b521b72b776d2c256c1",
    )
  }],
  include: "14-posix/include/c++",
  triple: "x86_64-w64-mingw32",
  lib: "14-posix",
  libgcc: false,
};

/// macOS's `sys/syscall.h`, which Zig's source doesn't have, without its numbers: jemalloc includes
/// it on every Unix, and syscall(2), deprecated on macOS, is what they're for.
const SYSCALL_H: &str = "/* cargo-solo's: macOS's header without the syscall numbers. */
#ifndef _SYS_SYSCALL_H_
#define _SYS_SYSCALL_H_
#endif
";

// MARK: Cache

/// The cache folder: `CARGO_SOLO_CACHE`, else the OS's own.
pub fn cache() -> io::Result<PathBuf> {
  let var = |name| env::var_os(name).filter(|x| !x.is_empty()).map(PathBuf::from);
  let dir = match (var("CARGO_SOLO_CACHE"), env::consts::OS) {
    (Some(dir), _) => Some(dir),
    (None, "macos") => var("HOME").map(|x| x.join("Library/Caches/cargo-solo")),
    (None, "windows") => var("LOCALAPPDATA").map(|x| x.join("cargo-solo")),
    (None, _) => {
      let base = var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|x| x.join(".cache")));
      base.map(|x| x.join("cargo-solo"))
    }
  };

  // Absolute: the wrapper runs in each build script's folder.
  std::path::absolute(dir.ok_or_else(|| bad("no cache folder: set CARGO_SOLO_CACHE"))?)
}

/// The sysroot `name` of `packages` in `cache`, `<name>-<hash>`: a changed table or [FORMAT]
/// unpacks into another folder.
pub fn dir(cache: &Path, name: &str, packages: &[Package]) -> PathBuf {
  let mut text = format!("{FORMAT}");
  for x in packages {
    text += &format!("\n{} {} {} {}", x.sha256, x.root, x.keep.join(" "), x.into);
  }

  let hash = hex(&hmac_sha256::Hash::hash(text.as_bytes()));
  cache.join(format!("{name}-{}", &hash[..12]))
}

/// [dir], fetched and unpacked first when it's not there: into a temporary folder, renamed when
/// complete, so that a run cut short, or one beside, never sees half of it.
pub fn ensure(cache: &Path, name: &str, packages: &[Package]) -> io::Result<PathBuf> {
  let dir = dir(cache, name, packages);
  if dir.is_dir() {
    return Ok(dir);
  }

  let tmp = cache.join("tmp");
  fs::create_dir_all(&tmp)?;
  sweep(&tmp);

  // The time too: runs in containers that share the cache have the same ids.
  let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
  let part = tmp.join(format!("{name}-{}-{now}", std::process::id()));
  eprintln!("cargo-solo: fetching {name} into {}", dir.display());
  let done = fill(&part, packages).and_then(|()| fs::rename(&part, &dir));
  let _ = fs::remove_dir_all(&part);
  match done {
    // Another run put its own in place first.
    Err(_) if dir.is_dir() => Ok(dir),
    Err(e) => Err(e),
    Ok(()) => Ok(dir),
  }
}

/// Removes what runs cut short left in `tmp`: anything over a day old.
fn sweep(tmp: &Path) {
  let day = Duration::from_secs(24 * 3600);
  for entry in fs::read_dir(tmp).into_iter().flatten().flatten() {
    let age = entry.metadata().and_then(|x| x.modified()).ok().and_then(|x| x.elapsed().ok());
    if age.is_some_and(|x| x > day) {
      let _ = fs::remove_dir_all(entry.path());
    }
  }
}

fn fill(root: &Path, packages: &[Package]) -> io::Result<()> {
  fs::create_dir_all(root)?;
  let mut links = vec![];
  // Packages in a row that are parts of one file share its download.
  let mut last: Option<(&str, Vec<u8>)> = None;
  for pkg in packages {
    let file = pkg.path.rsplit('/').next().unwrap_or(pkg.path);
    let at = |e: io::Error| io::Error::new(e.kind(), format!("{file}: {e}"));
    let tar = match last.take() {
      Some((path, tar)) if path == pkg.path => tar,
      _ => {
        let data = fetch(pkg, &root.join(".download")).map_err(at)?;
        let tar = match file.rsplit('.').next() {
          Some("deb") => member(&data, "data.tar.xz").and_then(xz),
          Some("rpm") => payload(&data).and_then(xz),
          Some("apk") => gunzip(&data),
          _ => xz(&data),
        };

        tar.map_err(at)?
      }
    };

    // An rpm's is a cpio archive, not a tar.
    let got = match file.ends_with(".rpm") {
      true => cpio(&tar, |x| pkg.place(x), root),
      false => unpack(&tar, |x| pkg.place(x), root),
    };

    links.extend(got.map_err(at)?);
    last = Some((pkg.path, tar));
  }

  materialize(root, links)?;
  if root.join("usr/lib/libSystem.tbd").is_file() {
    sdk(root)?;
  }

  Ok(())
}

/// Adds to Zig's files of Apple's SDK libSystem's aliases, libiconv and `sys/syscall.h`, and drops
/// `arm64e.x1` from libSystem's stub: rust-lld before Rust 1.100 can't read it.
fn sdk(root: &Path) -> io::Result<()> {
  let lib = root.join("usr/lib");
  let path = lib.join("libSystem.tbd");
  fs::write(&path, without(&fs::read_to_string(&path)?, "arm64e.x1-"))?;
  for name in LIBSYSTEM_ALIASES {
    copy(&path, &lib.join(format!("lib{name}.tbd")))?;
  }

  fs::write(lib.join("libiconv.tbd"), LIBICONV)?;
  let path = root.join("usr/include/sys/syscall.h");
  match path.exists() {
    true => Ok(()),
    false => fs::write(path, SYSCALL_H),
  }
}

/// The `.tbd` text `tbd` without the targets that start with `arch`, each with its comma.
fn without(tbd: &str, arch: &str) -> String {
  let mut out = String::with_capacity(tbd.len());
  let mut rest = tbd;
  while let Some(i) = rest.find(arch) {
    let mut end = rest[i..].find([',', ' ', '\n', ']']).map_or(rest.len(), |x| i + x);
    match rest[..i].trim_end().strip_suffix(',') {
      Some(head) => out.push_str(head),
      // The first in its list: the comma after it.
      None => {
        out.push_str(&rest[..i]);
        end += usize::from(rest[end..].starts_with(','));
      }
    }

    rest = &rest[end..];
  }

  out.push_str(rest);
  out
}

// MARK: Fetch

/// The file of `pkg` from the first mirror that has the pinned SHA-256, downloaded by `curl` into
/// `path`. No `--retry-all-errors`: Debian 10's and RHEL 8's curl lack it.
fn fetch(pkg: &Package, path: &Path) -> io::Result<Vec<u8>> {
  let mut failed = vec![];
  for mirror in pkg.mirrors {
    let url = format!("{mirror}/{}", pkg.path);
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "--connect-timeout", "30", "--speed-limit", "1024", "--speed-time", "60"]);
    curl.args(["--retry", "2", "-o"]).arg(path).arg(&url);

    let got = run(&mut curl).and_then(|_| fs::read(path));
    let _ = fs::remove_file(path);
    match got {
      Ok(data) => {
        let sum = hex(&hmac_sha256::Hash::hash(&data));
        if sum == pkg.sha256 {
          return Ok(data);
        }

        failed.push(format!("{url}: SHA-256 {sum}, pinned {}", pkg.sha256));
      }
      // No curl: no mirror would do.
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(e),
      Err(e) => failed.push(format!("{url}: {e}")),
    }
  }

  Err(io::Error::other(failed.join("; ")))
}

pub fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::toolchain::TARGETS;
  use crate::unpack::tests::{ar, tarball, temp};

  /// Every package of every target: its sysroot's and its libstdc++'s.
  fn every_package() -> impl Iterator<Item = &'static Package> {
    TARGETS.iter().flat_map(|x| x.packages.iter().chain(x.cxx.into_iter().flat_map(|x| x.packages)))
  }

  #[test]
  fn tables_are_pinned() {
    for pkg in every_package() {
      let hex = pkg.sha256.bytes().all(|x| x.is_ascii_digit() || (b'a'..=b'f').contains(&x));
      assert!(pkg.sha256.len() == 64 && hex, "{}", pkg.path);
      let kinds = [".deb", ".rpm", ".apk", ".tar.xz"];
      assert!(kinds.iter().any(|x| pkg.path.ends_with(x)), "{}", pkg.path);
      assert!(pkg.mirrors.iter().all(|x| x.starts_with("https://") && !x.ends_with('/')));
      assert!(pkg.root.is_empty() || pkg.root.ends_with('/'));
    }

    // Each sysroot and libstdc++ its own folder: targets share one only by its name, as Apple's do.
    let cache = Path::new("/c");
    let sysroots =
      TARGETS.iter().map(|x| (x.sysroot().to_string(), dir(cache, x.sysroot(), x.packages)));
    let cxx = TARGETS
      .iter()
      .filter_map(|x| Some((x.libstdcxx(), dir(cache, &x.libstdcxx(), x.cxx?.packages))));
    let dirs: Vec<_> = sysroots.chain(cxx).collect();
    for (i, (name, x)) in dirs.iter().enumerate() {
      assert!(dirs[..i].iter().all(|(n, y)| (n == name) == (y == x)), "{}", x.display());
    }
  }

  #[test]
  fn dir_changes_with_the_packages() {
    let pkg = |sha256| [Package::deb(BUSTER, "a.deb", sha256)];
    let (a, b) = (pkg("aa"), pkg("bb"));
    let cache = Path::new("/c");
    assert_eq!(dir(cache, "x", &a), dir(cache, "x", &a));
    assert_ne!(dir(cache, "x", &a), dir(cache, "x", &b));
    assert!(dir(cache, "x", &a).starts_with("/c"));
    assert!(dir(cache, "x", &a).file_name().unwrap().to_str().unwrap().starts_with("x-"));
  }

  #[test]
  fn place_keeps_headers_and_libraries() {
    let pkg = Package::deb(BUSTER, "a.deb", "");
    assert_eq!(pkg.place("usr/include/stdio.h"), Some("usr/include/stdio.h".into()));
    let libc = "lib/x86_64-linux-gnu/libc.so.6";
    assert_eq!(pkg.place(libc), Some(libc.into()));
    assert_eq!(pkg.place("usr/share/doc/libc6/copyright"), None);
    assert_eq!(pkg.place("usr/bin/ldd"), None);
    assert_eq!(pkg.place("usr/lib/x86_64-linux-gnu/gconv/UTF-16.so"), None);
    assert_eq!(pkg.place("usr/lib"), None);

    let mingw = &MINGW_X64[2];
    let got = mingw.place(&format!("{}libgcc_eh.a", mingw.root));
    assert_eq!(got, Some("usr/x86_64-w64-mingw32/lib/libgcc_eh.a".into()));
    assert_eq!(mingw.place(&format!("{}libmsvcrt.a", mingw.root)), None);

    let (musl, gcc) = (&MUSL_X64[2], "usr/lib/gcc/x86_64-linux-gnu/8");
    let got = musl.place(&format!("{gcc}/crtbeginS.o"));
    assert_eq!(got, Some("usr/lib/x86_64-linux-musl/crtbeginS.o".into()));
    assert_eq!(musl.place(&format!("{gcc}/libgcc_s.so")), None);
  }

  #[test]
  fn ensure_tries_the_next_mirror_and_checks_the_hash() {
    let root = temp("ensure");
    let tar = tarball(&[("./usr/include/a.h", "a")], &[]);
    let mut xz = vec![];
    lzma_rs::xz_compress(&mut &tar[..], &mut xz).unwrap();
    let file = ar(&[("debian-binary", b"2.0\n"), ("data.tar.xz", &xz)]);
    fs::create_dir_all(root.join("good/pool")).unwrap();
    fs::write(root.join("good/pool/a.deb"), &file).unwrap();

    let sha256: &str = hex(&hmac_sha256::Hash::hash(&file)).leak();
    let good: &str = format!("file://{}", root.join("good").display()).leak();
    let none: &str = format!("file://{}", root.join("none").display()).leak();
    let cache = root.join("cache");
    let pkgs = [Package::deb(vec![none, good].leak(), "pool/a.deb", sha256)];
    let dir = ensure(&cache, "x", &pkgs).unwrap();
    assert_eq!(fs::read_to_string(dir.join("usr/include/a.h")).unwrap(), "a");
    assert_eq!(fs::read_dir(cache.join("tmp")).unwrap().count(), 0);
    // Once there, it's not fetched again.
    fs::remove_file(root.join("good/pool/a.deb")).unwrap();
    assert_eq!(ensure(&cache, "x", &pkgs).unwrap(), dir);

    let wrong = [Package::deb(vec![good].leak(), "pool/a.deb", "00")];
    fs::write(root.join("good/pool/a.deb"), &file).unwrap();
    let err = ensure(&cache, "y", &wrong).unwrap_err().to_string();
    assert!(err.contains("a.deb: ") && err.contains(&format!("SHA-256 {sha256}, pinned 00")));
    assert!(!super::dir(&cache, "y", &wrong).exists());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn ensure_lays_out_apple_s_sdk() {
    let root = temp("sdk");
    let at = |i: usize, x: &str| format!("{}{x}", DARWIN[i].root);
    let files = [
      (at(0, "stdio.h"), "stdio"),
      (at(0, "sys/types.h"), "types"),
      (at(1, "libSystem.tbd"), "[ arm64e-macos, arm64e.x1-macos ]"),
      // Not taken: one that the sysroot leaves out, and another libc's.
      (at(1, "SDKSettings.json"), "{}"),
      (DARWIN[1].root.replace("/darwin/", "/glibc/") + "stdio.h", "glibc"),
    ];
    let files: Vec<_> = files.iter().map(|(x, data)| (x.as_str(), *data)).collect();
    let tar = tarball(&files, &[]);

    let mut xz = vec![];
    lzma_rs::xz_compress(&mut &tar[..], &mut xz).unwrap();
    fs::write(root.join(DARWIN[0].path), &xz).unwrap();

    let sha256: &str = hex(&hmac_sha256::Hash::hash(&xz)).leak();
    let mirror: &str = format!("file://{}", root.display()).leak();
    let pkgs: Vec<_> =
      DARWIN.iter().map(|x| Package { mirrors: vec![mirror].leak(), sha256, ..*x }).collect();
    let dir = ensure(&root.join("cache"), "macos", &pkgs).unwrap();

    let read = |x: &str| fs::read_to_string(dir.join(x)).unwrap();
    assert_eq!(read("usr/include/stdio.h"), "stdio");
    assert_eq!(read("usr/include/sys/types.h"), "types");
    assert_eq!(read("usr/lib/libSystem.tbd"), "[ arm64e-macos ]");
    assert_eq!(read("usr/lib/libc.tbd"), "[ arm64e-macos ]");
    assert_eq!(read("usr/lib/libm.tbd"), "[ arm64e-macos ]");
    assert!(read("usr/lib/libiconv.tbd").contains("'/usr/lib/libiconv.2.dylib'"));
    assert!(read("usr/include/sys/syscall.h").contains("_SYS_SYSCALL_H_"));

    let names = |x: &str| {
      let mut out: Vec<_> =
        fs::read_dir(dir.join(x)).unwrap().map(|x| x.unwrap().file_name()).collect();
      out.sort();
      out
    };
    assert_eq!(names(""), ["usr"]);
    assert_eq!(names("usr"), ["include", "lib"]);
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn without_drops_a_target_from_each_list() {
    let tbd =
      "targets:  [ x86_64-macos, arm64e-macos, \n    arm64e.x1-macos, arm64e.x1-maccatalyst ]
  - targets: [ arm64e.x1-macos, arm64-macos ]
    symbols: [ _a, _b ]";
    let want = "targets:  [ x86_64-macos, arm64e-macos ]
  - targets: [  arm64-macos ]
    symbols: [ _a, _b ]";
    assert_eq!(without(tbd, "arm64e.x1-"), want);
    assert_eq!(without("[ a ]", "arm64e.x1-"), "[ a ]");
  }

  #[test]
  #[ignore = "downloads every package from every mirror, ~240 MB"]
  fn every_package_is_on_every_mirror() {
    let root = temp("mirrors");
    let mut done = vec![];
    for pkg in every_package() {
      for mirror in pkg.mirrors {
        // Shared by targets, or one file's parts.
        if done.contains(&(mirror, pkg.path)) {
          continue;
        }

        let one = Package { mirrors: vec![*mirror].leak(), ..*pkg };
        fetch(&one, &root.join("file")).unwrap();
        done.push((mirror, pkg.path));
      }
    }

    fs::remove_dir_all(root).unwrap();
  }
}
