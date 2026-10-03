cmake_minimum_required(VERSION 3.5.0)
project(harmony_host_probe)
add_library(hostprobe SHARED host_probe.cpp)
target_link_libraries(hostprobe PUBLIC libace_napi.z.so)
