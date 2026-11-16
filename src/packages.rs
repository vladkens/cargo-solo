// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! The sysroots: each target's libc, headers and libraries from Debian's packages (mingw's libgcc
//! from rustup's, Apple's SDK from Zig's source), and its libstdc++ from AlmaLinux's, Alpine's or
//! Debian's, pinned by SHA-256, fetched by the system's `curl` on first use and unpacked into the
//! cache.

use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, io};

use tar::EntryType;

use crate::{bad, run};

/// Bumped when unpacking changes, so that every sysroot is unpacked anew into another folder.
const FORMAT: u32 = 1;

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

  /// The files of Zig's source tarball under `root` that start with one of `keep`, put under
  /// `into`.
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

  /// The package with only what its folder `root` has of `keep`, put in the sysroot's `into`.
  const fn only(
    self,
    root: &'static str,
    keep: &'static [&'static str],
    into: &'static str,
  ) -> Package {
    Package { root, keep, into, ..self }
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

// MARK: Cache

/// The cache folder: `CARGO_SOLO_CACHE`, else `~/Library/Caches/cargo-solo` on macOS,
/// `%LOCALAPPDATA%\cargo-solo` on Windows, `$XDG_CACHE_HOME/cargo-solo` or `~/.cache/cargo-solo`.
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

/// The sysroot `name` of `packages` in `cache`: `<name>-<hash>`, the hash of the packages and of
/// how they are unpacked, so that a changed table unpacks into another folder.
pub fn dir(cache: &Path, name: &str, packages: &[Package]) -> PathBuf {
  let mut text = format!("{FORMAT}");
  for x in packages {
    text += &format!("\n{} {} {} {}", x.sha256, x.root, x.keep.join(" "), x.into);
  }

  let hash = hex(&hmac_sha256::Hash::hash(text.as_bytes()));
  cache.join(format!("{name}-{}", &hash[..12]))
}

/// The sysroot `name` of `packages` in `cache`, fetched and unpacked first when it's not there:
/// into a temporary folder, renamed when complete, so that a run cut short, or one beside, never
/// sees half of it.
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

/// `packages` fetched and unpacked into `root`, their links put in place last, and Apple's SDK
/// completed.
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
      true => cpio(&tar, pkg, root),
      false => unpack(&tar, pkg, root),
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

/// What Apple's SDK at `root` has beside Zig's files: libSystem's aliases, and libiconv. Its
/// stub without `arm64e.x1`, which rust-lld before Rust 1.100 can't read: each list with it has
/// `arm64e` too.
fn sdk(root: &Path) -> io::Result<()> {
  let lib = root.join("usr/lib");
  let path = lib.join("libSystem.tbd");
  fs::write(&path, without(&fs::read_to_string(&path)?, "arm64e.x1-"))?;
  for name in LIBSYSTEM_ALIASES {
    copy(&path, &lib.join(format!("lib{name}.tbd")))?;
  }

  fs::write(lib.join("libiconv.tbd"), LIBICONV)
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

/// The file of `pkg`, downloaded by `curl` into `path` from each mirror in turn until one has the
/// pinned SHA-256. A stalled connection fails, and a transient error is retried twice; curl's
/// `--retry-all-errors` is newer than Debian 10's and RHEL 8's.
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

// MARK: Unpack

/// The data of the member `name` of the ar archive `ar`, as a `.deb` is: `!<arch>\n`, then for
/// each member a 60-byte header, its name in bytes 0..16 and its size in 48..58, and its data,
/// padded to an even size.
fn member<'a>(ar: &'a [u8], name: &str) -> io::Result<&'a [u8]> {
  if !ar.starts_with(b"!<arch>\n") {
    return Err(bad("not a .deb: no ar header"));
  }

  let mut at = 8;
  while let Some(head) = ar.get(at..at + 60) {
    let text = |from: usize, to: usize| String::from_utf8_lossy(&head[from..to]).trim().to_string();
    let size: usize = text(48, 58).parse().map_err(|_| bad("a malformed ar header"))?;
    let data = ar.get(at + 60..at + 60 + size).ok_or_else(|| bad("the .deb is cut short"))?;
    if text(0, 16).trim_end_matches('/') == name {
      return Ok(data);
    }

    at += 60 + size + size % 2;
  }

  Err(bad(format!("no {name} in the .deb")))
}

/// The data of the xz stream `data`. lzma-rs reads no SHA-256 check, which AlmaLinux's have, so its
/// blocks go to LZMA2 one by one and their checks are skipped: the pinned SHA-256 makes them moot.
/// A stream is a 12-byte header, the check's kind at 7, then blocks until the index, a zero. A
/// block is its header, `(first byte + 1) * 4` bytes, the data, padded to 4 bytes, and the check.
fn xz(data: &[u8]) -> io::Result<Vec<u8>> {
  let short = || bad("the xz stream is cut short");
  let [0xfd, b'7', b'z', b'X', b'Z', 0, 0, check, ..] = *data else {
    return Err(bad("not an xz stream"));
  };

  let check = match check & 15 {
    0 => 0,
    1 => 4,
    4 => 8,
    10 => 32,
    x => return Err(bad(format!("xz: an unknown check {x}"))),
  };

  let mut out = vec![];
  let mut at = 12;
  loop {
    let size = match data.get(at) {
      Some(0) => return Ok(out),
      Some(x) => (*x as usize + 1) * 4,
      None => return Err(short()),
    };

    // One filter, LZMA2's, after the sizes the flags name, each a varint.
    let head = data.get(at..at + size).ok_or_else(short)?;
    let mut i = 2;
    for flag in [0x40, 0x80] {
      if head[1] & flag != 0 {
        i += head[i..].iter().position(|x| x & 0x80 == 0).ok_or_else(short)? + 1;
      }
    }

    if head[1] & 3 != 0 || head.get(i) != Some(&0x21) {
      return Err(bad("xz: a filter other than LZMA2"));
    }

    let mut rest = &data[at + size..];
    lzma_rs::lzma2_decompress(&mut rest, &mut out).map_err(|e| bad(format!("xz: {e}")))?;
    at = (data.len() - rest.len()).next_multiple_of(4) + check;
  }
}

/// The payload of the RPM `rpm`, a compressed cpio archive: after the 96-byte lead come two
/// headers, the signature's, padded to 8 bytes, and the package's. A header is its magic, its
/// counts of index entries and of data bytes, big-endian at 8 and 12, then 16 bytes per entry and
/// the data.
fn payload(rpm: &[u8]) -> io::Result<&[u8]> {
  if !rpm.starts_with(b"\xed\xab\xee\xdb") {
    return Err(bad("not an RPM: no lead"));
  }

  let end = |at: usize| -> io::Result<usize> {
    let head = rpm.get(at..at + 16).filter(|x| x.starts_with(b"\x8e\xad\xe8\x01"));
    let head = head.ok_or_else(|| bad("a malformed RPM header"))?;
    let count = |i: usize| u32::from_be_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
    Ok(at + 16 + 16 * count(8) as usize + count(12) as usize)
  };

  let at = end(end(96)?.next_multiple_of(8))?;
  rpm.get(at..).ok_or_else(|| bad("the RPM is cut short"))
}

/// Writes the files of the cpio archive `cpio` that `pkg` keeps into `root`, and gives its symlinks
/// as [unpack] does. The format is "newc", an RPM's: per file a 110-byte header of hex fields, the
/// mode at 14, the link count at 38, the size at 54 and the name's size at 94, then the name and
/// the data, each padded to 4 bytes; a symlink's data is what it points at. The last is
/// `TRAILER!!!`.
fn cpio(cpio: &[u8], pkg: &Package, root: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
  let short = || bad("the cpio archive is cut short");
  let mut links = vec![];
  let mut at = 0;
  loop {
    let head = cpio.get(at..at + 110).ok_or_else(short)?;
    if !head.starts_with(b"070701") {
      return Err(bad("not a cpio archive in the newc format"));
    }

    let field = |from: usize| {
      let text = std::str::from_utf8(&head[from..from + 8]).unwrap_or_default();
      usize::from_str_radix(text, 16).map_err(|_| bad("a malformed cpio header"))
    };

    let (mode, count, size, end) = (field(14)?, field(38)?, field(54)?, at + 110 + field(94)?);
    // The name ends with a zero.
    let name = cpio.get(at + 110..end.saturating_sub(1)).ok_or_else(short)?;
    let name = std::str::from_utf8(name).map_err(|_| bad("a path not in UTF-8"))?;
    let from = end.next_multiple_of(4);
    let data = cpio.get(from..from + size).ok_or_else(short)?;
    at = (from + size).next_multiple_of(4);
    if name == "TRAILER!!!" {
      return Ok(links);
    }

    let Some(rel) = place(Path::new(name), pkg)? else { continue };
    match mode & 0o170000 {
      // A hard link's data is in only one of its entries: none of ours has one.
      0o100000 if count > 1 => return Err(bad(format!("{name}: a hard link"))),
      0o100000 => write(&root.join(rel), data)?,
      0o120000 => {
        let to = std::str::from_utf8(data).map_err(|_| bad("a path not in UTF-8"))?;
        links.extend(resolve(&rel, Path::new(to)).map(|to| (rel, to)));
      }
      _ => {}
    }
  }
}

/// The tarball of the Alpine package `apk`: its gzip streams, the signature's, the control files'
/// and the data's, are one tarball together. A stream is a 10-byte header, its flags at 3, the
/// fields they name, the deflate data and an 8-byte trailer; the pinned SHA-256 makes its CRC moot.
fn gunzip(apk: &[u8]) -> io::Result<Vec<u8>> {
  use miniz_oxide::inflate::stream::{InflateState, inflate};
  use miniz_oxide::{DataFormat, MZFlush, MZStatus};

  let short = || bad("the gzip stream is cut short");
  let mut out = vec![];
  let mut rest = apk;
  while !rest.is_empty() {
    let [0x1f, 0x8b, 8, flags, ..] = *rest else { return Err(bad("not a gzip stream")) };
    rest = rest.get(10..).ok_or_else(short)?;
    // FEXTRA: its size, then its data.
    if flags & 4 != 0 {
      let size = rest.get(..2).map(|x| u16::from_le_bytes([x[0], x[1]]) as usize);
      rest = rest.get(2 + size.ok_or_else(short)?..).ok_or_else(short)?;
    }

    // FNAME and FCOMMENT, each ended by a zero; then FHCRC.
    for flag in [8, 16] {
      if flags & flag != 0 {
        rest = &rest[rest.iter().position(|x| *x == 0).ok_or_else(short)? + 1..];
      }
    }

    if flags & 2 != 0 {
      rest = rest.get(2..).ok_or_else(short)?;
    }

    let mut state = InflateState::new_boxed(DataFormat::Raw);
    loop {
      let at = out.len();
      out.resize(at + (1 << 16), 0);
      let got = inflate(&mut state, rest, &mut out[at..], MZFlush::None);
      out.truncate(at + got.bytes_written);
      rest = &rest[got.bytes_consumed..];
      match got.status {
        Ok(MZStatus::StreamEnd) => break,
        Ok(_) => {}
        Err(e) => return Err(bad(format!("gzip: {e:?}"))),
      }
    }

    rest = rest.get(8..).ok_or_else(short)?;
  }

  Ok(out)
}

/// Writes the files of the tarball `tar` that `pkg` keeps into `root`, and gives its links,
/// symbolic or hard, each as where it goes and what it points at, both in `root`. Symlinks resolve
/// in the sysroot: the packages that have them keep their paths.
fn unpack(tar: &[u8], pkg: &Package, root: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
  let mut links = vec![];
  for entry in tar::Archive::new(tar).entries()? {
    let entry = entry?;
    let Some(rel) = place(&entry.path()?, pkg)? else { continue };
    let to = entry.link_name()?.map(|x| x.into_owned());
    match (entry.header().entry_type(), to) {
      (EntryType::Regular | EntryType::Continuous, _) => write(&root.join(&rel), entry)?,
      (EntryType::Symlink, Some(to)) => links.extend(resolve(&rel, &to).map(|to| (rel, to))),
      // A hard link names a path in the archive.
      (EntryType::Link, Some(to)) => links.extend(place(&to, pkg)?.map(|to| (rel, to))),
      _ => {}
    }
  }

  Ok(links)
}

/// Writes the file at `path` with `data`, making its folder.
fn write(path: &Path, mut data: impl io::Read) -> io::Result<()> {
  if let Some(dir) = path.parent() {
    fs::create_dir_all(dir)?;
  }

  io::copy(&mut data, &mut File::create(path)?).map(drop)
}

/// Where `name`, a path in the archive of `pkg`, goes in the sysroot; `None` when `pkg` doesn't
/// keep it. A path out of the archive is an error.
fn place(name: &Path, pkg: &Package) -> io::Result<Option<PathBuf>> {
  let mut parts = vec![];
  for part in name.components() {
    match part {
      Component::Normal(x) => parts.push(x.to_str().ok_or_else(|| bad("a path not in UTF-8"))?),
      Component::CurDir => {}
      _ => return Err(bad(format!("{}: a path out of the package", name.display()))),
    }
  }

  let path = parts.join("/");
  let Some(rest) = path.strip_prefix(pkg.root) else { return Ok(None) };
  // glibc's charset modules, loaded at run time only: 7 of its 12 MB.
  if !pkg.keep.iter().any(|x| rest.starts_with(x)) || rest.contains("/gconv/") {
    return Ok(None);
  }

  Ok(Some(PathBuf::from(format!("{}{rest}", pkg.into))))
}

/// What the symlink at `link` to `to` names in the sysroot: an absolute `to` from its root, a
/// relative one from the link's folder; `None` when that leaves the sysroot.
fn resolve(link: &Path, to: &Path) -> Option<PathBuf> {
  let mut out = match to.has_root() {
    true => PathBuf::new(),
    false => link.parent()?.to_path_buf(),
  };

  for part in to.components() {
    match part {
      Component::Normal(x) => out.push(x),
      Component::ParentDir if !out.pop() => return None,
      Component::Prefix(_) => return None,
      _ => {}
    }
  }

  Some(out)
}

/// Puts at each of `links` in `root` a hard link to the file it points at, or a copy where the file
/// system has none: Windows makes symlinks only for admins, and Debian's absolute ones would point
/// into the host. A link to a link waits for that one; one to what no package here holds (as
/// libgcc-8-dev's libasan.so) is left out.
fn materialize(root: &Path, mut links: Vec<(PathBuf, PathBuf)>) -> io::Result<()> {
  loop {
    let count = links.len();
    let mut left = vec![];
    for (link, to) in links {
      let (src, dst) = (root.join(&to), root.join(&link));
      match fs::metadata(&src) {
        // No package here has one: a pin that brings one fails here, not in a build.
        Ok(x) if x.is_dir() => return Err(bad(format!("{}: a link to a folder", link.display()))),
        Ok(_) => copy(&src, &dst)?,
        Err(_) => left.push((link, to)),
      }
    }

    if left.is_empty() || left.len() == count {
      return Ok(());
    }

    links = left;
  }
}

/// A hard link at `dst` to the file `src`, or a copy; a file already at `dst` stays.
fn copy(src: &Path, dst: &Path) -> io::Result<()> {
  if let Some(dir) = dst.parent() {
    fs::create_dir_all(dir)?;
  }

  if dst.exists() {
    return Ok(());
  }

  fs::hard_link(src, dst).or_else(|_| fs::copy(src, dst).map(drop))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::toolchain::TARGETS;

  /// A new empty folder for the test `name`.
  fn temp(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("cargo-solo-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
  }

  /// An ar archive of `members`, as `ar` writes it.
  fn ar(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = b"!<arch>\n".to_vec();
    for (name, data) in members {
      let head = format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", 0, 0, 0, 100644, data.len());
      out.extend(head.as_bytes());
      out.extend(*data);
      if data.len() % 2 == 1 {
        out.push(b'\n');
      }
    }

    out
  }

  /// A tarball of `files` (path and data) and `links` (path, target, hard or not).
  fn tarball(files: &[(&str, &str)], links: &[(&str, &str, bool)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(vec![]);
    for (path, data) in files {
      let mut head = tar::Header::new_gnu();
      head.set_size(data.len() as u64);
      head.set_mode(0o644);
      tar.append_data(&mut head, path, data.as_bytes()).unwrap();
    }

    for (path, to, hard) in links {
      let mut head = tar::Header::new_gnu();
      head.set_entry_type(if *hard { EntryType::Link } else { EntryType::Symlink });
      head.set_size(0);
      tar.append_link(&mut head, path, to).unwrap();
    }

    tar.into_inner().unwrap()
  }

  /// An RPM of `payload`: the lead, a signature header of one entry and 5 bytes, padded, and the
  /// header of two entries and 3 bytes.
  fn rpm(payload: &[u8]) -> Vec<u8> {
    let head = |count: u32, size: u32| {
      let mut out = b"\x8e\xad\xe8\x01\0\0\0\0".to_vec();
      out.extend(count.to_be_bytes());
      out.extend(size.to_be_bytes());
      out.resize(out.len() + 16 * count as usize + size as usize, 7);
      out
    };

    let mut out = b"\xed\xab\xee\xdb".to_vec();
    out.resize(96, 0);
    out.extend(head(1, 5));
    out.resize(out.len().next_multiple_of(8), 0);
    out.extend(head(2, 3));
    out.extend(payload);
    out
  }

  /// A cpio archive in the newc format of `entries`: path, mode, link count and data.
  fn newc(entries: &[(&str, u32, u32, &str)]) -> Vec<u8> {
    let mut out = vec![];
    let pad = |x: &mut Vec<u8>| x.resize(x.len().next_multiple_of(4), 0);
    for (name, mode, count, data) in entries.iter().chain([&("TRAILER!!!", 0, 1, "")]) {
      // ino, mode, uid, gid, nlink, mtime, filesize, 4 device numbers, namesize, check
      let size = data.len() as u32;
      let fields = [1, *mode, 0, 0, *count, 0, size, 0, 0, 0, 0, name.len() as u32 + 1, 0];
      out.extend(b"070701");
      out.extend(fields.iter().flat_map(|x| format!("{x:08x}").into_bytes()));
      out.extend(name.as_bytes());
      out.push(0);
      pad(&mut out);
      out.extend(data.as_bytes());
      pad(&mut out);
    }

    out
  }

  /// A gzip stream of `data` with the header fields `flags` names, its trailer zeros.
  fn gzip(data: &[u8], flags: u8) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, flags, 0, 0, 0, 0, 0, 3];
    let fields: [(u8, &[u8]); 4] = [(4, b"\x02\0ab"), (8, b"a.tar\0"), (16, b"hi\0"), (2, b"cc")];
    for (flag, field) in fields {
      if flags & flag != 0 {
        out.extend(field);
      }
    }

    out.extend(miniz_oxide::deflate::compress_to_vec(data, 6));
    out.extend([0; 8]);
    out
  }

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
    let cxx = TARGETS.iter().filter_map(|x| {
      let name = format!("{}-libstdc++", x.clang);
      Some((name.clone(), dir(cache, &name, x.cxx?.packages)))
    });
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
  fn member_finds_a_deb_s_data() {
    let deb = ar(&[("debian-binary", b"2.0\n"), ("control.tar.xz", b"odd"), ("data.tar.xz", b"x")]);
    assert_eq!(member(&deb, "data.tar.xz").unwrap(), b"x");
    assert_eq!(member(&deb, "control.tar.xz").unwrap(), b"odd");
    assert!(member(&deb, "data.tar.zst").is_err());
    // Cut into its data, not only its padding.
    assert!(member(&deb[..deb.len() - 2], "data.tar.xz").is_err());
    assert!(member(b"PK\x03\x04", "data.tar.xz").is_err());
  }

  #[test]
  fn place_keeps_headers_and_libraries() {
    let pkg = Package::deb(BUSTER, "a.deb", "");
    let place = |x: &str| place(Path::new(x), &pkg).unwrap();
    assert_eq!(place("./usr/include/stdio.h"), Some(PathBuf::from("usr/include/stdio.h")));
    assert_eq!(
      place("lib/x86_64-linux-gnu/libc.so.6"),
      Some("lib/x86_64-linux-gnu/libc.so.6".into())
    );
    assert_eq!(place("./usr/share/doc/libc6/copyright"), None);
    assert_eq!(place("./usr/bin/ldd"), None);
    assert_eq!(place("./usr/lib/x86_64-linux-gnu/gconv/UTF-16.so"), None);
    assert_eq!(place("./usr/lib/"), None);
    assert!(super::place(Path::new("./usr/lib/../../etc/passwd"), &pkg).is_err());
    assert!(super::place(Path::new("/usr/lib/libc.so"), &pkg).is_err());

    let mingw = &MINGW_X64[2];
    let got = super::place(&Path::new(mingw.root).join("libgcc_eh.a"), mingw).unwrap();
    assert_eq!(got, Some("usr/x86_64-w64-mingw32/lib/libgcc_eh.a".into()));
    assert_eq!(super::place(&Path::new(mingw.root).join("libmsvcrt.a"), mingw).unwrap(), None);

    let (musl, gcc) = (&MUSL_X64[2], Path::new("./usr/lib/gcc/x86_64-linux-gnu/8"));
    let got = super::place(&gcc.join("crtbeginS.o"), musl).unwrap();
    assert_eq!(got, Some("usr/lib/x86_64-linux-musl/crtbeginS.o".into()));
    assert_eq!(super::place(&gcc.join("libgcc_s.so"), musl).unwrap(), None);
  }

  #[test]
  fn resolve_stays_in_the_sysroot() {
    let resolve = |link: &str, to: &str| resolve(Path::new(link), Path::new(to));
    let libm = resolve("usr/lib/x/libm.so", "/lib/x/libm.so.6");
    assert_eq!(libm, Some("lib/x/libm.so.6".into()));
    assert_eq!(resolve("lib/x/libc.so.6", "libc-2.28.so"), Some("lib/x/libc-2.28.so".into()));
    assert_eq!(resolve("usr/x/include/a.h", "../../share/m/a.h"), Some("usr/share/m/a.h".into()));
    assert_eq!(resolve("usr/a.h", "../../../a.h"), None);
  }

  #[test]
  fn unpack_turns_links_into_files() {
    let root = temp("unpack");
    let files = [
      ("./lib/x/libc-2.28.so", "libc"),
      ("./usr/include/asm-generic/a.h", "a"),
      ("./usr/share/doc/libc6/copyright", "doc"),
    ];
    let links = [
      // Absolute, as Debian's are, relative, a link to a link, a hard link.
      ("./usr/lib/x/libc.so.6", "/lib/x/libc.so.6", false),
      ("./lib/x/libc.so.6", "libc-2.28.so", false),
      ("./usr/include/x/asm/a.h", "../../asm-generic/a.h", false),
      ("./lib/x/libc.hard", "./lib/x/libc-2.28.so", true),
      // To a file no package has, out of the sysroot, to a file not kept.
      ("./usr/lib/x/libasan.so", "../../../x86_64-linux-gnu/libasan.so.5", false),
      ("./usr/lib/x/out", "../../../../etc/passwd", false),
      ("./usr/lib/x/doc", "/usr/share/doc/libc6/copyright", false),
    ];
    let pkg = Package::deb(BUSTER, "a.deb", "");
    let got = unpack(&tarball(&files, &links), &pkg, &root).unwrap();
    assert_eq!(got.len(), 6);
    materialize(&root, got).unwrap();

    let read = |x: &str| fs::read_to_string(root.join(x)).ok();
    assert_eq!(read("lib/x/libc-2.28.so").as_deref(), Some("libc"));
    assert_eq!(read("lib/x/libc.so.6").as_deref(), Some("libc"));
    assert_eq!(read("usr/lib/x/libc.so.6").as_deref(), Some("libc"));
    assert_eq!(read("lib/x/libc.hard").as_deref(), Some("libc"));
    assert_eq!(read("usr/include/x/asm/a.h").as_deref(), Some("a"));
    for x in ["usr/share/doc", "usr/lib/x/libasan.so", "usr/lib/x/out", "usr/lib/x/doc"] {
      assert!(!root.join(x).exists(), "{x}");
    }

    let link = fs::symlink_metadata(root.join("lib/x/libc.so.6")).unwrap();
    assert!(link.is_file());

    let tar = tarball(&[("./usr/lib/x/sub/b.h", "b")], &[("./usr/lib/x/dir", "sub", false)]);
    let got = unpack(&tar, &pkg, &root).unwrap();
    assert!(materialize(&root, got).unwrap_err().to_string().contains("a link to a folder"));
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn xz_skips_the_checks() {
    let mut none = vec![];
    lzma_rs::xz_compress(&mut &b"tar"[..], &mut none).unwrap();
    assert_eq!(xz(&none).unwrap(), b"tar");

    // SHA-256 checks, as AlmaLinux's are, of blocks with sizes, which aren't checked.
    let mut sha = b"\xfd7zXZ\0\0\x0a\0\0\0\0".to_vec();
    for part in [&b"t"[..], b"ar"] {
      sha.extend(b"\x03\xc0\x7f\x81\x01\x21\x01\x16\0\0\0\0\0\0\0\0");
      lzma_rs::lzma2_compress(&mut &part[..], &mut sha).unwrap();
      sha.resize(sha.len().next_multiple_of(4) + 32, 0xaa);
    }

    sha.push(0);
    assert_eq!(xz(&sha).unwrap(), b"tar");
    assert!(xz(&sha[..sha.len() - 1]).is_err());
    // x86's BCJ filter.
    let mut bcj = sha.clone();
    bcj[17] = 0x04;
    assert!(xz(&bcj).unwrap_err().to_string().contains("other than LZMA2"));
    assert!(xz(b"PK\x03\x04").is_err());
  }

  #[test]
  fn payload_skips_an_rpm_s_headers() {
    assert_eq!(payload(&rpm(b"xz")).unwrap(), b"xz");
    assert!(payload(&rpm(b"")[..120]).is_err());
    assert!(payload(b"!<arch>\n").is_err());
  }

  #[test]
  fn cpio_writes_the_files_kept() {
    let root = temp("cpio");
    let keep = &["include/", "lib/libstdc++.a"];
    let pkg = Package { root: "usr/", keep, ..Package::deb(BUSTER, "a.rpm", "") };
    let entries = [
      ("./usr/include", 0o040755, 2, ""),
      ("./usr/include/c++/vector", 0o100644, 1, "vector"),
      ("./usr/include/c++/list", 0o120777, 1, "vector"),
      ("./usr/lib/libstdc++.a", 0o100644, 1, "a"),
      ("./usr/lib/libstdc++.so", 0o100644, 1, "so"),
    ];

    let got = cpio(&newc(&entries), &pkg, &root).unwrap();
    assert_eq!(got, [("include/c++/list".into(), "include/c++/vector".into())]);
    assert_eq!(fs::read_to_string(root.join("include/c++/vector")).unwrap(), "vector");
    assert_eq!(fs::read_to_string(root.join("lib/libstdc++.a")).unwrap(), "a");
    assert!(!root.join("lib/libstdc++.so").exists());

    let hard = newc(&[("./usr/lib/libstdc++.a", 0o100644, 2, "a")]);
    assert!(cpio(&hard, &pkg, &root).unwrap_err().to_string().contains("a hard link"));
    let all = newc(&entries);
    assert!(cpio(&all[..all.len() - 20], &pkg, &root).is_err());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn gunzip_joins_an_apk_s_streams() {
    // Over inflate's chunk.
    let big: Vec<u8> = (0..200_000u32).map(|x| (x * 7 % 251) as u8).collect();
    let apk = [gzip(b"sig", 0), gzip(b"control", 8), gzip(&big, 4 | 8 | 16 | 2)].concat();
    assert_eq!(gunzip(&apk).unwrap(), [&b"sigcontrol"[..], &big].concat());
    assert!(gunzip(&apk[..apk.len() - 4]).is_err());
    assert!(gunzip(b"PK\x03\x04").is_err());
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
