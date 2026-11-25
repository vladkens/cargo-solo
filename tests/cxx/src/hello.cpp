// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
// The C++ library from C++ through cc: a thread, an exception, and a float formatted by C++20's
// std::format, or on macOS by iostream: macOS's libc++ has std::format from 13.3, newer than
// rustc's oldest.
#include <cstdio>
#include <stdexcept>
#include <string>
#include <thread>
#ifdef __APPLE__
#include <iomanip>
#include <sstream>
#else
#include <format>
#endif

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

#ifdef __APPLE__
  std::ostringstream text;
  text << what << ' ' << sum << ' ' << std::fixed << std::setprecision(2) << 0.5;
  auto line = text.str();
#else
  auto line = std::format("{} {} {:.2f}", what, sum, 0.5);
#endif
  return std::snprintf(out, size, "%s", line.c_str());
}
