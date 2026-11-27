// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
// Apple's frameworks from C, as curl's: their headers and what they export.
#include <CoreFoundation/CoreFoundation.h>
#include <SystemConfiguration/SCDynamicStoreCopySpecific.h>

int apple_len(const char *text) {
  CFDictionaryRef proxies = SCDynamicStoreCopyProxies(NULL);
  if (proxies) CFRelease(proxies);

  // A constant string, as CFSTR makes it, against one made at run time.
  CFStringRef str = CFStringCreateWithCString(kCFAllocatorDefault, text, kCFStringEncodingUTF8);
  CFStringRef same = CFSTR("cargo-solo");
  int len = (int)CFStringGetLength(str);
  if (CFStringCompare(str, same, 0) != kCFCompareEqualTo) len = -1;
  CFRelease(str);
  return len;
}
