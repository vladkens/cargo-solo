use std::io::Read;

fn main() {
  let mut data = vec![];
  xz2::read::XzEncoder::new(&b"hello hello hello"[..], 6).read_to_end(&mut data).unwrap();
  let mut back = String::new();
  xz2::read::XzDecoder::new(&data[..]).read_to_string(&mut back).unwrap();
  println!("ok xz {back}");
}
