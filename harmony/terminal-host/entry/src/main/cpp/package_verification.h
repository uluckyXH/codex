#pragma once
#include <cstdio>
#include <string>
bool ExportInstalledPackageForVerification(int files, const std::string &directoryRoot, FILE *report, const std::string &prefix);
