fn main() {
  let (_, ver) = rdkafka::util::get_rdkafka_version();
  println!("ok rdkafka {ver}");
}
