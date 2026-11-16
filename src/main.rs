// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! `cargo solo`: runs a cargo command with each `--target`'s C compiler, linker and libc set up.

mod args;
mod packages;
mod toolchain;

use std::process::{Command, ExitCode};
use std::{env, io};

use args::Cmd;
use toolchain::{Host, Target};

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
  if let Some(target) = Target::wrapped(&arg0) {
    return Err(exec(toolchain::wrap(target, argv.collect())?));
  }

  let (args, targets) = match args::parse(env::args_os()) {
    Cmd::Help => {
      print!("{}", args::help(&packages::cache()?.display().to_string()));
      return Ok(());
    }
    Cmd::Version => {
      println!("cargo-solo {}", env!("CARGO_PKG_VERSION"));
      return Ok(());
    }
    Cmd::Cargo { args, targets } => (args, targets),
  };

  // cargo runs a subcommand with CARGO set to itself, so `cargo +nightly solo` stays on nightly.
  let mut cargo = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
  let ours: Vec<_> = targets.iter().filter_map(|x| Target::find(x)).collect();
  if !ours.is_empty() {
    let host = Host::find()?;
    cargo.envs(host.env());
    for target in ours {
      cargo.envs(host.setup(target, &args)?);
    }
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
pub fn exec(mut cmd: Command) -> io::Error {
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
