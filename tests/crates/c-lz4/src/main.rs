fn main() {
  let data = lz4::block::compress(b"hello hello hello", None, true).unwrap();
  let back = lz4::block::decompress(&data, None).unwrap();
  println!("ok lz4 {}", String::from_utf8_lossy(&back));
}
