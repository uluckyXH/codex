cmake_minimum_required(VERSION 3.5.0)
project(harmony_terminal_host)
add_library(hostprobe SHARED native_module.cpp codex_launcher.cpp secure_config.cpp tool_regression.cpp package_verification.cpp terminal_bridge.cpp pty_session.cpp)
target_link_libraries(hostprobe PUBLIC libace_napi.z.so)
