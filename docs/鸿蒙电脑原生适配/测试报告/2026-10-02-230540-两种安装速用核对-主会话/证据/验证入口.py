from pathlib import Path
import hashlib,json,os,re,shlex,shutil,subprocess,sys,tarfile,tomllib
root=Path.cwd();docs=root/'docs/鸿蒙电脑原生适配';batch=Path((root/'.harmony-build/速用文档/本批路径.txt').read_text().strip());delivery=root/'.harmony-build/交付/鸿蒙Codex安装'
names=['账号登录安装速用.md','接口密钥安装速用.md']
def blocks(name): return re.findall(r'```([^\n]+)\n(.*?)```',(docs/name).read_text(),re.S)
def sha(path):
 with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
checksum,filename=(docs/'安装包校验.sha256').read_text().rstrip('\n').split('  ',1)
if sys.argv[1]=='static':
 shell_count=config_count=0
 for name in names:
  assert (docs/name).read_bytes()==(delivery/name).read_bytes()
  for language,body in blocks(name):
   if language in ['sh','zsh']:
    args=['/bin/zsh','-f','-n'] if language=='zsh' else ['/bin/sh','-n']
    r=subprocess.run(args,input=body,text=True,capture_output=True);assert r.returncode==0,(name,r.stderr);shell_count+=1
    for cfg in re.findall(r"<<'TOML'\n(.*?)\nTOML",body,re.S):
     parsed=tomllib.loads(cfg);assert parsed['cli_auth_credentials_store']=='file';assert parsed['approval_policy']=='untrusted';assert parsed['sandbox_mode']=='workspace-write'
     if 'model_provider' in parsed:
      provider=parsed['model_providers'][parsed['model_provider']];assert provider['wire_api']=='responses';assert provider['requires_openai_auth'] is False;assert provider['env_key']=='HARMONY_CODEX_API_KEY'
     config_count+=1
  assert filename in (docs/name).read_text()
 assert len(list(delivery.iterdir()))==4
 assert (delivery/'安装包校验.sha256').read_bytes()==(docs/'安装包校验.sha256').read_bytes()
 assert sha(delivery/filename)==checksum
 print(f'Shell/zsh 语法检查 {shell_count} 段；TOML 解析 {config_count} 段；四份交付文件匹配，归档摘要正确。')
 print('仅解析与文件摘要，不执行配置、认证、凭据读写或模型请求。')
elif sys.argv[1]=='install':
 selected=[]
 for name in names:
  candidate=[body for _,body in blocks(name) if 'tar -xzf' in body or 'command -v codex' in body];assert len(candidate)==2;selected.append(candidate)
 assert selected[0]==selected[1], '两份教程公共安装步骤应完全一致'
 simulated=batch/'临时目录/用户 中文';simulated.mkdir(exist_ok=False)
 shutil.copytree(delivery,simulated/'鸿蒙Codex安装')
 script='harmony_doc_home='+shlex.quote(str(simulated))+'\n'+'\n'.join(selected[0]).replace('$HOME','$harmony_doc_home')
 assert 'codex login' not in script and '--version' not in script and 'config.toml' not in script
 result=subprocess.run(['/bin/zsh','-f'],input=script,text=True,capture_output=True)
 print(result.stdout);print(result.stderr,file=sys.stderr);assert result.returncode==0
 prefix=simulated/'应用工具/鸿蒙Codex';entries=(prefix/'文件校验清单.sha256').read_text().splitlines()
 for entry in entries:
  expected,name=entry.split('  ',1);assert sha(prefix/name)==expected,name
 for relative in ['bin/codex','codex-path/rg','codex-resources/bwrap']:assert os.access(prefix/relative,os.X_OK)
 verify='harmony_doc_home='+shlex.quote(str(simulated))+'\n. "$harmony_doc_home/.zshrc"\ncommand -v codex\n'
 found=subprocess.check_output(['/bin/zsh','-f','-c',verify],text=True).strip();assert found==str(prefix/'bin/codex'),found
 print('实际文档的共同安装与 PATH 段执行通过；清单中',len(entries),'个文件摘要及执行位一致。')
 print('新 Shell 从模拟启动配置定位到：',found)
 print('没有执行鸿蒙 ELF、配置创建、登录或模型请求；HOME/CODEX_HOME 环境变量未修改。')
else:raise SystemExit('unknown action')
