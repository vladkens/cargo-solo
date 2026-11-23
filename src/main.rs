// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! `cargo solo`: runs a cargo command with each `--target`'s C compiler, linker and libc set up.

mod host;
mod packages;
mod toolchain;
mod unpack;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs, io};

use host::Host;
use toolchain::{TARGETS, Target};

// MARK: Arguments

/// What `cargo solo` was asked.
#[derive(Debug, PartialEq)]
enum Cmd {
  /// `-h`, `--help` or nothing.
  Help,
  /// `-V` or `--version`.
  Version,
  /// A cargo command with its args as given, and the `--target`s in them, in order.
  Cargo { args: Vec<OsString>, targets: Vec<String> },
}

/// The command in `args`, the program's name first: cargo runs a subcommand as `cargo-solo solo …`,
/// and `cargo-solo …` run by hand works the same.
fn parse(args: impl IntoIterator<Item = OsString>) -> Cmd {
  let mut args: Vec<OsString> = args.into_iter().skip(1).collect();
  if args.first().is_some_and(|x| x == "solo") {
    args.remove(0);
  }

  match args.first().map(|x| x.to_str()) {
    None | Some(Some("-h" | "--help")) => return Cmd::Help,
    Some(Some("-V" | "--version")) => return Cmd::Version,
    _ => {}
  }

  let targets = values(&args, "--target");
  Cmd::Cargo { args, targets }
}

/// The values of `flag x` and `flag=x` in `args`, up to a `--`, after which the args are the
/// program's.
fn values(args: &[OsString], flag: &str) -> Vec<String> {
  let mut out = vec![];
  let mut it = args.iter().map(|x| x.to_str());
  while let Some(arg) = it.next() {
    match arg {
      Some("--") => break,
      Some(x) if x == flag => out.extend(it.next().flatten().map(String::from)),
      Some(x) => out.extend(x.split_once('=').filter(|x| x.0 == flag).map(|x| x.1.into())),
      None => {}
    }
  }

  out
}

/// Whether `args` has one of `flags`, before a `--`.
fn has(args: &[OsString], flags: &[&str]) -> bool {
  let args = args.iter().take_while(|x| *x != "--");
  args.filter_map(|x| x.to_str()).any(|x| flags.contains(&x))
}

/// cargo's command in `args`: the first arg that is neither a flag nor a global flag's value.
fn command(args: &[OsString]) -> Option<&str> {
  let mut it = args.iter().map(|x| x.to_str().unwrap_or_default());
  while let Some(arg) = it.next() {
    match arg {
      "--color" | "--config" | "-Z" | "-C" => _ = it.next(),
      _ if arg.starts_with('-') => {}
      _ => return Some(arg),
    }
  }

  None
}

/// The help, with the cache folder `cache`.
fn help(cache: &str) -> String {
  let ours = || TARGETS.iter().filter(|x| x.here());
  let width = ours().map(|x| x.rust.len()).max().unwrap_or(0);
  let targets: String =
    ours().map(|x| format!("  {:width$}  {}\n", x.rust, x.libc.version())).collect();

  format!(
    "Cross-compiles Rust with the host's clang and rustup's lld: no Zig, no Docker.

Usage: cargo solo <cargo command> [args]...

  cargo solo build --release --target x86_64-unknown-linux-gnu --target x86_64-pc-windows-gnu
  cargo solo clippy --target aarch64-unknown-linux-musl

Each of these targets, from --target or else cargo's build.target, gets its C and C++ compilers,
linker, libc and cmake toolchain file; any other goes to cargo as is:
{targets}
Needs clang, and rustup to add a target or llvm-tools when missing; cmake for crates built with it,
and on Windows ninja. Each target's libc comes from Debian's packages (Apple's SDK from Zig and
LLVM) on first use, its libstdc++ on the first C++ compile, cached in {cache} (CARGO_SOLO_CACHE).
`cargo solo fetch --target <target>` fetches them all ahead, and in a project its crates too.
"
  )
}

// MARK: Run

fn main() -> ExitCode {
  match start() {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("cargo-solo: {e}");
      ExitCode::FAILURE
    }
  }
}

fn start() -> io::Result<()> {
  let mut argv = env::args_os();
  let arg0 = argv.next().unwrap_or_default();
  if let Some((target, cxx)) = Target::wrapped(&arg0) {
    return Err(exec(toolchain::wrap(target, cxx, argv.collect())?));
  }

  let (args, mut targets) = match parse(env::args_os()) {
    Cmd::Help => {
      print!("{}", help(&packages::cache()?.display().to_string()));
      return Ok(());
    }
    Cmd::Version => {
      println!("cargo-solo {}", env!("CARGO_PKG_VERSION"));
      return Ok(());
    }
    Cmd::Cargo { args, targets } => (args, targets),
  };

  // Without a --target, cargo builds for its config's build.target, else for the host.
  if targets.is_empty() {
    let var = env::var("CARGO_BUILD_TARGET").ok();
    targets = configured(&args, var.as_deref(), &env::current_dir()?);
  }

  // cargo runs a subcommand with CARGO set to itself, so `cargo +nightly solo` stays on nightly.
  let mut cargo = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
  let ours: Vec<_> = targets.iter().filter_map(|x| Target::find(x)).collect();
  // A fetch that may not touch the network leaves the C++ library to the first C++ compile.
  let fetch = command(&args) == Some("fetch") && !has(&args, &["--offline", "--frozen"]);
  if !ours.is_empty() {
    let host = Host::find()?;
    cargo.envs(host.env());
    for target in &ours {
      cargo.envs(host.setup(target, &args)?);
      // What the first C++ compile would fetch, so that no build after it needs the network.
      if let Some(lib) = target.cxx.filter(|_| fetch) {
        packages::ensure(&packages::cache()?, &target.libstdcxx(), lib.packages)?;
      }
    }
  }

  // Outside a project, as a Dockerfile's step before it copies the sources, a fetch is ours alone:
  // cargo would fail with no Cargo.toml. Its help is cargo's still.
  let asks = has(&args, &["-h", "--help"]);
  if fetch && !asks && !ours.is_empty() && !project(&args, &env::current_dir()?) {
    return Ok(());
  }

  cargo.args(args);
  Err(exec(cargo))
}

pub fn bad(msg: impl Into<String>) -> io::Error {
  io::Error::other(msg.into())
}

/// The output of `cmd`, which must succeed; when it fails, the error has its stderr.
pub fn run(cmd: &mut Command) -> io::Result<Vec<u8>> {
  let name = cmd.get_program().to_string_lossy().into_owned();
  let out = match cmd.output() {
    Ok(x) => x,
    Err(e) if e.kind() == io::ErrorKind::NotFound => {
      return Err(io::Error::new(e.kind(), format!("{name} not found")));
    }
    Err(e) => return Err(bad(format!("{name}: {e}"))),
  };

  if !out.status.success() {
    let err = String::from_utf8_lossy(&out.stderr);
    return Err(bad(format!("{name} failed ({}): {}", out.status, err.trim())));
  }

  Ok(out.stdout)
}

/// Runs `cmd` in place of this program, its exit code ours; returns only when it can't run.
fn exec(mut cmd: Command) -> io::Error {
  let name = cmd.get_program().to_string_lossy().into_owned();
  let fail = |e: io::Error| match e.kind() {
    io::ErrorKind::NotFound => io::Error::new(e.kind(), format!("{name} not found")),
    _ => bad(format!("{name}: {e}")),
  };

  #[cfg(unix)]
  return fail(std::os::unix::process::CommandExt::exec(&mut cmd));
  #[cfg(not(unix))]
  match cmd.status() {
    // Windows' exit codes are 32-bit, as an NTSTATUS of a crash is: ExitCode takes only 8.
    Ok(x) => std::process::exit(x.code().unwrap_or(1)),
    Err(e) => fail(e),
  }
}

// MARK: Cargo's config

/// Whether the user set one of `vars` for cargo: in its environment, `args` or a config file it
/// reads, outside comments. By the name alone, which is all a choice needs.
pub fn chosen(vars: &[&str], args: &[OsString], here: &Path) -> bool {
  let name = |x: char| x.is_alphanumeric() || x == '_' || x == '-';
  let named = |x: &str| x.split(|x| !name(x)).any(|x| vars.contains(&x));
  let args: Vec<_> = args.iter().map(|x| x.to_string_lossy()).collect();
  if vars.iter().any(|x| env::var_os(x).is_some()) || args.iter().any(|x| named(x)) {
    return true;
  }

  let given = args.iter().map(|x| here.join(x.strip_prefix("--config=").unwrap_or(x)));
  let given = given.filter(|x| x.extension().is_some_and(|x| x == "toml"));
  let mut texts = given.flat_map(|x| included(&x, NESTED)).chain(configs(here));
  texts.any(|x| x.lines().any(|x| named(code(x))))
}

/// The targets of `build.target`, for `args` without `--target`, merged as cargo does: the arrays
/// of every `--config`, config file and CARGO_BUILD_TARGET (`env`) joined; else the first string.
fn configured(args: &[OsString], env: Option<&str>, here: &Path) -> Vec<String> {
  let given = values(args, "--config");
  let given = given.iter().rev().flat_map(|x| match here.join(x) {
    path if path.is_file() => included(&path, NESTED),
    _ => vec![x.clone()],
  });
  let given: Vec<_> = given.filter_map(|x| value(&x, &["build", "target"])).collect();
  let files: Vec<_> = configs(here).filter_map(|x| value(&x, &["build", "target"])).collect();
  let all = || given.iter().chain(&files);
  if all().any(|x| x.trim_start().starts_with('[')) {
    let env = env.into_iter().flat_map(|x| x.split_whitespace().map(String::from));
    return all().flat_map(|x| strings(x)).chain(env).collect();
  }

  match (given.first(), env, files.first()) {
    (Some(x), ..) | (None, None, Some(x)) => strings(x),
    (None, Some(x), _) => vec![x.into()],
    _ => vec![],
  }
}

/// Whether cargo may find a project for `args` run in `here`: one `--manifest-path` (`-m`) names,
/// one in the folder of `-C`, or a Cargo.toml there or in a folder above.
fn project(args: &[OsString], here: &Path) -> bool {
  let mut args = args.iter().take_while(|x| *x != "--").filter_map(|x| x.to_str());
  let named = |x: &str| x.starts_with("--manifest-path") || x.starts_with("-m") || x == "-C";
  args.any(named) || here.ancestors().any(|x| x.join("Cargo.toml").is_file())
}

/// How deep config files' `include`s nest at most, against a loop.
const NESTED: u8 = 8;

/// The texts of cargo's config files, by precedence: `config`, or else `config.toml`, in `.cargo`
/// of `here`, of each folder above and of CARGO_HOME, each with those it includes.
fn configs(here: &Path) -> impl Iterator<Item = String> {
  let home = env::var_os("CARGO_HOME").map(PathBuf::from);
  let home = home.or_else(|| env::home_dir().map(|x| x.join(".cargo")));
  let dirs = here.ancestors().map(|x| x.join(".cargo")).chain(home);
  let file =
    |x: PathBuf| [x.join("config"), x.join("config.toml")].into_iter().find(|x| x.is_file());
  dirs.filter_map(file).flat_map(|x| included(&x, NESTED))
}

/// The text of config file `path`, then of the files it `include`s, `depth` deep, the last first:
/// cargo merges each over those before it, and the file over them all.
fn included(path: &Path, depth: u8) -> Vec<String> {
  let Ok(text) = fs::read_to_string(path) else { return vec![] };
  let names = value(&text, &["include"]).map(|x| strings(&x)).unwrap_or_default();
  let dir = path.parent().unwrap_or(path);
  let mut out = vec![text];
  for name in names.iter().rev().filter(|_| depth > 0) {
    out.extend(included(&dir.join(name), depth - 1));
  }

  out
}

/// The value of `key` in TOML `text`, unparsed, when it sets it: under its table, as a dotted key
/// or in an inline table; a string or an array of them, which may span lines.
fn value(text: &str, key: &[&str]) -> Option<String> {
  let (mut table, mut lines) = (String::new(), text.lines().map(code));
  while let Some(line) = lines.next() {
    let line = line.trim();
    if let Some(x) = line.strip_prefix('[') {
      table = x.trim_end_matches(']').to_string();
      continue;
    }

    let Some((name, found)) = line.split_once('=') else { continue };
    let name = format!("{table}.{name}");
    let name =
      name.split('.').map(|x| x.trim().trim_matches(['"', '\''])).filter(|x| !x.is_empty());
    let name: Vec<_> = name.collect();
    // An inline table's pairs, as of `build = { target = "x", jobs = 1 }`, one a line.
    let inline = found.trim().strip_prefix('{').and_then(|x| x.strip_suffix('}'));
    let inline = inline.filter(|_| key.len() > name.len() && key.starts_with(&name));
    if let Some(x) = inline.and_then(|x| value(&x.replace(',', "\n"), &key[name.len()..])) {
      return Some(x);
    }

    if name != key {
      continue;
    }

    let mut found = found.to_string();
    while found.trim_start().starts_with('[') && !found.contains(']') {
      let Some(x) = lines.next() else { break };
      found.push_str(x);
    }

    return Some(found);
  }

  None
}

/// The strings in TOML `value`, quoted with `"` or `'`; in `"`, `\\` and `\"` taken as escapes,
/// as in a Windows path.
fn strings(value: &str) -> Vec<String> {
  let (mut out, mut chars) = (vec![], value.chars());
  while let Some(quote) = chars.find(|x| matches!(x, '"' | '\'')) {
    let mut x = String::new();
    while let Some(c) = chars.next() {
      match c {
        _ if c == quote => break,
        '\\' if quote == '"' => x.extend(chars.next()),
        _ => x.push(c),
      }
    }

    out.push(x);
  }

  out
}

/// A TOML `line` without its comment, from the first `#` outside quotes.
fn code(line: &str) -> &str {
  let (mut quote, mut escaped) = (None, false);
  for (i, c) in line.char_indices() {
    match (quote, c) {
      _ if escaped => escaped = false,
      (Some('"'), '\\') => escaped = true,
      (None, '#') => return line.get(..i).unwrap_or(line),
      (None, '"' | '\'') => quote = Some(c),
      (Some(q), _) if c == q => quote = None,
      _ => {}
    }
  }

  line
}

#[cfg(test)]
mod tests {
  use super::*;

  fn cmd(line: &str) -> Cmd {
    parse(line.split_whitespace().map(OsString::from))
  }

  fn cargo(args: &str, targets: &[&str]) -> Cmd {
    let args = args.split_whitespace().map(OsString::from).collect();
    Cmd::Cargo { args, targets: targets.iter().map(|x| x.to_string()).collect() }
  }

  #[test]
  fn parse_takes_the_cargo_command() {
    let t = "x86_64-unknown-linux-gnu";
    assert_eq!(cmd("cargo-solo solo build --release"), cargo("build --release", &[]));
    assert_eq!(cmd("cargo-solo build --target x"), cargo("build --target x", &["x"]));
    assert_eq!(
      cmd(&format!("cargo-solo solo build --target {t} --target=aarch64-apple-darwin")),
      cargo(
        &format!("build --target {t} --target=aarch64-apple-darwin"),
        &[t, "aarch64-apple-darwin"]
      )
    );
    // After `--` the args are the program's, `cargo solo run` passes them on.
    let run = "run --target=a -- --target b";
    assert_eq!(cmd(&format!("cargo-solo solo {run}")), cargo(run, &["a"]));
    assert_eq!(cmd("cargo-solo solo build --target"), cargo("build --target", &[]));
  }

  #[test]
  fn command_skips_cargo_s_global_flags() {
    let got = |line: &str| command(&args(line)).map(String::from);
    assert_eq!(got("fetch --target x").as_deref(), Some("fetch"));
    assert_eq!(got("-v --color always --config a=1 -Z x -C a fetch").as_deref(), Some("fetch"));
    assert_eq!(got("--color=never build").as_deref(), Some("build"));
    assert_eq!(got("-v"), None);
  }

  #[test]
  fn parse_keeps_help_and_version_of_cargo_commands() {
    assert_eq!(cmd("cargo-solo"), Cmd::Help);
    assert_eq!(cmd("cargo-solo solo"), Cmd::Help);
    assert_eq!(cmd("cargo-solo solo --help"), Cmd::Help);
    assert_eq!(cmd("cargo-solo solo -h"), Cmd::Help);
    assert_eq!(cmd("cargo-solo solo -V"), Cmd::Version);
    assert_eq!(cmd("cargo-solo solo --version"), Cmd::Version);
    assert_eq!(cmd("cargo-solo solo build --help"), cargo("build --help", &[]));
  }

  #[test]
  fn help_lists_every_target_of_this_host() {
    let text = help("/tmp/cache");
    for target in &TARGETS {
      assert_eq!(text.contains(target.rust), target.here(), "{}", target.rust);
    }

    assert!(text.contains("/tmp/cache"));
  }

  fn args(line: &str) -> Vec<OsString> {
    line.split_whitespace().map(OsString::from).collect()
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
  fn configured_reads_build_target_as_cargo_does() {
    let dir = env::temp_dir().join(format!("cargo-solo-configured-{}", std::process::id()));
    let (here, above) = (dir.join("a/b"), dir.join("a/.cargo/config.toml"));
    fs::create_dir_all(dir.join("a/.cargo")).unwrap();
    fs::create_dir_all(here.join(".cargo")).unwrap();
    let got = |line, env| configured(&args(line), env, &here);
    fs::write(&above, "[env]\ntarget = \"n\"\n['build'] # b\n# target = \"n\"\ntarget = 'y'\n")
      .unwrap();
    assert_eq!(got("build", None), ["y"]);

    // cargo reads `config` over `config.toml`, a file over those it includes, the last of them
    // over the others.
    let config = |text| fs::write(here.join(".cargo/config"), text).unwrap();
    fs::write(here.join(".cargo/config.toml"), "build.target = \"n\"\n").unwrap();
    config("include = ['x.toml', { path = \"y.toml\" }]\n");
    fs::write(here.join(".cargo/x.toml"), "build = { jobs = 1, target = \"x\" }\n").unwrap();
    assert_eq!(got("build", None), ["x"]);
    fs::write(here.join(".cargo/y.toml"), "build = { target = \"z\" }\n").unwrap();
    assert_eq!(got("build", None), ["z"]);
    config("include = ['x.toml', 'y.toml']\nbuild.target = \"z\"\n[build]\njobs = 1\n");
    assert_eq!(got("build", None), ["z"]);
    assert_eq!(got("build", Some("w")), ["w"]);

    fs::write(here.join("my.toml"), "[build]\ntarget = \"u\"\n").unwrap();
    assert_eq!(got("build --config my.toml", Some("w")), ["u"]);
    assert_eq!(got(r#"build --config=build.target="t" --config my.toml"#, None), ["u"]);
    assert_eq!(got(r#"build --config my.toml --config build.target="t""#, None), ["t"]);
    assert_eq!(got(r#"run -- --config build.target="t""#, None), ["z"]);

    // cargo joins the arrays and CARGO_BUILD_TARGET's names.
    fs::write(&above, "[build]\ntarget = [\n  \"x\", # x\n  'y',\n]\n").unwrap();
    config("build = { target = [\"z\", \"v\"] }\n");
    assert_eq!(got("build", None), ["z", "v", "x", "y"]);
    let line = r#"build --config build.target=["t"]"#;
    assert_eq!(got(line, Some("v w")), ["t", "z", "v", "x", "y", "v", "w"]);
    fs::remove_dir_all(&dir).unwrap();

    assert_eq!(strings(r#"["a\\b", 'c\d', "e\"f"]"#), [r"a\b", r"c\d", "e\"f"]);
  }

  #[test]
  fn project_is_where_cargo_finds_one() {
    let dir = env::temp_dir().join(format!("cargo-solo-project-{}", std::process::id()));
    let here = dir.join("a/b");
    fs::create_dir_all(&here).unwrap();
    assert!(!project(&args("fetch --target x"), &here));
    assert!(project(&args("fetch --manifest-path ../c/Cargo.toml"), &here));
    assert!(project(&args("fetch --manifest-path=c/Cargo.toml"), &here));
    assert!(project(&args("fetch -m c/Cargo.toml"), &here));
    assert!(project(&args("fetch -mc/Cargo.toml"), &here));
    assert!(project(&args("-C c fetch"), &here));
    assert!(!project(&args("run -- --manifest-path c"), &here));

    fs::write(dir.join("a/Cargo.toml"), "").unwrap();
    assert!(project(&args("fetch"), &here));
    fs::remove_dir_all(&dir).unwrap();
  }
}
