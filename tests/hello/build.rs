// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! Compiles the C half of the test crate through cc, as a crate's C dependency is, links a C
//! program as a configure script does (jemalloc's, for one), and builds a C library through
//! cmake-rs.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
  cc::Build::new().file("src/hello.c").compile("hello");
  println!("cargo:rerun-if-changed=src/hello.c");

  // By the C compiler alone, with an archive by the target's ar, found in PATH by `--host`.
  let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
  let (obj, lib) = (out.join("configure.o"), out.join("libconfigure.a"));
  let cc = || cc::Build::new().get_compiler().to_command();
  run(cc().args(["-c", "src/hello.c", "-o"]).arg(&obj));
  run(Command::new(format!("{}-ar", env::var("TARGET").unwrap())).arg("crs").arg(&lib).arg(&obj));
  let link = ["src/configure.c", "-lconfigure", "-o"];
  run(cc().arg("-L").arg(&out).args(link).arg(out.join("configure")));
  println!("cargo:rerun-if-changed=src/configure.c");

  // It finds libconfigure.a by the path given, as rdkafka-sys's finds libz-sys's zlib.
  let path = out.to_string_lossy().replace('\\', "/");
  let dir = cmake::Config::new("cmake").define("CMAKE_LIBRARY_PATH", path).build();
  println!("cargo:rustc-link-search=native={}", dir.join("lib").display());
  println!("cargo:rustc-link-lib=static=hello_cmake");
  println!("cargo:rerun-if-changed=cmake");
}

fn run(cmd: &mut Command) {
  let status = cmd.status().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
  assert!(status.success(), "{cmd:?}: {status}");
}
