fn main() {
  let path = std::env::temp_dir().join(format!("c-rocksdb-{}", std::process::id()));
  let mut opts = rocksdb::Options::default();
  opts.create_if_missing(true);
  opts.set_compression_type(rocksdb::DBCompressionType::Zstd);
  let db = rocksdb::DB::open(&opts, &path).unwrap();
  db.put(b"k", b"v").unwrap();
  let got = db.get(b"k").unwrap().unwrap();
  drop(db);
  let _ = std::fs::remove_dir_all(&path);
  println!("ok rocksdb {}", String::from_utf8_lossy(&got));
}
