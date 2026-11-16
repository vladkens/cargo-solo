#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
  let xs: Vec<u64> = (0..1000).collect();
  println!("ok mimalloc {}", xs.iter().sum::<u64>());
}
