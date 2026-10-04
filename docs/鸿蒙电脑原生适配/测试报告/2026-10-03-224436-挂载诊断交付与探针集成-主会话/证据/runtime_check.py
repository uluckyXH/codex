from pathlib import Path
from datetime import datetime
from zoneinfo import ZoneInfo
import hashlib,json,os,platform,subprocess,sys
root=Path.cwd(); name=sys.argv[1]; command=sys.argv[2:]
evidence=root/'.harmony-build/挂载修复/执行记录'; evidence.mkdir(parents=True,exist_ok=True)
log=evidence/(name+'.log')
env=dict(os.environ)
env.update(RUSTUP_HOME='/Volumes/MacSSD/dev/rust/rustup', CARGO_HOME='/Volumes/MacSSD/dev/rust/cargo', CARGO_BUILD_JOBS='2', CARGO_NET_OFFLINE='true', RUSTUP_AUTO_INSTALL='0', PYTHONDONTWRITEBYTECODE='1', TMPDIR=str(evidence/'临时目录'))
env['PYTHONPATH']=str(root/'scripts')
env['CARGO_TARGET_DIR']=str(root/'.harmony-build/目录权限修复/宿主产物')
env['PATH']='/opt/homebrew/opt/rustup/bin:'+env.get('PATH','')
Path(env['TMPDIR']).mkdir(exist_ok=True)
now=lambda:datetime.now(ZoneInfo('Asia/Shanghai')).isoformat(timespec='seconds')
start=now()
with log.open('x') as stream:
    result=subprocess.run(command,cwd=root,env=env,stdout=stream,stderr=subprocess.STDOUT)
end=now()
record={'工作区状态':subprocess.check_output(['git','status','--porcelain'],text=True),'工作区补丁摘要':hashlib.sha256(subprocess.check_output(['git','diff','HEAD'])).hexdigest(),'命令':command,'开始':start,'结束':end,'退出码':result.returncode,'主机':platform.platform(),'源码':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'日志':str(log.relative_to(root)),'日志字节数':log.stat().st_size,'日志摘要':hashlib.sha256(log.read_bytes()).hexdigest(),'并行编译数':2,'工具链与下载缓存':'外置盘 /Volumes/MacSSD/dev/rust；未运行目标程序'}
(evidence/(name+'.json')).write_text(json.dumps(record,ensure_ascii=False,indent=2)+'\n')
lines=log.read_text(errors='replace').splitlines()
print(name+' 退出码 '+str(result.returncode));print('\n'.join(lines[-25:]))
sys.exit(result.returncode)
