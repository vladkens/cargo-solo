// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! What cargo's config says: the targets of `build.target`, and whether the user set a variable
//! cargo-solo would set.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{env, fs};

use crate::args;

/// Whether the user set one of `vars` for cargo: in its environment, `args` or a config file it
/// reads, outside comments. By the name alone, which is all a choice needs.
pub fn chosen(vars: &[&str], args: &[OsString], here: &Path) -> bool {
  let name = |x: char| x.is_alphanumeric() || x == '_' || x == '-';
  let named = |x: &str| x.split(|x| !name(x)).any(|x| vars.contains(&x));
  let args: Vec<_> = args.iter().map(|x| x.to_string_lossy()).collect();
  if vars.iter().any(|x| env::var_os(x).is_some()) || args.iter().any(|x| named(x)) {
    return true;
  }

  let given = args.iter().map(|x| here.join(x.strip_prefix("--config=").unwrap_or(x)));
  let given = given.filter(|x| x.extension().is_some_and(|x| x == "toml"));
  let mut texts = given.flat_map(|x| included(&x, NESTED)).chain(configs(here));
  texts.any(|x| x.lines().any(|x| named(code(x))))
}

/// The targets of `build.target`, for `args` without `--target`, merged as cargo does: the arrays
/// of every `--config`, config file and CARGO_BUILD_TARGET (`env`) joined; else the first string.
pub fn configured(args: &[OsString], env: Option<&str>, here: &Path) -> Vec<String> {
  let given = args::values(args, "--config");
  let given = given.iter().rev().flat_map(|x| match here.join(x) {
    path if path.is_file() => included(&path, NESTED),
    _ => vec![x.clone()],
  });
  let given: Vec<_> = given.filter_map(|x| value(&x, &["build", "target"])).collect();
  let files: Vec<_> = configs(here).filter_map(|x| value(&x, &["build", "target"])).collect();
  let all = || given.iter().chain(&files);
  if all().any(|x| x.trim_start().starts_with('[')) {
    let env = env.into_iter().flat_map(|x| x.split_whitespace().map(String::from));
    return all().flat_map(|x| strings(x)).chain(env).collect();
  }

  match (given.first(), env, files.first()) {
    (Some(x), ..) | (None, None, Some(x)) => strings(x),
    (None, Some(x), _) => vec![x.into()],
    _ => vec![],
  }
}

/// How deep config files' `include`s nest at most, against a loop.
const NESTED: u8 = 8;

/// The texts of cargo's config files, by precedence: `config`, or else `config.toml`, in `.cargo`
/// of `here`, of each folder above and of CARGO_HOME, each with those it includes.
fn configs(here: &Path) -> impl Iterator<Item = String> {
  let home = env::var_os("CARGO_HOME").map(PathBuf::from);
  let home = home.or_else(|| env::home_dir().map(|x| x.join(".cargo")));
  let dirs = here.ancestors().map(|x| x.join(".cargo")).chain(home);
  let file =
    |x: PathBuf| [x.join("config"), x.join("config.toml")].into_iter().find(|x| x.is_file());
  dirs.filter_map(file).flat_map(|x| included(&x, NESTED))
}

/// The text of config file `path`, then of the files it `include`s, `depth` deep, the last first:
/// cargo merges each over those before it, and the file over them all.
fn included(path: &Path, depth: u8) -> Vec<String> {
  let Ok(text) = fs::read_to_string(path) else { return vec![] };
  let names = value(&text, &["include"]).map(|x| strings(&x)).unwrap_or_default();
  let dir = path.parent().unwrap_or(path);
  let mut out = vec![text];
  for name in names.iter().rev().filter(|_| depth > 0) {
    out.extend(included(&dir.join(name), depth - 1));
  }

  out
}

/// The value of `key` in TOML `text`, unparsed, when it sets it: under its table, as a dotted key
/// or in an inline table; a string or an array of them, which may span lines.
fn value(text: &str, key: &[&str]) -> Option<String> {
  let (mut table, mut lines) = (String::new(), text.lines().map(code));
  while let Some(line) = lines.next() {
    let line = line.trim();
    if let Some(x) = line.strip_prefix('[') {
      table = x.trim_end_matches(']').to_string();
      continue;
    }

    let Some((name, found)) = line.split_once('=') else { continue };
    let name = format!("{table}.{name}");
    let name =
      name.split('.').map(|x| x.trim().trim_matches(['"', '\''])).filter(|x| !x.is_empty());
    let name: Vec<_> = name.collect();
    // An inline table's pairs, as of `build = { target = "x", jobs = 1 }`, one a line.
    let inline = found.trim().strip_prefix('{').and_then(|x| x.strip_suffix('}'));
    let inline = inline.filter(|_| key.len() > name.len() && key.starts_with(&name));
    if let Some(x) = inline.and_then(|x| value(&x.replace(',', "\n"), &key[name.len()..])) {
      return Some(x);
    }

    if name != key {
      continue;
    }

    let mut found = found.to_string();
    while found.trim_start().starts_with('[') && !found.contains(']') {
      let Some(x) = lines.next() else { break };
      found.push_str(x);
    }

    return Some(found);
  }

  None
}

/// The strings in TOML `value`, quoted with `"` or `'`; in `"`, `\\` and `\"` taken as escapes,
/// as in a Windows path.
fn strings(value: &str) -> Vec<String> {
  let (mut out, mut chars) = (vec![], value.chars());
  while let Some(quote) = chars.find(|x| matches!(x, '"' | '\'')) {
    let mut x = String::new();
    while let Some(c) = chars.next() {
      match c {
        _ if c == quote => break,
        '\\' if quote == '"' => x.extend(chars.next()),
        _ => x.push(c),
      }
    }

    out.push(x);
  }

  out
}

/// A TOML `line` without its comment, from the first `#` outside quotes.
fn code(line: &str) -> &str {
  let (mut quote, mut escaped) = (None, false);
  for (i, c) in line.char_indices() {
    match (quote, c) {
      _ if escaped => escaped = false,
      (Some('"'), '\\') => escaped = true,
      (None, '#') => return line.get(..i).unwrap_or(line),
      (None, '"' | '\'') => quote = Some(c),
      (Some(q), _) if c == q => quote = None,
      _ => {}
    }
  }

  line
}

#[cfg(test)]
mod tests {
  use super::*;

  fn args(line: &str) -> Vec<OsString> {
    line.split_whitespace().map(OsString::from).collect()
  }

  #[test]
  fn chosen_finds_the_vars_in_args_and_cargo_s_config() {
    let dir = env::temp_dir().join(format!("cargo-solo-chosen-{}", std::process::id()));
    let (here, config) = (dir.join("a/b"), dir.join("a/.cargo/config.toml"));
    fs::create_dir_all(dir.join("a/.cargo")).unwrap();
    fs::create_dir_all(&here).unwrap();
    let vars = ["CARGO_SOLO_TEST_CHOSEN", "CARGO_SOLO_TEST_CHOSEN_x", "CARGO_SOLO_TEST_CHOSEN_x-y"];
    let chosen = |line| chosen(&vars, &args(line), &here);
    assert!(!chosen("build"));
    assert!(chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_x=0"));
    assert!(!chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_y=0"));
    assert!(chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_x-y=0"));
    assert!(!chosen("build --config env.CARGO_SOLO_TEST_CHOSEN_x-z=0"));

    let comments = "[env]\n# CARGO_SOLO_TEST_CHOSEN = \"0\"\nA = 1 # CARGO_SOLO_TEST_CHOSEN\n";
    fs::write(&config, comments).unwrap();
    assert!(!chosen("build"));

    fs::write(here.join("my.toml"), "[env]\nCARGO_SOLO_TEST_CHOSEN_x = \"0\"\n").unwrap();
    assert!(chosen("build --config my.toml"));
    assert!(chosen("build --config=my.toml"));

    fs::write(&config, r##"env = { A = "\"#", B = '#', CARGO_SOLO_TEST_CHOSEN = "0" }"##).unwrap();
    assert!(chosen("build"));
    fs::remove_dir_all(&dir).unwrap();
  }

  #[test]
  fn configured_reads_build_target_as_cargo_does() {
    let dir = env::temp_dir().join(format!("cargo-solo-configured-{}", std::process::id()));
    let (here, above) = (dir.join("a/b"), dir.join("a/.cargo/config.toml"));
    fs::create_dir_all(dir.join("a/.cargo")).unwrap();
    fs::create_dir_all(here.join(".cargo")).unwrap();
    let got = |line, env| configured(&args(line), env, &here);
    fs::write(&above, "[env]\ntarget = \"n\"\n['build'] # b\n# target = \"n\"\ntarget = 'y'\n")
      .unwrap();
    assert_eq!(got("build", None), ["y"]);

    // cargo reads `config` over `config.toml`, a file over those it includes, the last of them
    // over the others.
    let config = |text| fs::write(here.join(".cargo/config"), text).unwrap();
    fs::write(here.join(".cargo/config.toml"), "build.target = \"n\"\n").unwrap();
    config("include = ['x.toml', { path = \"y.toml\" }]\n");
    fs::write(here.join(".cargo/x.toml"), "build = { jobs = 1, target = \"x\" }\n").unwrap();
    assert_eq!(got("build", None), ["x"]);
    fs::write(here.join(".cargo/y.toml"), "build = { target = \"z\" }\n").unwrap();
    assert_eq!(got("build", None), ["z"]);
    config("include = ['x.toml', 'y.toml']\nbuild.target = \"z\"\n[build]\njobs = 1\n");
    assert_eq!(got("build", None), ["z"]);
    assert_eq!(got("build", Some("w")), ["w"]);

    fs::write(here.join("my.toml"), "[build]\ntarget = \"u\"\n").unwrap();
    assert_eq!(got("build --config my.toml", Some("w")), ["u"]);
    assert_eq!(got(r#"build --config=build.target="t" --config my.toml"#, None), ["u"]);
    assert_eq!(got(r#"build --config my.toml --config build.target="t""#, None), ["t"]);
    assert_eq!(got(r#"run -- --config build.target="t""#, None), ["z"]);

    // cargo joins the arrays and CARGO_BUILD_TARGET's names.
    fs::write(&above, "[build]\ntarget = [\n  \"x\", # x\n  'y',\n]\n").unwrap();
    config("build = { target = [\"z\", \"v\"] }\n");
    assert_eq!(got("build", None), ["z", "v", "x", "y"]);
    let line = r#"build --config build.target=["t"]"#;
    assert_eq!(got(line, Some("v w")), ["t", "z", "v", "x", "y", "v", "w"]);
    fs::remove_dir_all(&dir).unwrap();

    assert_eq!(strings(r#"["a\\b", 'c\d', "e\"f"]"#), [r"a\b", r"c\d", "e\"f"]);
  }
}
