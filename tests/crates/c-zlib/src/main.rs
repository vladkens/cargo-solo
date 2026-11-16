fn main() {
  let ver = unsafe { std::ffi::CStr::from_ptr(libz_sys::zlibVersion()) };
  println!("ok zlib {}", ver.to_string_lossy());
}
