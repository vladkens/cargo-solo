fn main() {
  let path = std::env::temp_dir().join(format!("c-git2-{}", std::process::id()));
  let repo = git2::Repository::init(&path).unwrap();
  let empty = repo.is_empty().unwrap();
  drop(repo);
  let _ = std::fs::remove_dir_all(&path);
  let ver = git2::Version::get();
  println!(
    "ok git2 {:?} empty={empty} https={} ssh={}",
    ver.libgit2_version(),
    ver.https(),
    ver.ssh()
  );
}
