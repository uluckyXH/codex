from pathlib import Path
import os,re,subprocess,json
root=Path.cwd();work=root/'.harmony-build/挂载修复';docs=root/'docs/鸿蒙电脑原生适配';fixtures=work/'文档命令夹具';fixtures.mkdir(exist_ok=False)
results=[]
for name in ('接口密钥安装速用.md','账号登录安装速用.md','升级与专项日志速用.md','挂载修复候选安装与复测.md'):
 text=(docs/name).read_text();blocks=re.findall(r'```(?:sh|zsh)\n(.*?)\n```',text,re.S)
 for block in blocks:subprocess.run(['/bin/zsh','-n'],input=block,text=True,check=True)
 install=next(b for b in blocks if 'set -- 鸿蒙Codex-' in b)
 for count in (0,1,2):
  task=fixtures/(name.removesuffix('.md')+'-'+str(count));task.mkdir();fake=task/'命令';fake.mkdir();marker=task/'实际调用.txt'
  copied=task/('鸿蒙Codex更新-挂载修复' if name=='升级与专项日志速用.md' else '鸿蒙Codex安装-挂载修复');copied.mkdir()
  for i in range(count):(copied/f'鸿蒙Codex-{i}-未真机验证.tar.gz').write_text('fixture only')
  for command,body in [('sha256sum','exit 0'),('shasum','exit 0'),('tar','printf tar >> "$harmony_fixture_marker"'),('sh','printf install >> "$harmony_fixture_marker"')]:
   file=fake/command;file.write_text('#!/bin/sh\n'+body+'\n');file.chmod(0o755)
  env=dict(os.environ,harmony_fixture_root=str(task),harmony_fixture_marker=str(marker));env['PATH']=str(fake)+':'+env.get('PATH','')
  result=subprocess.run(['/bin/sh'],input=install.replace('$HOME','$harmony_fixture_root'),text=True,env=env,capture_output=True)
  assert (result.returncode==0)==(count==1),(name,count,result.stderr)
  assert marker.exists()==(count==1),(name,count,'unexpected extraction/installation')
  results.append({'用例':name+' 压缩包数量 '+str(count),'退出码':result.returncode,'提取与安装是否触发':marker.exists()})
text=(docs/'升级与专项日志速用.md').read_text();blocks=re.findall(r'```sh\n(.*?)\n```',text,re.S)
trace=next(b for b in blocks if 'harmony_trace_dir=' in b);summary=next(b for b in blocks if 'for harmony_log_file' in b)
for mode in ('正常捕获159','目录已存在','切目录失败','终端dumb'):
 task=fixtures/mode;task.mkdir();fake=task/'命令';fake.mkdir();marker=task/'Codex调用标记.txt';tracepath=task/'Codex诊断/固定'
 binary=fake/'codex';binary.write_text('#!/bin/sh\nprintf "%s" "$PWD" > "$harmony_fixture_marker"\nprintf "%s\n" "[codex-process] stage=pipe-wait raw_wait_status=40704 signal=None" >&2\nexit 159\n');binary.chmod(0o755)
 env=dict(os.environ,harmony_fixture_root=str(task),harmony_fixture_marker=str(marker),TERM='dumb' if mode=='终端dumb' else 'xterm-256color');env['PATH']=str(fake)+':'+env.get('PATH','')
 block=trace.replace('$HOME','$harmony_fixture_root');block=re.sub(r'^harmony_trace_dir=.*$', 'harmony_trace_dir="$harmony_fixture_root/Codex诊断/固定"',block,flags=re.M)
 if mode=='目录已存在':tracepath.mkdir(parents=True)
 if mode=='切目录失败':block='cd() { return 1; }\n'+block
 result=subprocess.run(['/bin/sh'],input=block,text=True,env=env,capture_output=True)
 if mode=='正常捕获159':
  assert result.returncode==159 and marker.read_text()==str(tracepath/'空白项目')
  assert 'raw_wait_status=40704' in (tracepath/'进程专项.txt').read_text()
  assert (tracepath.stat().st_mode & 0o777)==0o700
  env['harmony_trace_dir']=str(tracepath)
  extracted=subprocess.run(['/bin/sh'],input=summary,text=True,env=env,capture_output=True);assert extracted.returncode==0
  assert 'raw_wait_status=40704' in extracted.stdout and '不存在或不可读' in extracted.stdout
  (tracepath/'交互日志/codex-tui.log').write_text('fs.sandbox_prepare\nOperation not permitted (os error 1)\n错误续行\n')
  extracted=subprocess.run(['/bin/sh'],input=summary,text=True,env=env,capture_output=True);assert extracted.returncode==0
  assert 'fs.sandbox_prepare' in extracted.stdout and '错误续行' in extracted.stdout
 else:
  assert result.returncode!=0 and not marker.exists(),(mode,result.stdout,result.stderr)
 results.append({'用例':mode,'退出码':result.returncode,'Codex是否触发':marker.exists()})
(work/'文档命令验收.json').write_text(json.dumps({'场景数':len(results),'结果':results,'边界':'Mac Shell 模拟，提取实际教程命令，HOME 路径替换为专用夹具变量；不修改 HOME，不执行真正 Codex、认证、模型或鸿蒙 ELF。'},ensure_ascii=False,indent=2)+'\n')
print(json.dumps({'场景数':len(results),'结果':'全部通过','边界':'Mac命令夹具，不是真机测试'},ensure_ascii=False))
