"""No compilation or device access. macOS shell/PTY evidence only."""
import errno
import json
import os
from pathlib import Path
import pty
import select
import signal
import tempfile
import termios
import time


def describe(settings):
    def value(index):
        entry = settings[6][index]
        return entry if isinstance(entry, int) else entry[0]
    return {
        "icanon": bool(settings[3] & termios.ICANON),
        "isig": bool(settings[3] & termios.ISIG),
        "vmin": value(termios.VMIN), "vtime": value(termios.VTIME),
    }


def run_case(working, private_home, zero_minimum):
    master, slave = pty.openpty()
    initial = termios.tcgetattr(slave)
    if zero_minimum:
        injected = termios.tcgetattr(slave)
        injected[3] &= ~termios.ICANON
        injected[6][termios.VMIN] = 0
        injected[6][termios.VTIME] = 0
        termios.tcsetattr(slave, termios.TCSANOW, injected)
    launched = termios.tcgetattr(slave)
    child = os.fork()
    if child == 0:
        os.close(master)
        os.login_tty(slave)
        bootstrap = 'umask 077; cd "$1" || exit 125; shift; exec "$@"'
        os.execve('/bin/sh', ['/bin/sh', '-c', bootstrap, 'codex-terminal',
                             str(working), '/bin/sh', '-i'],
                  {'HOME': str(private_home), 'PATH': '/usr/bin:/bin',
                   'TERM': 'xterm-256color', 'LANG': 'en_US.UTF-8'})
    os.close(slave)
    os.set_blocking(master, False)
    output = bytearray()
    status = None
    start = time.monotonic()

    def collect(duration):
        nonlocal status
        until = time.monotonic() + duration
        while time.monotonic() < until:
            ready, _, _ = select.select([master], [], [], min(0.05, max(0, until-time.monotonic())))
            if ready:
                try:
                    data = os.read(master, 4096)
                    if data:
                        output.extend(data)
                except OSError as error:
                    if error.errno not in (errno.EAGAIN, errno.EIO):
                        raise
            if status is None:
                waited, value = os.waitpid(child, os.WNOHANG)
                if waited == child:
                    status = value
            if status is not None:
                break

    try:
        collect(0.6)
        stayed_alive = status is None
        if stayed_alive:
            os.write(master, b'printf "\\nFIXTURE_CWD=<%s>\\n" "$PWD"\nexit\n')
            collect(2)
        observed = {'case': 'injected_noncanonical_vmin0' if zero_minimum else 'default_canonical',
                    'initial': describe(initial), 'launch': describe(launched),
                    'alive_before_input_after_600ms': stayed_alive,
                    'exit_code': os.waitstatus_to_exitcode(status) if status is not None else None,
                    'cwd_verified': ('FIXTURE_CWD=<' + str(working) + '>').encode() in output,
                    'output_bytes': len(output),
                    'elapsed_ms': round((time.monotonic()-start)*1000)}
        if not zero_minimum:
            assert stayed_alive, observed
            assert observed['exit_code'] == 0, observed
            assert observed['cwd_verified'], observed
        return observed
    finally:
        if status is None:
            # Only the directly created fixture child is signalled/reaped.
            try:
                os.kill(child, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(child, 0)
        os.close(master)


with tempfile.TemporaryDirectory(prefix='codex-pty-source-check-') as temporary:
    workspace = Path(temporary)/'中文 space "quote"'
    home = Path(temporary)/'empty-home'
    workspace.mkdir(mode=0o700)
    home.mkdir(mode=0o700)
    cases = [run_case(workspace, home, injected) for injected in (False, True)]
result = {'platform': 'macOS', 'shell': '/bin/sh', 'native_source_executed': False,
          'ohos_device_test': False, 'cases': cases}
Path(__file__).with_name('本机PTY对照结果.json').write_text(json.dumps(result, ensure_ascii=False, indent=2)+'\n')
print(json.dumps(result, ensure_ascii=False, indent=2))
