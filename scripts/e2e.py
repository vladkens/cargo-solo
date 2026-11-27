# Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
# /// script
# requires-python = ">=3.11"
# ///
"""The end-to-end test: the test crates, tests/hello (C through cc and cmake) and tests/cxx (C++
through both), built by cargo-solo for every target it sets up, then run in each target's OS.

uv run scripts/e2e.py build <dir>   every target's binaries, <dir>/<target>/<name>[.exe], checked
uv run scripts/e2e.py run <dir>     each binary under <dir> this host can run, its output checked
uv run scripts/e2e.py all <dir>     the two on this host (`make e2e`)

`build` checks each binary's file: `-linux-gnu` needs no glibc symbol over 2.28 and no shared
libstdc++, `-linux-musl` is static, `-windows-gnu` imports only Windows' own DLLs, `-apple-darwin`
loads only the system's dylibs and runs on the oldest macOS cargo-solo builds for. `run` takes any
host's binaries, found anywhere under <dir>: Linux ones in Docker (Debian 10 for glibc, Alpine for
musl; the other arch through QEMU), Windows ones on Windows, Apple ones on a Mac (x86_64 through
Rosetta). CI builds on each OS and runs on each (.github/workflows/main.yml).

With E2E_FULL=true, as CI on main and release PRs: release builds, and tests/hello with aws-lc-rs.
"""

import argparse
import hashlib
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if sys.platform == "win32" else ""
GLIBC = (2, 28)
# What a Windows binary may import: the system's DLLs, never MinGW's runtime (libgcc_s, winpthread).
SYSTEM_DLL = re.compile(
    r"(?i)^(kernel32|msvcrt|ntdll|userenv|ws2_32|advapi32|bcrypt|bcryptprimitives"
    r"|api-ms-win-.+)\.dll$"
)
# The oldest macOS cargo-solo builds for, rustc's or aligned `new`'s, which Apple's binaries must not
# raise.
MACOS = {"aarch64": (11, 0), "x86_64": (10, 13)}
SHA = hashlib.sha256(b"cargo-solo").hexdigest()[:16]
FULL = os.environ.get("E2E_FULL") == "true"
PROFILE = "release" if FULL else "debug"
# Each test crate's folder in tests/: its binary, the line it prints and its cargo args.
CRATES = {
    "hello": ("hello", f"hello 10 {SHA}" + f" {SHA}" * FULL, ["--features", "aws-lc-rs"] * FULL),
    "cxx": ("hello-cxx", "hello-cxx caught 10 0.50 48", []),
}
EXPECTED = {name: line for name, line, _ in CRATES.values()}


def log(msg: str) -> None:
    print(f"e2e: {msg}", flush=True)


def run(*args, **kw) -> str:
    """Runs a command to its end, its stdout given back; its stderr passes through."""
    args = [str(a) for a in args]
    log(" ".join(args))
    return subprocess.run(args, check=True, stdout=subprocess.PIPE, text=True, **kw).stdout


def start(*args, **kw) -> subprocess.Popen:
    """Starts a command beside this script's, its output passing through."""
    args = [str(a) for a in args]
    log(" ".join(args))
    return subprocess.Popen(args, **kw)


def wait(*procs: subprocess.Popen) -> None:
    """Waits for each of `procs`; fails when one did."""
    for proc in procs:
        if proc.wait():
            sys.exit(f"e2e: {' '.join(proc.args)} failed ({proc.returncode})")


# MARK: Build


def readobj() -> Path:
    """rustup's llvm-readobj, which cargo-solo's build has made sure of (llvm-tools)."""
    info = run("rustc", "-vV")
    host = next(ln.split()[1] for ln in info.splitlines() if ln.startswith("host:"))
    sysroot = Path(run("rustc", "--print", "sysroot").strip())
    return sysroot / "lib" / "rustlib" / host / "bin" / f"llvm-readobj{EXE}"


def check(target: str, path: Path, tool: Path) -> None:
    """Fails unless the binary at `path` runs on its OS's oldest system cargo-solo promises."""
    if target.endswith("-linux-gnu"):
        syms = run(tool, "--dyn-syms", path)
        found = {tuple(int(x) for x in v.split(".")) for v in re.findall(r"GLIBC_([0-9.]+)", syms)}
        top = max(found)
        if top > GLIBC:
            sys.exit(f"e2e: {target} needs glibc {'.'.join(map(str, top))}")

        if "libstdc++" in run(tool, "--needed-libs", path):
            sys.exit(f"e2e: {target} needs the shared libstdc++")

        log(f"{target}: glibc {'.'.join(map(str, top))}")
    elif target.endswith("-linux-musl"):
        if "PT_INTERP" in run(tool, "--program-headers", path):
            sys.exit(f"e2e: {target} is not static")

        log(f"{target}: static")
    elif target.endswith("-windows-gnu"):
        dlls = re.findall(r"Name: (\S+\.dll)", run(tool, "--coff-imports", path), re.I)
        other = sorted({x for x in dlls if not SYSTEM_DLL.match(x)})
        if not dlls or other:
            sys.exit(f"e2e: {target} imports {other or 'nothing'}")

        log(f"{target}: imports {', '.join(sorted(set(dlls)))}")
    elif target.endswith("-apple-darwin"):
        text = run(tool, "--needed-libs", "--macho-version-min", path)
        libs = re.search(r"NeededLibraries \[\n(.*?)\n\]", text, re.S)[1].split()
        ver = tuple(int(x) for x in re.search(r"Version: ([0-9.]+)", text)[1].split("."))
        other = [x for x in libs if not x.startswith(("/usr/lib/", "/System/Library/Frameworks/"))]
        if not libs or other or ver > MACOS[target.split("-")[0]]:
            sys.exit(f"e2e: {target} needs macOS {ver} and loads {libs}")

        log(f"{target}: macOS {'.'.join(map(str, ver))}, loads {', '.join(libs)}")


def build(out: Path) -> None:
    """Each test crate for every target, built side by side, copied to <out>/<target>/."""
    # rustup's downloads beside cargo's, rather than one by one in the first build
    tools = start("rustup", "component", "add", "llvm-tools")
    run("cargo", "build", cwd=ROOT)
    solo = ROOT / "target" / "debug" / f"cargo-solo{EXE}"
    text = run(solo, "--help")
    targets = re.findall(r"^  ([a-z0-9_]+-[a-z0-9_-]+) {2,}", text, re.M)
    if not targets:
        sys.exit("e2e: no targets in `cargo solo --help`")

    fetch = [start("cargo", "fetch", cwd=ROOT / "tests" / x) for x in CRATES]
    wait(tools)
    run("rustup", "target", "add", *targets)
    wait(*fetch)

    builds = []
    for crate, (name, _, extra) in CRATES.items():
        args = [x for t in targets for x in ("--target", t)]
        dest = ROOT / "target" / crate
        # --keep-going: every target that fails, not only the first
        cmd = [solo, "solo", "build", "--keep-going", "--target-dir", dest, *args, *extra]
        proc = start(*cmd, "--profile", "release" if FULL else "dev", cwd=ROOT / "tests" / crate)
        builds.append((name, dest, proc))

    wait(*(proc for *_, proc in builds))
    tool = readobj()
    for name, dest, _ in builds:
        for target in targets:
            file = f"{name}.exe" if "-windows-" in target else name
            (out / target).mkdir(parents=True, exist_ok=True)
            path = shutil.copy2(dest / target / PROFILE / file, out / target / file)
            check(target, Path(path), tool)


# MARK: Run


def command(target: str, path: Path) -> list[str] | None:
    """How this host runs the binary of `target` at `path`, or None when it can't."""
    if "-linux-" in target and sys.platform != "win32" and shutil.which("docker"):
        arch = "amd64" if target.startswith("x86_64-") else "arm64"
        image = "debian:buster" if target.endswith("-gnu") else "alpine"
        mount = f"{path.parent}:/b:ro"
        docker = ["docker", "run", "--rm", "-q", "--platform", f"linux/{arch}", "-v", mount]
        return [*docker, image, f"/b/{path.name}"]

    if "-windows-" in target and sys.platform == "win32":
        return [str(path)]

    if "-apple-" in target and sys.platform == "darwin":
        if not target.startswith("x86_64-") or platform.machine() == "x86_64":
            return [str(path)]

        # Rosetta, when this Mac has it
        rosetta = subprocess.run(["arch", "-x86_64", "/usr/bin/true"], capture_output=True)
        if rosetta.returncode == 0:
            return ["arch", "-x86_64", str(path)]

    return None


def check_runs(out: Path) -> None:
    """Each binary under <out> this host can run prints its line; fails when none could run."""
    ran = 0
    for path in sorted(x for x in out.rglob("*") if x.name.removesuffix(".exe") in EXPECTED):
        target = path.parent.name
        # An artifact downloaded in CI loses the executable bit.
        if sys.platform != "win32":
            path.chmod(path.stat().st_mode | 0o111)

        cmd = command(target, path)
        if cmd is None:
            log(f"{path.relative_to(out)}: not on this host")
            continue

        got, want = run(*cmd).strip(), EXPECTED[path.name.removesuffix(".exe")]
        if got != want:
            sys.exit(f"e2e: {path.relative_to(out)} printed {got!r}, not {want!r}")

        log(f"{path.relative_to(out)}: {got}")
        ran += 1

    if not ran:
        sys.exit(f"e2e: nothing under {out} runs on this host")


def main() -> None:
    fmt = argparse.RawDescriptionHelpFormatter
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=fmt)
    ap.add_argument("step", choices=("build", "run", "all"))
    ap.add_argument("dir", type=Path)
    args = ap.parse_args()
    out = args.dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    steps = {"build": build, "run": check_runs}
    for step in steps if args.step == "all" else [args.step]:
        steps[step](out)


if __name__ == "__main__":
    main()
