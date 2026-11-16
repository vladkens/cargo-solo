use lmdb_master_sys as ffi;

fn main() {
  let mut env = std::ptr::null_mut();
  assert_eq!(unsafe { ffi::mdb_env_create(&mut env) }, 0);
  unsafe { ffi::mdb_env_close(env) };
  let ver =
    unsafe { ffi::mdb_version(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
  println!("ok lmdb {}", unsafe { std::ffi::CStr::from_ptr(ver) }.to_string_lossy());
}
