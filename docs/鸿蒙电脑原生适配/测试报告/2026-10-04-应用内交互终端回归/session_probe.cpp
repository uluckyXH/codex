#include "pty_session.h"
#include <chrono>
#include <cstdio>
#include <string>
#include <thread>
using Clock=std::chrono::steady_clock;
static bool ok(const std::string &value){return value.find("\"ok\":true")!=std::string::npos;}
static std::string decode(const std::string &value) {
    const std::string marker="\"dataBase64\":\"";
    size_t start=value.find(marker);if(start==std::string::npos)return "";start+=marker.size();
    size_t end=value.find('"',start);if(end==std::string::npos)return "";
    const std::string table="ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string result;unsigned buffer=0;int bits=0;
    for(size_t index=start;index<end;index++) {
        if(value[index]=='=')break;
        size_t digit=table.find(value[index]);if(digit==std::string::npos)return "";
        buffer=(buffer<<6)|static_cast<unsigned>(digit);bits+=6;
        if(bits>=8){bits-=8;result+=static_cast<char>((buffer>>bits)&255);}
    }
    return result;
}
static bool wait_for(const char *marker,std::string &output) {
    auto until=Clock::now()+std::chrono::seconds(5);
    do {
        std::string response=codex_hnp::PtyRead();
        if(!ok(response)){std::printf("read_failed=%s\n",response.c_str());return false;}
        output+=decode(response);
        if(output.find(marker)!=std::string::npos)return true;
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }while(Clock::now()<until);
    std::printf("wait_marker_failed=%s status=%s\n",marker,codex_hnp::PtyStatus().c_str());return false;
}
static bool wait_exit(const char *expected) {
    auto until=Clock::now()+std::chrono::seconds(4);
    do {
        std::string value=codex_hnp::PtyStatus();
        if(value.find("\"running\":false")!=std::string::npos){std::printf("exit_status=%s\n",value.c_str());return value.find(expected)!=std::string::npos;}
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }while(Clock::now()<until);
    return false;
}
static int run() {
    using namespace codex_hnp;
    if(ok(PtyStart(0,36)))return 1;
    std::string started=PtyStart(120,36);std::printf("start=%s\n",started.c_str());
    if(!ok(started))return 2;
    if(ok(PtyStart(120,36)))return 3;
    std::string output;
    if(!wait_for("SESSION_CHILD_READY",output))return 4;
    if(ok(PtyWrite(std::string(16385,'X'))))return 5;
    if(!ok(PtyWrite("K")) || !wait_for("SESSION_GOT_K",output))return 6;
    std::string invalidResize=PtyResize(1001,44), validResize=PtyResize(132,44);
    std::printf("invalid_resize=%s\nvalid_resize=%s\n",invalidResize.c_str(),validResize.c_str());
    if(ok(invalidResize) || !ok(validResize))return 7;
    if(!ok(PtyWrite("R")) || !ok(PtyWrite(std::string(16384,'X'))))return 8;
    if(!wait_for("SESSION_BULK_16384",output))return 9;
    if(!ok(PtyWrite("Q")) || !wait_for("SESSION_CHILD_PASS",output) || !wait_exit("\"exitCode\":37"))return 10;
    std::printf("normal_exchange_bytes=%zu\n",output.size());
    if(!ok(PtyStart(120,36)))return 11;
    output.clear();
    if(!wait_for("SESSION_CHILD_READY",output) || !ok(PtyWrite("M")) || !wait_for("SESSION_HOLDING",output))return 12;
    bool rejected=false;
    for(int count=0;count<32;count++) {
        std::string value=PtyWrite(std::string(16384,'X'));
        if(!ok(value)){rejected=value.find("input_queue_full")!=std::string::npos;std::printf("queue_rejection=%s\n",value.c_str());break;}
    }
    if(!rejected)return 13;
    if(!ok(PtyStop()) || !wait_exit("\"signal\":9"))return 14;
    std::string final=PtyRead();
    std::printf("final_read=%s\n",final.c_str());
    return 0;
}
int main(){int result=run();codex_hnp::PtyStop();std::printf("PTY_SESSION_PROBE_%s result=%d\n",result?"FAIL":"PASS",result);return result;}
