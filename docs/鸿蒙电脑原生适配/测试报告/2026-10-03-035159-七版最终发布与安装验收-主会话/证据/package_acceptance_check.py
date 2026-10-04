from pathlib import Path
import hashlib, json, os, shutil, subprocess, sys, tarfile
root=Path.cwd(); sys.path.insert(0,str(root/'scripts'))
from harmony_elf import inspect_ohos_elf
from sign_harmony import check_signature
from build_harmony_distribution import digest
release=root/'releases/2026-10-03-鸿蒙PC七版修复候选'
summary=json.loads((release/'交付摘要.json').read_text())
archive=release/summary['归档']
assert digest(archive)==summary['SHA-256']
inspection=root/'.harmony-build/七版发布/独立包验收'
inspection.mkdir(exist_ok=False)
with tarfile.open(archive) as tf:
    for member in tf.getmembers():
        p=Path(member.name)
        assert not p.is_absolute() and '..' not in p.parts
        assert p.parts[0]=='鸿蒙Codex' and (member.isfile() or member.isdir())
    tf.extractall(inspection,filter='data')
package=inspection/'鸿蒙Codex'
manifest=package/'文件校验清单.sha256'
entries=[]
for line in manifest.read_text().splitlines():
    checksum,name=line.split('  ',1)
    assert len(checksum)==64 and Path(name).is_relative_to('.') and '..' not in Path(name).parts
    path=package/name
    assert path.is_file() and not path.is_symlink() and digest(path)==checksum, name
    entries.append(name)
assert len(entries)==len(set(entries)), 'duplicate manifest entries'
actual={p.relative_to(package).as_posix() for p in package.rglob('*') if p.is_file()}
assert actual==set(entries)|{'文件校验清单.sha256'}
record=json.loads((package/'构建与签名记录.json').read_text())
assert not record['源码']['工作区状态']
assert record['源码']['提交']==json.loads((root/'.harmony-build/七版发布/执行记录/最终冻结源码.json').read_text())['源码']
assert record['版本']==summary['版本']
sdk=Path('/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native')
java='/opt/homebrew/opt/openjdk@17/bin/java'
elf={}
for name in ('bin/codex','codex-path/rg','codex-resources/bwrap'):
    info=inspect_ohos_elf(package/name)
    assert info['SHA-256']==record['文件'][name]['SHA-256']
    assert info['签名节存在'] and os.access(package/name,os.X_OK)
    signature=check_signature(package/name,sdk=sdk,java=java)
    (inspection/(Path(name).name+'签名信息.txt')).write_text(signature)
    elf[name]=info
assert elf['codex-resources/bwrap']['SHA-256']==record['编入CLI的沙箱摘要']
assert record['源码']['提交'].encode() in (package/'bin/codex').read_bytes(), 'missing compiled build identity'
for name in ('安装.sh','启用终端.sh','诊断.sh'):
    assert (package/name).read_bytes()==(root/'scripts/harmony'/name).read_bytes()
    subprocess.run(['sh','-n',str(package/name)],check=True)
prefix=inspection/'用户程序 空格'/"鸿蒙Codex'候选"
install=subprocess.run(['sh',str(package/'安装.sh'),'--prefix',str(prefix)],capture_output=True,text=True)
(inspection/'文件安装输出.txt').write_text(install.stdout+install.stderr)
assert install.returncode==0,install.stderr
for name in entries: assert digest(prefix/name)==digest(package/name),name
again=subprocess.run(['sh',str(package/'安装.sh'),'--prefix',str(prefix)],capture_output=True,text=True)
assert again.returncode!=0 and '已存在' in again.stderr
rc=inspection/'夹具.zshrc'; original="typeset -a demo=(one two)\n# 用户自定义配置\n"
rc.write_text(original)
activate=['sh',str(prefix/'启用终端.sh'),'--rc-file',str(rc)]
first=subprocess.run(activate,capture_output=True,text=True);assert first.returncode==0,first.stderr
active=rc.read_bytes();assert original.encode() in active
second=subprocess.run(activate,capture_output=True,text=True);assert second.returncode==0 and rc.read_bytes()==active
shell=subprocess.run(['/bin/zsh','-f','-c','. "$1/环境.sh"; . "$1/环境.sh"; command -v codex','sh',str(prefix)],capture_output=True,text=True)
assert shell.returncode==0 and shell.stdout.strip()==str(prefix/'bin/codex'),shell.stdout+shell.stderr
remove=subprocess.run(activate+['--remove'],capture_output=True,text=True);assert remove.returncode==0 and rc.read_text()==original
# 只检查安装文件、Shell 路径解析和专用配置夹具，不执行任何 OHOS ELF。
result={'归档':archive.name,'压缩包摘要':digest(archive),'清单文件数':len(entries),'源码':record['源码']['提交'],'版本':record['版本'],'程序':elf,'结果':['归档路径与普通文件校验通过','完整清单逐项 SHA256 通过','三个 OHOS ELF 与 SDK 签名信息检查通过','CLI 含冻结源码身份','包内 bwrap 与编入摘要一致','实际包安装到中文/空格/单引号路径通过','现有目录拒绝覆盖','终端启用幂等与撤销通过','Shell 路径解析通过'],'设备运行':'未执行；没有运行登录、模型调用、目标 ELF 或用户认证文件测试'}
(inspection/'验收结果.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
print(json.dumps(result,ensure_ascii=False,indent=2))
