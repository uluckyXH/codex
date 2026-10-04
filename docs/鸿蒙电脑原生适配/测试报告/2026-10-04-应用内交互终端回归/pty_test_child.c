#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

static int read_byte(char *value) {
    ssize_t count;
    do { count=read(0,value,1); } while(count<0 && errno==EINTR);
    return count==1;
}
int main(int argc,char **argv) {
    int model=0,sandbox=0,approval=0;
    for(int index=1;index+1<argc;index++) {
        if(!strcmp(argv[index],"--model") && !strcmp(argv[index+1],"gpt-5.6-terra")) model=1;
        if(!strcmp(argv[index],"--sandbox") && !strcmp(argv[index+1],"read-only")) sandbox=1;
        if(!strcmp(argv[index],"--ask-for-approval") && !strcmp(argv[index+1],"on-request")) approval=1;
    }
    if(!model || !sandbox || !approval) return 61;
    if(!isatty(0) || !isatty(1) || !isatty(2) || getsid(0)!=getpid()) return 62;
    int terminal=open("/dev/tty",O_RDWR|O_CLOEXEC);
    struct termios original={0},raw={0};
    if(terminal<0 || tcgetattr(terminal,&original)) return 63;
    raw=original;cfmakeraw(&raw);
    if(tcsetattr(terminal,TCSANOW,&raw)) return 64;
    signal(SIGTERM,SIG_IGN);
    puts("SESSION_CHILD_READY");fflush(stdout);
    char value=0;
    if(!read_byte(&value)) return 65;
    if(value=='M') {
        puts("SESSION_HOLDING");fflush(stdout);
        for(;;) pause();
    }
    if(value!='K') return 66;
    puts("SESSION_GOT_K");fflush(stdout);
    if(!read_byte(&value) || value!='R') return 67;
    struct winsize size={0};
    if(ioctl(terminal,TIOCGWINSZ,&size) || size.ws_row!=44 || size.ws_col!=132) return 68;
    size_t total=0;
    while(total<16384) {
        char bytes[1024];size_t room=sizeof(bytes);if(room>16384-total)room=16384-total;
        ssize_t count=read(0,bytes,room);
        if(count<0 && errno==EINTR)continue;
        if(count<=0)return 69;
        for(ssize_t index=0;index<count;index++) if(bytes[index]!='X')return 70;
        total+=(size_t)count;
    }
    puts("SESSION_BULK_16384");fflush(stdout);
    if(!read_byte(&value) || value!='Q') return 71;
    if(tcsetattr(terminal,TCSANOW,&original)) return 72;
    close(terminal);
    puts("SESSION_CHILD_PASS");fflush(stdout);
    return 37;
}
