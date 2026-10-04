from pathlib import Path
import os, shlex, subprocess, sys

repo = Path.cwd()
sys.path.insert(0, str(repo / "scripts"))
from build_harmony import TARGET, build_environment, native_sdk

root = Path(__file__).resolve().parent
sdk = native_sdk(Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native"))
output = root / "归档探针"
env = build_environment(sdk, output, dict(os.environ))
source = output / "库.c"
source.write_text("int harmony_archive_probe(void) { return 42; }\n")
main = output / "入口.c"
main.write_text("int harmony_archive_probe(void); int main(void) { return harmony_archive_probe() != 42; }\n")
obj, archive, binary = output / "库.o", output / "libprobe.a", output / "归档链接探针"
cc, ar, ranlib = [env[f"{name}_{TARGET}"] for name in ("CC", "AR", "RANLIB")]
for cmd in [[cc, "-c", str(source), "-o", str(obj)], [ar, "qc", str(archive), str(obj)], [ranlib, str(archive)], [cc, str(main), str(archive), "-o", str(binary)]]:
    print("执行：" + shlex.join(cmd), flush=True)
    subprocess.run(cmd, env=env, check=True)
header = subprocess.check_output([str(sdk / "llvm/bin/llvm-readelf"), "-h", str(binary)], text=True)
assert "AArch64" in header and "ELF64" in header
print("目标归档可用于 ELF64/AArch64 最终链接；未运行该探针。")
