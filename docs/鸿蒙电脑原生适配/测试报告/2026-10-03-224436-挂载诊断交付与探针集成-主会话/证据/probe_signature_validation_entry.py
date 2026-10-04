from pathlib import Path
import json, os, subprocess, sys
root=Path.cwd()
sys.path.insert(0,str(root/'scripts'))
from build_harmony_distribution import build_runtime_probe, package_version
from harmony_elf import inspect_ohos_elf
from sign_harmony import check_signature
output=root/'.harmony-build/挂载修复'/sys.argv[1]
output.mkdir(exist_ok=False)
commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
sdk=Path('/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native')
java='/opt/homebrew/opt/openjdk@17/bin/java'
path, record=build_runtime_probe(output,sdk=sdk,java=java,env=dict(os.environ),commit=commit,version=package_version('0.160.0-dev',commit),runtime_base='/data/storage/el2/base/files')
info=inspect_ohos_elf(path)
assert info['签名节存在'] and info['OHOS标识'] and info['架构']=='AArch64'
assert commit.encode() in path.read_bytes()
assert b'/data/storage/el2/base/files' in path.read_bytes()
assert b'native-worker-_Exit' in path.read_bytes()
assert record['编译时运行根']=='/data/storage/el2/base/files'
assert info['动态依赖']==['libc.so']
(output/'签名信息.txt').write_text(check_signature(path,sdk=sdk,java=java))
(output/'探针验收.json').write_text(json.dumps({'源码':commit,'构建与签名':record,'ELF':info,'目标运行':'未执行'},ensure_ascii=False,indent=2)+'\n')
print('实际 OHOS C11 编译、版本身份、裁剪、自签名及 SDK 签名信息检查通过；未执行目标 ELF。')
