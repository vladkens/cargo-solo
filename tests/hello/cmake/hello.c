// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
// The libc's headers through cmake.
#include <string.h>

int cmake_len(const char *text) {
  return (int)strlen(text);
}
