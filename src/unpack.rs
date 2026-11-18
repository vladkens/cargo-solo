// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! Unpacks the distros' packages: a `.deb`'s ar and xz, an RPM's cpio, an Alpine package's gzip
//! streams, and the tarball inside, its links turned into files.

use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use tar::EntryType;

use crate::bad;

/// The member `name` of a `.deb`'s ar archive: `!<arch>\n`, then per member a 60-byte header
/// (name at 0..16, size at 48..58) and its data, padded to an even size.
pub fn member<'a>(ar: &'a [u8], name: &str) -> io::Result<&'a [u8]> {
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

/// An xz stream's data, block by block through LZMA2: lzma-rs can't check SHA-256, which
/// AlmaLinux's use, and the pinned SHA-256 covers it. A 12-byte header (check kind at 7), blocks of
/// `(first byte + 1) * 4` header bytes, data and check, then a zero.
pub fn xz(data: &[u8]) -> io::Result<Vec<u8>> {
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

/// An RPM's payload, a compressed cpio archive, after the 96-byte lead and two headers, the first
/// padded to 8: 16 bytes with the entry and data counts at 8 and 12, then 16 per entry and the
/// data.
pub fn payload(rpm: &[u8]) -> io::Result<&[u8]> {
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

/// [unpack] for an RPM's cpio archive (newc): per file a 110-byte header of hex fields (mode at 14,
/// links at 38, size at 54, name size at 94), the name and the data, each padded to 4; a symlink's
/// data is its target. The last is `TRAILER!!!`.
pub fn cpio(
  cpio: &[u8],
  keep: impl Fn(&str) -> Option<PathBuf>,
  root: &Path,
) -> io::Result<Vec<(PathBuf, PathBuf)>> {
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

    let Some(rel) = keep(&relative(Path::new(name))?) else { continue };
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

/// An Alpine package's tarball: its gzip streams (signature, control, data) joined. Per stream a
/// 10-byte header with the flags at 3, the fields they name, the deflate data, an 8-byte trailer.
pub fn gunzip(apk: &[u8]) -> io::Result<Vec<u8>> {
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

/// Writes the files of `tar` that `keep` places into `root`, and gives its links, symbolic or hard,
/// for [materialize]. Symlinks resolve in the sysroot: packages with them keep their paths.
pub fn unpack(
  tar: &[u8],
  keep: impl Fn(&str) -> Option<PathBuf>,
  root: &Path,
) -> io::Result<Vec<(PathBuf, PathBuf)>> {
  let mut links = vec![];
  for entry in tar::Archive::new(tar).entries()? {
    let entry = entry?;
    let Some(rel) = keep(&relative(&entry.path()?)?) else { continue };
    let to = entry.link_name()?.map(|x| x.into_owned());
    match (entry.header().entry_type(), to) {
      (EntryType::Regular | EntryType::Continuous, _) => write(&root.join(&rel), entry)?,
      (EntryType::Symlink, Some(to)) => links.extend(resolve(&rel, &to).map(|to| (rel, to))),
      // A hard link names a path in the archive.
      (EntryType::Link, Some(to)) => links.extend(keep(&relative(&to)?).map(|to| (rel, to))),
      _ => {}
    }
  }

  Ok(links)
}

fn write(path: &Path, mut data: impl io::Read) -> io::Result<()> {
  if let Some(dir) = path.parent() {
    fs::create_dir_all(dir)?;
  }

  io::copy(&mut data, &mut File::create(path)?).map(drop)
}

/// `name`, a path in an archive, as `a/b`; one out of the archive is an error.
fn relative(name: &Path) -> io::Result<String> {
  let mut parts = vec![];
  for part in name.components() {
    match part {
      Component::Normal(x) => parts.push(x.to_str().ok_or_else(|| bad("a path not in UTF-8"))?),
      Component::CurDir => {}
      _ => return Err(bad(format!("{}: a path out of the package", name.display()))),
    }
  }

  Ok(parts.join("/"))
}

/// The file the symlink at `link` to `to` names in the sysroot; `None` out of it.
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

/// Turns `links` in `root` into files: Windows makes symlinks only for admins, and Debian's
/// absolute ones would point into the host. A link to a link waits for it; one to a file no package
/// has (as libgcc-8-dev's libasan.so) is left out.
pub fn materialize(root: &Path, mut links: Vec<(PathBuf, PathBuf)>) -> io::Result<()> {
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

/// A hard link, or a copy where the file system has none; a file already at `dst` stays.
pub fn copy(src: &Path, dst: &Path) -> io::Result<()> {
  if let Some(dir) = dst.parent() {
    fs::create_dir_all(dir)?;
  }

  if dst.exists() {
    return Ok(());
  }

  fs::hard_link(src, dst).or_else(|_| fs::copy(src, dst).map(drop))
}

#[cfg(test)]
pub mod tests {
  use std::env;

  use super::*;

  /// A new empty folder for the test `name`.
  pub fn temp(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("cargo-solo-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
  }

  /// An ar archive of `members`, as `ar` writes it.
  pub fn ar(members: &[(&str, &[u8])]) -> Vec<u8> {
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
  pub fn tarball(files: &[(&str, &str)], links: &[(&str, &str, bool)]) -> Vec<u8> {
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
  fn relative_stays_in_the_archive() {
    let relative = |x: &str| relative(Path::new(x)).ok();
    assert_eq!(relative("./usr/include/stdio.h").as_deref(), Some("usr/include/stdio.h"));
    assert_eq!(relative("usr/lib/").as_deref(), Some("usr/lib"));
    assert_eq!(relative("./usr/lib/../../etc/passwd"), None);
    assert_eq!(relative("/usr/lib/libc.so"), None);
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
    let keep = |x: &str| (!x.starts_with("usr/share/")).then(|| PathBuf::from(x));
    let got = unpack(&tarball(&files, &links), keep, &root).unwrap();
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
    let got = unpack(&tar, keep, &root).unwrap();
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
    let keep = |x: &str| {
      let x = x.strip_prefix("usr/")?;
      ["include/", "lib/libstdc++.a"].iter().any(|k| x.starts_with(k)).then(|| PathBuf::from(x))
    };
    let entries = [
      ("./usr/include", 0o040755, 2, ""),
      ("./usr/include/c++/vector", 0o100644, 1, "vector"),
      ("./usr/include/c++/list", 0o120777, 1, "vector"),
      ("./usr/lib/libstdc++.a", 0o100644, 1, "a"),
      ("./usr/lib/libstdc++.so", 0o100644, 1, "so"),
    ];

    let got = cpio(&newc(&entries), keep, &root).unwrap();
    assert_eq!(got, [("include/c++/list".into(), "include/c++/vector".into())]);
    assert_eq!(fs::read_to_string(root.join("include/c++/vector")).unwrap(), "vector");
    assert_eq!(fs::read_to_string(root.join("lib/libstdc++.a")).unwrap(), "a");
    assert!(!root.join("lib/libstdc++.so").exists());

    let hard = newc(&[("./usr/lib/libstdc++.a", 0o100644, 2, "a")]);
    assert!(cpio(&hard, keep, &root).unwrap_err().to_string().contains("a hard link"));
    let all = newc(&entries);
    assert!(cpio(&all[..all.len() - 20], keep, &root).is_err());
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
}
