fn main() {
  let mut parser = tree_sitter::Parser::new();
  parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
  let tree = parser.parse("fn main() {}", None).unwrap();
  println!("ok tree-sitter {}", tree.root_node().kind());
}
