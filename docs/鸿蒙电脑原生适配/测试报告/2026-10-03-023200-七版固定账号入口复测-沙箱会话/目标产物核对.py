from pathlib import Path
import hashlib,json,re,subprocess
root=Path(__file__).resolve().parents[4]
report=Path(__file__).parent
run=root/'.harmony-build/测试批次'/report.name
readelf=Path('/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native/llvm/bin/llvm-readelf')
artifacts=[]
for log in sorted((run/'日志').glob('交叉03-*.log')):
    text=log.read_text()
    for executable in re.findall(r'Executable unittests src/lib.rs \((.+)\)',text):
        path=root/executable
        command=[str(readelf),'-h','-l','-d',str(path)]
        result=subprocess.run(command,text=True,capture_output=True,check=True)
        assert 'AArch64' in result.stdout and 'ELF64' in result.stdout,result.stdout
        interpreter=re.search(r'Requesting program interpreter: ([^\]]+)',result.stdout).group(1)
        needed=re.findall(r'Shared library: \[([^\]]+)\]',result.stdout)
        artifacts.append({'文件':str(path),'大小':path.stat().st_size,'SHA256':hashlib.sha256(path.read_bytes()).hexdigest(),'ELF':'ELF64 AArch64','解释器':interpreter,'动态依赖':needed,'命令':command,'工具退出码':result.returncode,'运行':'未执行、未签名'})
assert len(artifacts)==2,artifacts
(report/'目标产物摘要.json').write_text(json.dumps(artifacts,ensure_ascii=False,indent=2)+'\n')
for item in artifacts:
    print(Path(item['文件']).name,item['ELF'],item['解释器'],item['动态依赖'])

libc=readelf.parents[2]/'sysroot/usr/lib/aarch64-linux-ohos/libc.so'
command=[str(readelf),'--dyn-syms',str(libc)]
result=subprocess.run(command,text=True,capture_output=True,check=True)
required=['getpwuid_r','mkdirat','openat']
exports=[]
for name in required:
    found=any(len(fields:=line.split())>=8 and fields[-1]==name and fields[-2]!='UND' for line in result.stdout.splitlines())
    assert found,name
    exports.append(name)
(report/'系统符号核对.json').write_text(json.dumps({'SDK库':str(libc),'命令':command,'退出码':result.returncode,'已导出符号':exports,'边界':'仅 SDK 动态符号存在性检查，不代表商业设备运行行为'},ensure_ascii=False,indent=2)+'\n')
print('SDK libc 已导出:', ', '.join(exports))
