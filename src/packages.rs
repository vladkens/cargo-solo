// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! The sysroots: each target's libc, headers and libraries from Debian's packages (mingw's libgcc
//! from rustup's, Apple's SDK from Zig's source), pinned by SHA-256, fetched by the system's `curl`
//! on first use and unpacked into the cache.

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
  /// The path under each mirror, of a `.deb` or a `.tar.xz`.
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
  Package::deb(
    BUSTER,
    "pool/main/g/gcc-8/libgcc-8-dev_8.3.0-6_amd64.deb",
    "a52d5216858c7185bc245e52ad95a6ad515d087bc25e8556df06c5626b41a837",
  ),
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
  Package::deb(
    BUSTER,
    "pool/main/g/gcc-8/libgcc-8-dev_8.3.0-6_arm64.deb",
    "d8385c0106d4ac559ada75b162aecb9d507a67da3ac8742eb32258f501d07c7e",
  ),
  Package::deb(
    BUSTER,
    "pool/main/g/gcc-8/libgcc1_8.3.0-6_arm64.deb",
    "2851ac25d12958586c035de5ec4f2fc17272dec48f776dd0dd24c62f62674fd9",
  ),
];

/// musl's headers for x86_64, and the kernel's: Rust ships its libc.a and crt objects
/// (`self-contained`), and of the musl it bundles, 1.2.5, Debian's is trixie's.
pub const MUSL_X64: &[Package] = &[
  Package::deb(
    TRIXIE,
    "pool/main/m/musl/musl-dev_1.2.5-3.1~deb13u1_amd64.deb",
    "a3f7d47e452129bb65aae201d6c115abf4d5d3efb136b38b7c6b4fa03ec065e9",
  ),
  LINUX_X64,
];

/// [MUSL_X64] for aarch64.
pub const MUSL_ARM64: &[Package] = &[
  Package::deb(
    TRIXIE,
    "pool/main/m/musl/musl-dev_1.2.5-3.1~deb13u1_arm64.deb",
    "384ea029e3694baac696d3993d93246f9a4b3741c493697e1ceb7256d8820748",
  ),
  LINUX_ARM64,
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
  eprintln!("cargo-solo: fetching {name}'s sysroot into {}", dir.display());
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
        let tar = match file.ends_with(".deb") {
          true => member(&data, "data.tar.xz").and_then(xz),
          false => xz(&data),
        };

        tar.map_err(at)?
      }
    };

    links.extend(unpack(&tar, pkg, root).map_err(at)?);
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

fn xz(data: &[u8]) -> io::Result<Vec<u8>> {
  let mut out = vec![];
  lzma_rs::xz_decompress(&mut &data[..], &mut out).map_err(|e| bad(format!("xz: {e}")))?;
  Ok(out)
}

/// Writes the files of the tarball `tar` that `pkg` keeps into `root`, and gives its links,
/// symbolic or hard, each as where it goes and what it points at, both in `root`. Symlinks resolve
/// in the sysroot: the packages that have them keep their paths.
fn unpack(tar: &[u8], pkg: &Package, root: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
  let mut links = vec![];
  for entry in tar::Archive::new(tar).entries()? {
    let mut entry = entry?;
    let Some(rel) = place(&entry.path()?, pkg)? else { continue };
    let to = entry.link_name()?.map(|x| x.into_owned());
    match (entry.header().entry_type(), to) {
      (EntryType::Regular | EntryType::Continuous, _) => {
        let path = root.join(&rel);
        if let Some(dir) = path.parent() {
          fs::create_dir_all(dir)?;
        }

        io::copy(&mut entry, &mut File::create(&path)?)?;
      }
      (EntryType::Symlink, Some(to)) => links.extend(resolve(&rel, &to).map(|to| (rel, to))),
      // A hard link names a path in the archive.
      (EntryType::Link, Some(to)) => links.extend(place(&to, pkg)?.map(|to| (rel, to))),
      _ => {}
    }
  }

  Ok(links)
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

  #[test]
  fn tables_are_pinned() {
    for pkg in TARGETS.iter().flat_map(|x| x.packages) {
      let hex = pkg.sha256.bytes().all(|x| x.is_ascii_digit() || (b'a'..=b'f').contains(&x));
      assert!(pkg.sha256.len() == 64 && hex, "{}", pkg.path);
      assert!(pkg.path.ends_with(".deb") || pkg.path.ends_with(".tar.xz"), "{}", pkg.path);
      assert!(pkg.mirrors.iter().all(|x| x.starts_with("https://") && !x.ends_with('/')));
      assert!(pkg.root.is_empty() || pkg.root.ends_with('/'));
    }

    // Each sysroot its own folder: targets share one only by its name, as Apple's do.
    let dirs: Vec<_> = TARGETS
      .iter()
      .map(|x| (x.sysroot(), dir(Path::new("/c"), x.sysroot(), x.packages)))
      .collect();
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
  #[ignore = "downloads every package from every mirror, ~160 MB"]
  fn every_package_is_on_every_mirror() {
    let root = temp("mirrors");
    let mut done = vec![];
    for pkg in TARGETS.iter().flat_map(|x| x.packages) {
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
