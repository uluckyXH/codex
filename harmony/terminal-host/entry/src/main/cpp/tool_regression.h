#pragma once
#include <atomic>
#include <cstdio>
#include <string>
bool RunToolRegression(int files, int logs, FILE *report, const std::string &prefix,
                       const std::atomic<bool> &cancelled);
