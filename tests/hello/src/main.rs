// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! The test crate cargo-solo builds for every target: ring (C and assembly through cc), a C file
//! of its own, and a thread. It prints one line, the same on every target.

use std::ffi::{CStr, c_char, c_int};

unsafe extern "C" {
  fn hello_len(text: *const c_char) -> c_int;
}

fn main() {
  let text: &CStr = c"cargo-solo";
  let len = unsafe { hello_len(text.as_ptr()) };
  let sum =
    std::thread::spawn(move || ring::digest::digest(&ring::digest::SHA256, text.to_bytes()));
  let hex: String = sum.join().unwrap().as_ref().iter().map(|x| format!("{x:02x}")).collect();
  println!("hello {len} {}", &hex[..16]);
}
