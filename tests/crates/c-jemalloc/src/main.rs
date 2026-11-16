#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
  let xs: Vec<u64> = (0..1000).collect();
  println!("ok jemalloc {}", xs.iter().sum::<u64>());
}
