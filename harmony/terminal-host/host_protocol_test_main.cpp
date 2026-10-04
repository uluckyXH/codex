#include "host_json.h"
#include "owned_processes.h"
#include <cstdio>
#include <cstdlib>
#include <string>
using namespace codex_hnp;
static unsigned checks = 0;
static void Check(bool passed, const char *name) {
    if (!passed) { fprintf(stderr, "FAIL %s\n", name); exit(1); }
    ++checks; printf("PASS %s\n", name);
}
static std::string Stat(const std::string &name = "sh ) job", const std::string &start = "987654") {
    return "123 (" + name + ") S 100 123 123 0 -1 4194304 1 2 3 4 5 6 7 8 9 10 1 0 " + start + " 4096 0\n";
}
int main() {
    std::string value;
    Check(JsonPathString(R"({"paths":{"root":"/files/codex"},"identity":{"euid":13579}})", {"paths", "root"}, value) && value == "/files/codex", "initializer exact root path");
    Check(JsonPathString(R"({"paths":{"root":"/\u4e2d\u6587/space and \"quote\"/codex"}})", {"paths", "root"}, value) && value == "/中文/space and \"quote\"/codex", "JSON Unicode spaces and quotes preserved");
    Check(JsonPathString(R"({"paths":{"root":"/\ud83d\ude42/codex"}})", {"paths", "root"}, value) && value == "/🙂/codex", "JSON surrogate pair");
    Check(!JsonPathString(R"({"paths":{"root":"a","root":"b"}})", {"paths", "root"}, value), "duplicate root rejected");
    Check(!JsonPathString(R"({"paths":{"root":"a"},"paths":{"root":"b"}})", {"paths", "root"}, value), "duplicate paths rejected");
    Check(!JsonPathString(R"({"elsewhere":{"paths":{"root":"a"}}})", {"paths", "root"}, value), "nested unrelated path not adopted");
    Check(!JsonPathString(R"({"paths":{"root":null}})", {"paths", "root"}, value), "non-string root rejected");
    Check(!JsonPathString(R"({"paths":{"root":"a"}} trailing)", {"paths", "root"}, value), "trailing bytes rejected");
    Check(!JsonPathString(R"({"paths":{"root":"\ud800"}})", {"paths", "root"}, value), "broken surrogate rejected");
    Check(!JsonPathString(std::string(16385, ' '), {"paths", "root"}, value), "initializer output bound");
    Check(JsonPathString(R"({"ignored":[true,false,null,1.2e-3,{"other":"x"}],"paths":{"root":"ok"}})", {"paths", "root"}, value) && value == "ok", "other valid JSON values skipped");
    Check(!JsonPathString(R"({"ignored":01,"paths":{"root":"ok"}})", {"paths", "root"}, value), "malformed number rejected");
    std::string deep(30, '['); deep += "0"; deep += std::string(30, ']');
    Check(!JsonPathString("{\"ignored\":" + deep + ",\"paths\":{\"root\":\"ok\"}}", {"paths", "root"}, value), "excess nesting rejected");
    ProcessIdentity identity;
    Check(ParseProcessUids("Name:\tsh\nUid:\t13579\t24680\t24680\t24680\nGid:\t100\t200\t200\t200\n", identity) &&
        identity.realUid == 13579 && identity.effectiveUid == 24680, "actual process UIDs independent of inode owner and GIDs");
    Check(!ParseProcessUids("Uid:\t1\t2\t3\n", identity), "truncated UID tuple rejected");
    Check(!ParseProcessUids("Uid:\t1\t2\t3\t4294967296\n", identity), "UID overflow rejected");
    Check(ParseProcessStat(Stat(), identity) && identity.pid == 123 && identity.parent == 100 && identity.group == 123 && identity.session == 123 && identity.start == 987654, "proc comm parentheses and exact fields");
    Check(!ParseProcessStat(Stat("normal", "-1"), identity), "negative start identity rejected");
    Check(!ParseProcessStat(Stat("normal", "18446744073709551616"), identity), "start time overflow rejected");
    Check(!ParseProcessStat("123 (sh) S 1 123 123", identity), "truncated process record rejected");
    Check(!ParseProcessStat("123junk (sh)" + Stat().substr(Stat().rfind(')') + 1), identity), "ambiguous PID prefix rejected");
    Check(!ParseProcessStat("-123 (sh)" + Stat().substr(Stat().rfind(')') + 1), identity), "negative PID rejected");
    printf("%u checks passed\n", checks); return 0;
}
