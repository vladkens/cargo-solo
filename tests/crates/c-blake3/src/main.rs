fn main() {
  println!("ok blake3 {}", &blake3::hash(b"cargo-solo").to_hex()[..16]);
}
