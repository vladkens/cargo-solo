# /// script
# requires-python = ">=3.11"
# ///
"""Popular crates with C and C++ inside, built by cargo-solo on every host for every target, then
run in each target's OS (.github/workflows/crates.yml).

uv run tests/crates/crates.py collect <host> <target> <out>   after a build: what built, its binaries
uv run tests/crates/crates.py run <out>                        each binary this host can run
uv run tests/crates/crates.py report <out>                     the table, from every host's results
"""

import json
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CRATES = sorted(x.name for x in HERE.glob("c-*"))
TARGETS = {
    "x86_64-unknown-linux-gnu": "gnu x64",
    "aarch64-unknown-linux-gnu": "gnu arm64",
    "x86_64-unknown-linux-musl": "musl x64",
    "aarch64-unknown-linux-musl": "musl arm64",
    "x86_64-pc-windows-gnu": "windows",
    "aarch64-apple-darwin": "mac arm64",
    "x86_64-apple-darwin": "mac x64",
}
FAILED = re.compile(
    r"^error: (?:failed to run custom build command for|could not compile) `([^`]+)`", re.M
)


def collect(host: str, target: str, out: Path) -> None:
    log = (HERE / "build.log").read_text(errors="replace")
    exe = ".exe" if "-windows-" in target else ""
    dest = out / "bin" / host / target
    crates = {}
    for name in CRATES:
        path = HERE / "target" / target / "release" / f"{name}{exe}"
        crates[name] = path.is_file()
        if path.is_file() and not host.startswith("native"):
            dest.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, dest / path.name)

    errors = sorted(set(FAILED.findall(log)))
    res = {"host": host, "target": target, "crates": crates, "errors": errors}
    (out / "res").mkdir(parents=True, exist_ok=True)
    (out / "res" / f"build-{host}-{target}.json").write_text(json.dumps(res, indent=2))
    print(json.dumps(res, indent=2))


def runs(target: str, dir: Path) -> list[tuple[str, int, str]]:
    """(crate, exit code, output) of each binary in `dir`, or [] when this host can't run them."""
    files = sorted(x for x in dir.iterdir() if x.is_file())
    if sys.platform != "win32":
        for x in files:
            x.chmod(x.stat().st_mode | 0o111)

    def one(cmd: list[str]) -> tuple[int, str]:
        try:
            rs = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
        except subprocess.TimeoutExpired:
            return -1, "timeout"
        return rs.returncode, (rs.stdout + rs.stderr).strip()

    if "-linux-" in target and sys.platform == "linux":
        arch = "amd64" if target.startswith("x86_64-") else "arm64"
        image = "debian:buster" if target.endswith("-gnu") else "alpine"
        loop = 'for f in /b/*; do echo "@@ $(basename $f)"; "$f" 2>&1; echo "@@rc $?"; done'
        cmd = ["docker", "run", "--rm", "--platform", f"linux/{arch}", "-v", f"{dir}:/b:ro"]
        _, text = one([*cmd, image, "sh", "-c", loop])
        got = re.findall(r"^@@ (\S+)\n(.*?)^@@rc (\d+)$", text + "\n", re.M | re.S)
        return [(name, int(rc), body.strip()[-600:]) for name, body, rc in got]

    out = []
    if "-windows-" in target and sys.platform == "win32":
        for x in files:
            rc, text = one([str(x)])
            out.append((x.stem, rc, text[-600:]))
    elif "-apple-" in target and sys.platform == "darwin":
        rosetta = target.startswith("x86_64-") and platform.machine() != "x86_64"
        for x in files:
            rc, text = one(["arch", "-x86_64", str(x)] if rosetta else [str(x)])
            out.append((x.name, rc, text[-600:]))

    return out


def run(out: Path) -> None:
    # absolute, for docker's -v
    out = out.resolve()
    res = []
    for dir in sorted((out / "bin").glob("*/*")):
        host, target = dir.parent.name, dir.name
        for name, rc, text in runs(target, dir):
            ok = rc == 0 and text.startswith("ok ")
            res.append({"host": host, "target": target, "crate": name, "ok": ok, "out": text})
            print(f"{host} {target} {name}: {'ok' if ok else 'FAILED'}: {text}", flush=True)

    (out / "res").mkdir(parents=True, exist_ok=True)
    (out / "res" / f"run-{sys.platform}.json").write_text(json.dumps(res, indent=2))


def report(out: Path) -> None:
    builds = [json.loads(x.read_text()) for x in sorted(out.rglob("build-*.json"))]
    ran = {}
    for x in out.rglob("run-*.json"):
        for r in json.loads(x.read_text()):
            ran[r["host"], r["target"], r["crate"]] = r

    lines = ["✅ built and ran · 🔨 built, not run · ❌ didn't build · 💥 ran and failed", ""]
    for host in sorted({b["host"] for b in builds}):
        mine = {b["target"]: b for b in builds if b["host"] == host}
        targets = [t for t in TARGETS if t in mine]
        lines += [f"### host {host}", ""]
        lines += ["| crate | " + " | ".join(TARGETS[t] for t in targets) + " |"]
        lines += ["|---" * (len(targets) + 1) + "|"]
        for name in CRATES:
            cells = []
            for t in targets:
                r = ran.get((host, t, name))
                if not mine[t]["crates"].get(name):
                    cells.append("❌")
                elif r is None:
                    cells.append("🔨")
                else:
                    cells.append("✅" if r["ok"] else "💥")
            lines.append(f"| {name} | " + " | ".join(cells) + " |")

        lines.append("")
        for t in targets:
            if mine[t]["errors"]:
                lines.append(f"- {TARGETS[t]}: " + ", ".join(mine[t]["errors"]))

        lines.append("")

    bad = [r for r in ran.values() if not r["ok"]]
    if bad:
        lines += ["### ran and failed", ""]
        lines += [f"- {r['host']} {r['target']} {r['crate']}: `{r['out'][-200:]}`" for r in bad]

    text = "\n".join(lines) + "\n"
    print(text)
    if "GITHUB_STEP_SUMMARY" in os.environ:
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write(text)


def main() -> None:
    step, *args = sys.argv[1:]
    match step:
        case "collect":
            collect(args[0], args[1], Path(args[2]))
        case "run":
            run(Path(args[0]))
        case "report":
            report(Path(args[0]))


if __name__ == "__main__":
    main()
