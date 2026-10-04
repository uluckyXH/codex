"""Exercise the exact fixed startup body, without compiling or device access."""
import ast
import errno
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import stat
import tempfile
import time

ROOT = Path('/Volumes/MacSSD/Repositories/codex')
SOURCE = ROOT/'harmony/terminal-host/entry/src/main/cpp/pty_session.cpp'
match = re.search(r'const char script\[\] = (.*?);\n', SOURCE.read_text(), re.S)
BODY = ''.join(ast.literal_eval(value) for value in re.findall(r'"(?:\\.|[^"\\])*"', match.group(1)))


def check_case(folder, kind):
    home = folder/'empty-home'
    home.mkdir(mode=0o700)
    work = folder/'中文 space "quote" $(touch injected) `touch escaped`'
    work.mkdir(mode=0o700)
    startup = folder/'private-startup'
    startup.write_text(BODY)
    startup.chmod(0o600)
    if kind == 'missing-startup':
        startup.unlink()
    cwd = work if kind != 'invalid-cwd' else folder/'does-not-exist'
    master, slave = pty.openpty()
    reader, writer = os.pipe()
    child = os.fork()
    if child == 0:
        os.close(reader)
        os.close(master)
        os.login_tty(slave)
        os.chdir(folder)
        os.dup2(writer, 3)
        if writer != 3:
            os.close(writer)
        os.execve('/bin/sh', ['/bin/sh', '-i', '-s'],
                  {'ENV': str(startup), 'CODEX_TERMINAL_CWD': str(cwd), 'HOME': str(home), 'PATH': '/usr/bin:/bin',
                   'TERM': 'xterm-256color', 'LANG': 'en_US.UTF-8'})
    os.close(slave)
    os.close(writer)
    os.set_blocking(master, False)
    os.set_blocking(reader, False)
    parent_cwd = os.getcwd()
    ready = bytearray()
    output = bytearray()
    status = None
    handshake_eof = False

    def collect(duration):
        nonlocal status, handshake_eof
        until = time.monotonic()+duration
        while time.monotonic() < until:
            fds = [master] + ([] if handshake_eof else [reader])
            available, _, _ = select.select(fds, [], [], min(0.05, max(0, until-time.monotonic())))
            for fd in available:
                try:
                    data = os.read(fd, 4096)
                except OSError as error:
                    if error.errno not in (errno.EAGAIN, errno.EIO):
                        raise
                    continue
                if fd == reader:
                    if data:
                        ready.extend(data)
                    else:
                        handshake_eof = True
                elif data:
                    output.extend(data)
            if status is None:
                waited, value = os.waitpid(child, os.WNOHANG)
                if waited == child:
                    status = value
            if status is not None and handshake_eof:
                break

    try:
        collect(0.6)
        alive_before_input = status is None
        initialized = ready == b'ready\n'
        if kind == 'success':
            assert initialized and handshake_eof and alive_before_input
            # Tests umask/cwd/positional cleanup and an external command after
            # startup; fixture commands are sent only after acknowledgement.
            commands = b'printf "\\nFIXTURE_CWD=<%s>\\nFIXTURE_ARGC=%s\\nFIXTURE_ENV=<%s>\\nFIXTURE_CWD_ENV=<%s>\\n" "$PWD" "$#" "${ENV-unset}" "${CODEX_TERMINAL_CWD-unset}"\n: > private-file\nmkdir private-directory\nexit\n'
            os.write(master, commands)
        elif alive_before_input:
            # No user work is performed in an uninitialized shell.
            os.write(master, b'exit\n')
        collect(2)
        result = {'case': kind, 'ready': initialized, 'handshake_closed': handshake_eof,
                  'alive_before_input_after_600ms': alive_before_input,
                  'exit_code': os.waitstatus_to_exitcode(status) if status is not None else None,
                  'parent_cwd_unchanged': os.getcwd() == parent_cwd}
        if kind == 'success':
            result['cwd_verified'] = ('FIXTURE_CWD=<' + str(work) + '>').encode() in output
            result['arguments_consumed'] = b'FIXTURE_ARGC=0' in output
            result['env_cleared'] = b'FIXTURE_ENV=<unset>' in output
            result['cwd_env_cleared'] = b'FIXTURE_CWD_ENV=<unset>' in output
            if not (work/'private-file').exists():
                print(json.dumps({'result':result,'output':output.decode('utf-8',errors='replace'),
                                  'files':[str(name.relative_to(folder)) for name in folder.rglob('*')]},ensure_ascii=False))
            result['file_mode'] = oct(stat.S_IMODE((work/'private-file').stat().st_mode))
            result['directory_mode'] = oct(stat.S_IMODE((work/'private-directory').stat().st_mode))
            result['no_cwd_command_execution'] = not (folder/'injected').exists() and not (folder/'escaped').exists()
            assert all(result[name] for name in ['cwd_verified','arguments_consumed','env_cleared','cwd_env_cleared','no_cwd_command_execution','parent_cwd_unchanged'])
            assert result['file_mode'] == '0o600' and result['directory_mode'] == '0o700'
            assert result['exit_code'] == 0
        else:
            assert not initialized
            if kind == 'invalid-cwd':
                assert result['exit_code'] == 125
        return result
    finally:
        if status is None:
            try:
                os.kill(child, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(child, 0)
        os.close(master)
        os.close(reader)


with tempfile.TemporaryDirectory(prefix='codex-direct-sh-check-') as temporary:
    results = []
    for kind in ['success','missing-startup','invalid-cwd']:
        folder = Path(temporary)/kind
        folder.mkdir(mode=0o700)
        results.append(check_case(folder, kind))
report = {'platform':'macOS', 'shell':'/bin/sh', 'native_cpp_executed':False,
          'device_operations':False, 'source_sha256':hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
          'cases':results}
Path(__file__).with_name('直接Shell初始化结果.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
print(json.dumps(report,ensure_ascii=False,indent=2))
