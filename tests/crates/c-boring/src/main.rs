fn main() {
  let digest = boring::sha::sha256(b"cargo-solo");
  println!("ok boring {} {:02x}{:02x}", boring::version::version(), digest[0], digest[1]);
}
