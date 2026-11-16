// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
// A program as a configure script links: the crt objects, the libc and libgcc, by clang alone.
#include <stdio.h>

int main(void) {
  puts("hello");
  return 0;
}
