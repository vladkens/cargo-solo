# Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
# The test crate's C++ library through cmake-rs: cmake's checks of both compilers link a program
# with them, and Threads is found in the sysroot.
cmake_minimum_required(VERSION 3.16)
project(hello_cmake C CXX)
find_package(Threads REQUIRED)
add_library(hello_cmake STATIC hello.cpp)
target_compile_features(hello_cmake PRIVATE cxx_std_17)
target_link_libraries(hello_cmake PRIVATE Threads::Threads)
install(TARGETS hello_cmake ARCHIVE DESTINATION lib)
