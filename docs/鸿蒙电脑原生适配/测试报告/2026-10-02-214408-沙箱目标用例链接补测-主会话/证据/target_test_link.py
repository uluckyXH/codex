from pathlib import Path
import os,subprocess,sys
sys.path.insert(0,str(Path.cwd()/"scripts"))
from build_harmony import build_environment,native_sdk,TARGET,TOOLCHAIN
sdk=native_sdk(Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native"))
env=build_environment(sdk,Path(sys.argv[1]),dict(os.environ))
for name in ("HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","http_proxy","https_proxy","all_proxy"):
 env.pop(name,None)
env["CARGO_NET_OFFLINE"]="false"
subprocess.run(["rustup","run",TOOLCHAIN,"cargo","test","--manifest-path","codex-rs/Cargo.toml","--locked","--no-run","--target",TARGET,"-p","codex-linux-sandbox","-p","codex-sandboxing","--lib"],env=env,check=True)
