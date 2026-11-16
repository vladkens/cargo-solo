fn main() {
  let re = pcre2::bytes::Regex::new(r"\d+").unwrap();
  let found = re.find(b"abc 123").unwrap().unwrap();
  println!("ok pcre2 {}", String::from_utf8_lossy(found.as_bytes()));
}
