fn main() {
  let ver = curl::Version::get();
  println!("ok curl {} {}", ver.version(), ver.ssl_version().unwrap_or("-"));
}
