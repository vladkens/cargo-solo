// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! Compiles the C half of the test crate through cc, as a crate's C dependency is, and links a C
//! program with the same compiler, as a configure script's test does (jemalloc's, for one).

use std::env;
use std::path::PathBuf;

fn main() {
  cc::Build::new().file("src/hello.c").compile("hello");
  println!("cargo:rerun-if-changed=src/hello.c");

  let exe = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("configure");
  let mut cc = cc::Build::new().get_compiler().to_command();
  let status = cc.args(["src/configure.c", "-o"]).arg(&exe).status().unwrap();
  assert!(status.success(), "the C compiler can't create executables");
  println!("cargo:rerun-if-changed=src/configure.c");
}
