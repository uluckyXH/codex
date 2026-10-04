from pathlib import Path
import hashlib,json,os,subprocess,sys,tarfile
root=Path.cwd()
sys.path.insert(0,str(root/'scripts'))
from harmony_elf import inspect_ohos_elf
from sign_harmony import check_signature
batch=Path(sys.argv[1]); output=batch/'发布产物'
summary=json.loads((output/'交付摘要.json').read_text())
archive=output/summary['归档']
def digest(path):
 with path.open('rb') as f: return hashlib.file_digest(f,'sha256').hexdigest()
assert digest(archive)==summary['SHA-256']
unpacked=batch/'解包验证'; unpacked.mkdir()
with tarfile.open(archive) as stream: stream.extractall(unpacked,filter='data')
package=unpacked/'鸿蒙Codex'; manifest=json.loads((package/'codex-package.json').read_text())
assert manifest['target']=='aarch64-unknown-linux-ohos'
assert manifest['entrypoint']=='bin/codex'
assert not (package/'bin/codex-code-mode-host').exists()
assert not (package/'codex-resources/zsh').exists()
expected={}
for line in (package/'文件校验清单.sha256').read_text().splitlines():
 checksum,name=line.split('  ',1); path=package/name
 assert path.is_file() and not path.is_symlink()
 assert digest(path)==checksum, name
 expected[name]=checksum
assert {str(p.relative_to(package)) for p in package.rglob('*') if p.is_file()}==set(expected)|{'文件校验清单.sha256'}
records={}; sdk=Path('/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native')
for relative in ('bin/codex','codex-path/rg','codex-resources/bwrap'):
 path=package/relative; records[relative]=inspect_ohos_elf(path)
 records[relative]['官方签名信息检查']=check_signature(path,sdk=sdk,java='/opt/homebrew/opt/openjdk@17/bin/java')
 assert os.access(path,os.X_OK)
 result=subprocess.run([str(sdk/'llvm/bin/llvm-readelf'),'-h','-l','-d','-S',str(path)],text=True,capture_output=True,check=True)
 (batch/(Path(relative).name+'-静态检查.txt')).write_text(result.stdout+result.stderr)
record=json.loads((package/'构建与签名记录.json').read_text())
assert expected['codex-resources/bwrap']==record['编入CLI的沙箱摘要']
assert record['源码']['提交']=='bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9'
prefix=batch/"模拟安装/中文 路径'字面$变量"
result=subprocess.run(['/bin/sh',str(package/'安装.sh'),'--prefix',str(prefix)],text=True,capture_output=True)
print(result.stdout); print(result.stderr,file=sys.stderr); assert result.returncode==0
for name,checksum in expected.items(): assert digest(prefix/name)==checksum,name
path=subprocess.check_output(['/bin/sh','-c','. "$1"; command -v codex','sh',str(prefix/'环境.sh')],text=True).strip()
assert path==str(prefix/'bin/codex'),path
results={'归档':summary,'签名ELF':records,'包文件数':len(expected)+1,'安装后PATH解析':path,'校验':'解包及安装前后所有清单文件摘要一致','边界':'仅 Mac 文件安装模拟、静态 ELF 与官方签名信息检查；未执行目标程序、CLI、设备探针或认证测试'}
(batch/'交付验证结果.json').write_text(json.dumps(results,ensure_ascii=False,indent=2)+'\n')
print(json.dumps({k:v for k,v in results.items() if k!='签名ELF'},ensure_ascii=False,indent=2))
