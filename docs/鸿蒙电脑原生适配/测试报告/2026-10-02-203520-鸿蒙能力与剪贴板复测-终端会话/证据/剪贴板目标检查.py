from pathlib import Path
import os, sys
sys.path.insert(0, str(Path.cwd() / "scripts"))
from build_harmony import build_environment, native_sdk, run
sdk=native_sdk(Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native"))
output=Path(sys.argv[1])
env=build_environment(sdk, output, dict(os.environ))
run(["rustup", "run", "1.95.0", "cargo", "check", "--offline", "--tests", "--target", "aarch64-unknown-linux-ohos", "--manifest-path", sys.argv[2]], env)
