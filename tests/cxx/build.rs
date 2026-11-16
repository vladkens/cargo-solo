// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! Compiles the C++ halves of the test crate: one through cc, one through cmake-rs, as a crate's
//! C++ dependency is.

fn main() {
  cc::Build::new().cpp(true).std("c++20").file("src/hello.cpp").compile("hello");
  println!("cargo:rerun-if-changed=src/hello.cpp");

  let dir = cmake::Config::new("cmake").build();
  println!("cargo:rustc-link-search=native={}", dir.join("lib").display());
  println!("cargo:rustc-link-lib=static=hello_cmake");
  println!("cargo:rerun-if-changed=cmake");
}
