// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
// A program as a configure script links one: the crt objects, the libc and libgcc, by clang alone,
// and a function from an archive.
#include <stdio.h>

int hello_len(const char *text);

int main(void) {
  printf("hello %d\n", hello_len("configure"));
  return 0;
}
