fn main() {
  let db = rusqlite::Connection::open_in_memory().unwrap();
  let ver: String = db.query_row("select sqlite_version()", [], |x| x.get(0)).unwrap();
  println!("ok sqlite {ver}");
}
