// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
// The libc's headers and the libc itself, from C; on Linux, the kernel's headers, as aws-lc's.
#include <stdio.h>
#include <string.h>
#ifdef __linux__
#include <linux/random.h>
_Static_assert(RNDGETENTCNT != 0, "the kernel's ioctls");
#endif

int hello_len(const char *text) {
  char buf[64];
  snprintf(buf, sizeof buf, "%s", text);
  return (int)strlen(buf);
}
