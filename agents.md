# cargo-solo

A cargo subcommand, `cargo solo`, that cross-compiles Rust, C dependencies included, for Linux (glibc and musl) and Windows (gnu) on a Mac, Linux or Windows, with no Zig, Docker or prebuilt toolchain: the host's clang compiles C and drives the link, rustup's rust-lld links and its llvm-ar archives, and each target's libc and headers come from the distro's own packages, fetched on first use, pinned by SHA-256 and cached. One crate, `cargo-solo`. Agents carry out each change end to end by following this file.

Each fact has one source: what cargo-solo does is `readme.md` and `cargo solo --help`, the packages and their hashes the code's tables, dependencies `Cargo.toml`, checks the `Makefile`, what comes next `docs/roadmap.md`, the work under way `docs/plans/`.

## Scope

- Hosts: macOS on Apple silicon, Linux x86_64 and aarch64, Windows x86_64. Targets: `x86_64` and `aarch64` `-unknown-linux-gnu` (glibc 2.28) and `-unknown-linux-musl`, `x86_64-pc-windows-gnu`. Any other target, Apple's included, goes to cargo untouched.
- C through the `cc` crate. cmake, C++ and other glibc versions come later, each when a real project needs it.
- The basic case works with no config and no flags of its own; cargo-solo doesn't chase every case Zig covers.
- Nothing of ours is hosted: every file cargo-solo fetches is the distro's or rustup's, pinned by version and SHA-256 in the code. cargo-solo installs nothing but through rustup (a target, `llvm-tools`) and writes nothing outside its cache and cargo's target folder.
- Leave the user's `RUSTFLAGS`, `CFLAGS` and cargo config theirs: cargo-solo adds its flags without replacing any.
- Change the CLI only with the person's approval. Add a dependency only with the person's approval; prefer a small own routine. Change dependencies only with `cargo add`, `cargo remove` or `cargo upgrade`, never by hand.

## Code

- Start every `.rs` file with `// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo`, then a `//!` doc whose first sentence says what the file does.
- A file holds one feature (the arguments, a target's toolchain, the packages), not one helper each. Inside a file, `// MARK: <name>` separates its blocks.
- A comment adds what the code doesn't say, briefly: a toolchain quirk, why a package or a flag is needed. It never restates the code. Code taken from another project names it in the file's header (project, licence, link to the file); code moved from the person's own projects says nothing of where it came from.
- Helper scripts go in `scripts/`, kebab-case, Python run with `uv run`.

## Tests

- Unit-test what breaks silently: the arguments passed to cargo, the environment and flags per target, the package tables, unpacking (symlinks, paths).
- The proof is a binary that runs: the test crate `tests/hello` (`ring`, `aws-lc-rs`, C through `cc`, a thread, a `raw-dylib` link on Windows) built on every host for every target and run in its OS: Debian 10 and Alpine in Docker, Windows on a Windows runner (`make e2e`, `scripts/e2e.py`). Check the highest `GLIBC_` symbol of every `-linux-gnu` binary.
- Tests that download are `#[ignore]`d and run in CI. Tests use a temp folder for the cache, never the user's.

## Workflow

- `docs/roadmap.md` is the person's backlog: change it only when the person asks. Each item is done as a plan and one PR.
- Keep multi-step work as a checklist in `docs/plans/yyyymmdd-<name>.md`, only ticking it during the work; when done, add `Done: yyyy-mm-dd` on top and a short `## Result`, and move it to `docs/plans/completed/`.
- `make pins` re-pins the packages to what Debian and rustup have now and fetches every file from every mirror; then `make e2e`.
- Before committing, run `make check` and `make test`. `make prepare` rewrites files: not a check.
- A commit is one meaningful change a person can check: one short lowercase line, no body, docs updated in the same commit. When fixing your own unpushed commit, amend or fix up that commit instead of adding a separate fix.
- Ask the person only when what cargo-solo does must change, a dependency is needed, or the request is ambiguous. Otherwise decide and say so in your report.
- CI runners are `ubuntu-latest`, `macos-latest`, `windows-latest`, never pinned.
- Keep docs to `readme.md` (users), the rustdoc, `agents.md`, the roadmap and the plans. No progress journals, run logs or disclaimer paragraphs.
