// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! The C++ test crate cargo-solo builds for every target: C++ through cc (a thread, an exception,
//! C++20's `std::format`, iostream on macOS) and a C++ library through cmake-rs. It prints one
//! line, the same on every target.

use std::ffi::{c_char, c_int};

unsafe extern "C" {
  fn cc_hello(out: *mut c_char, size: c_int) -> c_int;
  fn cmake_hello(n: c_int) -> c_int;
}

fn main() {
  let mut buf = [0u8; 64];
  let len = unsafe { cc_hello(buf.as_mut_ptr().cast(), buf.len() as c_int) };
  let text = String::from_utf8_lossy(&buf[..len as usize]);
  println!("hello-cxx {text} {}", unsafe { cmake_hello(4) });
}
