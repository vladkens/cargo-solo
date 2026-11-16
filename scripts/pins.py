# Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
# /// script
# requires-python = ">=3.11"
# ///
"""Re-pins the packages in src/packages.rs to what their sources have now, in place, and prints
what changed: each Debian package to its release's version (a point release's `+deb13u2`), and
rust-mingw to rustup's stable. `make pins` then fetches every file from every mirror.

A release is a mirror table named after it, `const TRIXIE`, its first mirror Debian's own, and its
package `Package::deb(TRIXIE, "<pool path>", "<sha256>")`, looked up by the name and arch in the
path's file name; a new one can have the empty hash. The release's snapshot.debian.org mirror moves
to the latest snapshot when one of its packages changes, so that it has every one; a snapshot is up
to six hours behind Debian, and until it has a new package, nothing is written.

uv run scripts/pins.py
"""

import lzma
import re
import sys
import time
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src" / "packages.rs"
MANIFEST = "https://static.rust-lang.org/dist/channel-rust-stable.toml"
MINGW = "x86_64-pc-windows-gnu"
TABLE = re.compile(r"const (\w+): &\[&str\] =\s*&\[(.*?)\];", re.S)
DEB = re.compile(r'Package::deb\(\s*(\w+),\s*"([^"]+)",\s*"([0-9a-f]*)",?\s*\)')
SNAPSHOT = re.compile(r"https://snapshot\.debian\.org/archive/([\w-]+)/(\d{8}T\d{6}Z)")
RUST_MINGW = re.compile(
  r'mirrors: RUSTUP,\s*path: "([^"]+)",\s*sha256: "([0-9a-f]*)",\s*root: "([^"]+)"'
)


def log(msg: str) -> None:
  print(f"pins: {msg}", flush=True)


def get(url: str, method: str = "GET"):
  req = urllib.request.Request(url, method=method)
  return urllib.request.urlopen(req, timeout=120)


# MARK: Debian


def index(mirror: str, release: str, arch: str) -> dict[str, list[dict[str, str]]]:
  """The fields of each package in `release`'s main for `arch`, by name: one each, but for a few
  of buster's `all` ones, listed in two versions."""
  url = f"{mirror}/dists/{release}/main/binary-{arch}/Packages.xz"
  log(f"reading {url}")
  rs = {}
  for block in lzma.decompress(get(url).read()).decode().split("\n\n"):
    pld = dict(re.findall(r"^([\w-]+): (.*)$", block, re.M))
    if "Package" in pld:
      rs.setdefault(pld["Package"], []).append(pld)

  return rs


def upstream(version: str) -> str:
  """The version without Debian's revision: `1.2.5` of `1.2.5-3.1~deb13u1`."""
  return version.rsplit("-", 1)[0]


def latest(archive: str) -> str:
  """The latest snapshot of `archive`: snapshot.debian.org redirects a time to the one before it."""
  now = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
  url = get(f"https://snapshot.debian.org/archive/{archive}/{now}/", "HEAD").geturl()
  return SNAPSHOT.search(url).group(2)


def debian(src: str) -> str:
  """`src` with each Debian package re-pinned, and the snapshot of each release that changed."""
  tables = {name: re.findall(r'"([^"]+)"', body) for name, body in TABLE.findall(src)}
  indexes = {}
  changed = {}

  def repin(m: re.Match) -> str:
    table, path, sha = m.groups()
    name, version, arch = Path(path).stem.split("_")
    key = (table, "amd64" if arch == "all" else arch)
    if key not in indexes:
      indexes[key] = index(tables[table][0], table.lower(), key[1])

    found = indexes[key].get(name, [])
    if len(found) != 1:
      versions = ", ".join(x["Version"] for x in found) or "none"
      sys.exit(f"pins: {name} in {table.lower()} for {key[1]}: {versions}, pin it by hand")

    pkg = found[0]
    if (pkg["Filename"], pkg["SHA256"]) == (path, sha):
      return m[0]

    new = Path(pkg["Filename"]).stem.split("_")[1]
    log(f"{name} {arch}: {version} -> {new}" if new != version else f"{name} {arch}: hash")
    if upstream(version) != upstream(new):
      log(f"{name}: a new upstream version, check Libc::version and readme.md")

    changed.setdefault(table, []).append(pkg["Filename"])
    return m[0].replace(path, pkg["Filename"]).replace(f'"{sha}"', f'"{pkg["SHA256"]}"')

  src = DEB.sub(repin, src)
  for m in TABLE.finditer(src):
    snap = SNAPSHOT.search(m[0])
    if m[1] not in changed or not snap:
      continue

    now = latest(snap[1])
    log(f"{m[1]}: snapshot {snap[2]} -> {now}")
    # Nothing is written until the snapshot has them, so that the next run tries again.
    mirror = next(x for x in tables[m[1]] if SNAPSHOT.search(x)).replace(snap[2], now)
    for path in changed[m[1]]:
      try:
        get(f"{mirror}/{path}", "HEAD")
      except urllib.error.HTTPError as e:
        sys.exit(f"pins: {path} on snapshot {now}: {e}; it's behind Debian, run again later")

    src = src.replace(m[0], m[0].replace(snap[2], now))

  return src


# MARK: Rustup


def rustup(src: str) -> str:
  """`src` with rust-mingw re-pinned to rustup's stable."""
  m = RUST_MINGW.search(src)
  path, sha, root = m.groups()
  log(f"reading {MANIFEST}")
  pkg = tomllib.loads(get(MANIFEST).read().decode())["pkg"]["rust-mingw"]["target"][MINGW]
  new = pkg["xz_url"].removeprefix("https://static.rust-lang.org/")
  if (new, pkg["xz_hash"]) == (path, sha):
    return src

  log(f"rust-mingw: {path} -> {new}")
  # The root starts with the archive's top folder, its file's name.
  top = Path(new).name.removesuffix(".tar.xz")
  block = m[0].replace(path, new).replace(f'"{sha}"', f'"{pkg["xz_hash"]}"')
  block = block.replace(f'"{root}"', f'"{top}/{root.split("/", 1)[1]}"')
  return src.replace(m[0], block)


def main() -> None:
  src = SRC.read_text()
  # The tables' block, where the tests' packages aren't.
  start = src.index("// MARK: Packages")
  end = src.index("// MARK:", start + 1)
  new = src[:start] + rustup(debian(src[start:end])) + src[end:]
  if new == src:
    log("all current")
    return

  SRC.write_text(new)
  log(f"rewrote {SRC.relative_to(ROOT)}")


if __name__ == "__main__":
  main()
