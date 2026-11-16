fn main() {
  let db = duckdb::Connection::open_in_memory().unwrap();
  let n: i64 = db.query_row("select count(*) from range(10)", [], |x| x.get(0)).unwrap();
  println!("ok duckdb {n}");
}
