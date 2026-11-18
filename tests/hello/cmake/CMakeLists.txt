# Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
# The test crate's C library through cmake-rs, for macOS too: a project of C and C++, as
# rdkafka-sys's is, whose check of C++ links a program, and that finds the build script's archive,
# in cargo's build folder as another crate's would be, outside the sysroot.
cmake_minimum_required(VERSION 3.18)
project(hello_cmake C CXX)
find_library(CONFIGURE configure REQUIRED)
add_library(hello_cmake STATIC hello.c)
install(TARGETS hello_cmake ARCHIVE DESTINATION lib)
