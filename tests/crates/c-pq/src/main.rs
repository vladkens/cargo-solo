fn main() {
  println!("ok pq {}", unsafe { pq_sys::PQlibVersion() });
}
