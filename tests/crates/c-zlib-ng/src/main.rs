use std::io::{Read, Write};

fn main() {
  let mut enc = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
  enc.write_all(b"hello hello hello").unwrap();
  let data = enc.finish().unwrap();
  let mut back = String::new();
  flate2::read::GzDecoder::new(&data[..]).read_to_string(&mut back).unwrap();
  println!("ok zlib-ng {back}");
}
