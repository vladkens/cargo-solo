// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
// libstdc++ from C++ through cc: a thread, an exception, and C++20's std::format, with a float.
#include <cstdio>
#include <format>
#include <stdexcept>
#include <string>
#include <thread>

extern "C" int cc_hello(char *out, int size) {
  int sum = 0;
  std::thread([&] {
    for (int i = 1; i <= 4; i++) sum += i;
  }).join();

  std::string what;
  try {
    throw std::runtime_error("caught");
  } catch (const std::exception &e) {
    what = e.what();
  }

  auto text = std::format("{} {} {:.2f}", what, sum, 0.5);
  return std::snprintf(out, size, "%s", text.c_str());
}
