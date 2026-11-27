# Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
# /// script
# requires-python = ">=3.11"
# ///
"""Re-pins the packages in src/packages.rs to what Debian, AlmaLinux, Alpine, rustup and Zig have
now (`make pins`).

uv run scripts/pins.py

A Debian package is `Package::deb(TRIXIE, "<pool path>", "<sha256>")`: its release is the mirror
table's name, its name and arch are in the file name; a new one can have the empty hash. A
release's snapshot.debian.org mirror moves to the latest snapshot when one of its packages
changes. A snapshot lags Debian by up to six hours: until it has the new packages, nothing is
written. Zig's source tarball, Apple's SDK, moves to Zig's latest release. LLVM's libc++ stays at
20.1.8, by hand: its headers take clang 18, 22's need clang 20. objc2's crates, Swift's
CoreFoundation and configd are bumped by hand too: `sdk` fails on a header its patches don't fit.

An AlmaLinux or Alpine package is `mirrors: <table>, path: "<arch>/<path>", sha256: "<sha256>"`:
its name is in the file name. AlmaLinux's is its latest build in the repodata, with its SHA-256;
Alpine's its version in APKINDEX, hashed by download, for the index has only SHA-1.
"""

import gzip
import hashlib
import io
import json
import lzma
import re
import sys
import tarfile
import time
import tomllib
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src" / "packages.rs"
MANIFEST = "https://static.rust-lang.org/dist/channel-rust-stable.toml"
MINGW = "x86_64-pc-windows-gnu"
ZIG_INDEX = "https://ziglang.org/download/index.json"

TABLE = re.compile(r"const (\w+): &\[&str\] =\s*&\[(.*?)\];", re.S)
DEB = re.compile(r'Package::deb\(\s*(\w+),\s*"([^"]+)",\s*"([0-9a-f]*)",?\s*\)')
SNAPSHOT = re.compile(r"https://snapshot\.debian\.org/archive/([\w-]+)/(\d{8}T\d{6}Z)")
RUST_MINGW = re.compile(
    r'mirrors: RUSTUP,\s*path: "([^"]+)",\s*sha256: "([0-9a-f]*)",\s*root: "([^"]+)"'
)
ZIG = re.compile(r'path: "zig-([\d.]+)\.tar\.xz",\s*sha256: "([0-9a-f]*)"')
RPM_APK = re.compile(r'mirrors: (\w+),\s*path: "([^"]+\.(?:rpm|apk))",\s*sha256: "([0-9a-f]*)"')
REPO = "{http://linux.duke.edu/metadata/repo}"
COMMON = "{http://linux.duke.edu/metadata/common}"


def log(msg: str):
    print(f"pins: {msg}", flush=True)


def get(url: str, method="GET"):
    req = urllib.request.Request(url, method=method)
    return urllib.request.urlopen(req, timeout=120)


# MARK: Debian


def index(mirror: str, release: str, arch: str) -> dict[str, list[dict]]:
    url = f"{mirror}/dists/{release}/main/binary-{arch}/Packages.xz"
    log(f"reading {url}")
    text = lzma.decompress(get(url).read()).decode()

    # a list: buster has a few `all` packages in two versions
    rs = {}
    for block in text.split("\n\n"):
        pld = dict(re.findall(r"^([\w-]+): (.*)$", block, re.M))
        if "Package" in pld:
            rs.setdefault(pld["Package"], []).append(pld)

    return rs


def latest(archive: str) -> str:
    # snapshot.debian.org redirects a time to the snapshot before it
    now = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    url = get(f"https://snapshot.debian.org/archive/{archive}/{now}/", "HEAD").geturl()
    return SNAPSHOT.search(url)[2]


def debian(src: str) -> str:
    tables = {name: re.findall(r'"([^"]+)"', body) for name, body in TABLE.findall(src)}
    indexes = {}
    changed = {}  # table -> new paths

    def repin(m: re.Match) -> str:
        table, path, sha = m.groups()
        name, ver, arch = Path(path).stem.split("_")
        key = (table, "amd64" if arch == "all" else arch)
        if key not in indexes:
            indexes[key] = index(tables[table][0], table.lower(), key[1])

        found = indexes[key].get(name, [])
        if len(found) != 1:
            vers = ", ".join(x["Version"] for x in found) or "none"
            sys.exit(f"pins: {name} in {table.lower()} for {key[1]}: {vers}, pin it by hand")

        new_path, new_sha = found[0]["Filename"], found[0]["SHA256"]
        if (new_path, new_sha) == (path, sha):
            return m[0]

        new_ver = Path(new_path).stem.split("_")[1]
        log(f"{name} {arch}: {ver} -> {new_ver}" if new_ver != ver else f"{name} {arch}: hash")
        if ver.rsplit("-", 1)[0] != new_ver.rsplit("-", 1)[0]:
            log(f"{name}: new upstream version, check Libc::version and readme.md")

        changed.setdefault(table, []).append(new_path)
        return m[0].replace(path, new_path).replace(f'"{sha}"', f'"{new_sha}"')

    src = DEB.sub(repin, src)

    for m in TABLE.finditer(src):
        snap = SNAPSHOT.search(m[0])
        if m[1] not in changed or not snap:
            continue

        ts = latest(snap[1])
        log(f"{m[1]}: snapshot {snap[2]} -> {ts}")

        # nothing is written until the snapshot has them, so the next run tries again
        mirror = next(x for x in tables[m[1]] if SNAPSHOT.search(x)).replace(snap[2], ts)
        for path in changed[m[1]]:
            try:
                get(f"{mirror}/{path}", "HEAD")
            except urllib.error.HTTPError as e:
                sys.exit(f"pins: {path} on snapshot {ts}: {e}, run again later")

        src = src.replace(m[0], m[0].replace(snap[2], ts))

    return src


# MARK: AlmaLinux, Alpine


def alma(mirror: str, arch: str) -> dict[str, tuple[int, str, str]]:
    """Each package's latest build in AlmaLinux's repo at `mirror` for `arch`: its build time, its
    path from the mirror and its SHA-256."""
    base = f"{mirror}/{arch}/os"
    log(f"reading {base}/repodata/repomd.xml")
    repomd = ET.fromstring(get(f"{base}/repodata/repomd.xml").read())
    href = repomd.find(f"{REPO}data[@type='primary']/{REPO}location").get("href")
    log(f"reading {base}/{href}")
    data = get(f"{base}/{href}").read()
    xml = gzip.decompress(data) if href.endswith(".gz") else lzma.decompress(data)

    rs = {}
    for _, pkg in ET.iterparse(io.BytesIO(xml)):
        if pkg.tag != f"{COMMON}package":
            continue

        name, check = pkg.findtext(f"{COMMON}name"), pkg.find(f"{COMMON}checksum")
        build = int(pkg.find(f"{COMMON}time").get("build"))
        path = f"{arch}/os/{pkg.find(f'{COMMON}location').get('href')}"
        # x86_64's repo has i686 packages too
        ours = pkg.findtext(f"{COMMON}arch") in (arch, "noarch") and check.get("type") == "sha256"
        if ours and build >= rs.get(name, (0,))[0]:
            rs[name] = (build, path, check.text)

        pkg.clear()

    return rs


def alpine(mirror: str, arch: str) -> dict[str, str]:
    """Each package's version in Alpine's repo at `mirror` for `arch`."""
    url = f"{mirror}/{arch}/APKINDEX.tar.gz"
    log(f"reading {url}")
    with tarfile.open(fileobj=io.BytesIO(get(url).read())) as tar:
        text = tar.extractfile("APKINDEX").read().decode()

    rs = {}
    for block in text.split("\n\n"):
        pld = dict(x.split(":", 1) for x in block.splitlines() if x[1:2] == ":")
        if "P" in pld:
            rs[pld["P"]] = pld["V"]

    return rs


def others(src: str) -> str:
    tables = {name: re.findall(r'"([^"]+)"', body) for name, body in TABLE.findall(src)}
    indexes = {}

    def repin(m: re.Match) -> str:
        table, path, sha = m.groups()
        arch, file = path.split("/")[0], Path(path).name
        rpm = file.endswith(".rpm")
        # name-version-release, and an RPM's arch
        name = file.removesuffix(".apk").removesuffix(f".{arch}.rpm").rsplit("-", 2)[0]
        mirror = tables[table][0]
        key = (table, arch)
        if key not in indexes:
            indexes[key] = alma(mirror, arch) if rpm else alpine(mirror, arch)

        found = indexes[key].get(name)
        if not found:
            sys.exit(f"pins: no {name} in {table.lower()} for {arch}, pin it by hand")

        if rpm:
            _, new_path, new_sha = found
        else:
            new_path = f"{arch}/{name}-{found}.apk"
            new_sha = sha
            if new_path != path or not sha:
                log(f"hashing {mirror}/{new_path}")
                new_sha = hashlib.sha256(get(f"{mirror}/{new_path}").read()).hexdigest()

        if (new_path, new_sha) == (path, sha):
            return m[0]

        new = Path(new_path).name
        log(f"{name} {arch}: {file} -> {new}" if new != file else f"{name} {arch}: hash")
        return m[0].replace(path, new_path).replace(f'"{sha}"', f'"{new_sha}"')

    return RPM_APK.sub(repin, src)


# MARK: Rustup


def rustup(src: str) -> str:
    m = RUST_MINGW.search(src)
    path, sha, root = m.groups()

    log(f"reading {MANIFEST}")
    rep = tomllib.loads(get(MANIFEST).read().decode())
    pkg = rep["pkg"]["rust-mingw"]["target"][MINGW]
    new_path, new_sha = pkg["xz_url"].removeprefix("https://static.rust-lang.org/"), pkg["xz_hash"]
    if (new_path, new_sha) == (path, sha):
        return src

    log(f"rust-mingw: {path} -> {new_path}")
    # the root starts with the archive's top folder, named as the file
    top = Path(new_path).name.removesuffix(".tar.xz")
    new_root = f"{top}/{root.split('/', 1)[1]}"

    block = m[0].replace(path, new_path).replace(f'"{sha}"', f'"{new_sha}"')
    block = block.replace(f'"{root}"', f'"{new_root}"')
    return src.replace(m[0], block)


# MARK: Zig


def zig(src: str) -> str:
    ver, sha = ZIG.search(src).groups()

    log(f"reading {ZIG_INDEX}")
    rels = json.loads(get(ZIG_INDEX).read())
    vers = [x for x in rels if re.fullmatch(r"\d+\.\d+\.\d+", x)]
    new = max(vers, key=lambda x: tuple(map(int, x.split("."))))
    tarball, new_sha = rels[new]["src"]["tarball"], rels[new]["src"]["shasum"]
    if (new, new_sha) == (ver, sha):
        return src

    if tarball != f"https://ziglang.org/download/{new}/zig-{new}.tar.xz":
        sys.exit(f"pins: zig {new}'s source is {tarball}, pin it by hand")

    log(f"zig: {ver} -> {new}, check Libc::version's SDK and readme.md")
    # the mirror, the file, and the roots, which start with the archive's top folder
    src = src.replace(f'/download/{ver}"', f'/download/{new}"').replace(f"zig-{ver}", f"zig-{new}")
    return src.replace(f'"{sha}"', f'"{new_sha}"')


def main():
    src = SRC.read_text()

    # only the tables' block: the tests have packages too
    start = src.index("// MARK: Packages")
    end = src.index("// MARK:", start + 1)
    new = src[:start] + others(zig(rustup(debian(src[start:end])))) + src[end:]
    if new == src:
        log("all current")
        return

    SRC.write_text(new)
    log(f"rewrote {SRC.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
