// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! Compiles the C half of the test crate through cc, as a crate's C dependency is.

fn main() {
  cc::Build::new().file("src/hello.c").compile("hello");
  println!("cargo:rerun-if-changed=src/hello.c");
}
