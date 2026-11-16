fn main() {
  let digest = openssl::sha::sha256(b"cargo-solo");
  println!("ok openssl {} {:02x}{:02x}", openssl::version::version(), digest[0], digest[1]);
}
