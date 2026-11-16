fn main() {
  let data = zstd::encode_all(&b"hello hello hello"[..], 3).unwrap();
  let back = zstd::decode_all(&data[..]).unwrap();
  println!("ok zstd {}", String::from_utf8_lossy(&back));
}
