// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! The test crate cargo-solo builds for every target: ring (C and assembly through cc), with the
//! `aws-lc-rs` feature aws-lc-rs, rustls' crypto (C through its own cc build, NASM objects on
//! Windows), a C file of its own, a C program its build script links and a C library through
//! cmake, a thread, on Windows a `raw-dylib` link, as windows-sys's are, and on macOS Apple's
//! frameworks: CoreFoundation for the time zone, as chrono's, Security, as native-tls', and
//! SystemConfiguration for the proxies, as reqwest's. It prints one line, the same on every target.

use std::ffi::{CStr, c_char, c_int};

unsafe extern "C" {
  fn hello_len(text: *const c_char) -> c_int;
  fn cmake_len(text: *const c_char) -> c_int;
}

// rustc makes its import library with dlltool.
#[cfg(windows)]
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
  fn GetCurrentProcessId() -> u32;
}

fn main() {
  let text: &CStr = c"cargo-solo";
  let len = unsafe { hello_len(text.as_ptr()) };
  assert_eq!(unsafe { cmake_len(text.as_ptr()) }, len);
  let sum =
    std::thread::spawn(move || ring::digest::digest(&ring::digest::SHA256, text.to_bytes()));
  #[cfg(feature = "aws-lc-rs")]
  let aws = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, text.to_bytes());
  #[cfg(windows)]
  assert_eq!(unsafe { GetCurrentProcessId() }, std::process::id());
  #[cfg(target_os = "macos")]
  {
    assert!(!iana_time_zone::get_timezone().unwrap().is_empty());
    security_framework::random::SecRandom::default().copy_bytes(&mut [0; 8]).unwrap();
    let store = system_configuration::dynamic_store::SCDynamicStoreBuilder::new("hello").build();
    assert!(store.unwrap().get_proxies().is_some());
  }

  let hex = |x: &[u8]| x[..8].iter().map(|x| format!("{x:02x}")).collect::<String>();
  print!("hello {len} {}", hex(sum.join().unwrap().as_ref()));
  #[cfg(feature = "aws-lc-rs")]
  print!(" {}", hex(aws.as_ref()));
  println!();
}
