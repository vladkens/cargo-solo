// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
// The C++ library from C++ through cmake: std::async on a thread, a map, an exception across
// them.
#include <future>
#include <map>
#include <stdexcept>
#include <string>

extern "C" int cmake_hello(int n) {
  auto squares = std::async(std::launch::async, [n] {
    std::map<std::string, int> map;
    for (int i = 0; i < n; i++) map[std::to_string(i)] = i * i;
    if (map.size() > 3) throw std::out_of_range("too many");
    return (int)map.size();
  });

  try {
    return squares.get();
  } catch (const std::out_of_range &e) {
    return 40 + (int)std::string(e.what()).size();
  }
}
