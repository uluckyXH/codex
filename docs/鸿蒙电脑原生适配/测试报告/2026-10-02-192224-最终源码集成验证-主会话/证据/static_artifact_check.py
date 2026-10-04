from pathlib import Path
import hashlib, json, re, subprocess

root = Path(__file__).resolve().parent
sdk = Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native")
binary = root / "鸿蒙构建/target/aarch64-unknown-linux-ohos/debug/codex"
assert binary.is_file(), f"目标文件缺失：{binary}"
header = subprocess.check_output([str(sdk / "llvm/bin/llvm-readelf"), "--wide", "-h", "-l", "-d", "-n", str(binary)], text=True)
(root / "ELF完整输出.txt").write_text(header)
assert "AArch64" in header
assert "ELF64" in header
assert "Requesting program interpreter: /lib/ld-musl-aarch64.so.1" in header
needed = re.findall(r"\(NEEDED\).*?Shared library: \[(.*?)\]", header)
paths = [line.strip() for line in header.splitlines() if "(RPATH)" in line or "(RUNPATH)" in line]
meta = {
    "文件": str(binary),
    "字节数": binary.stat().st_size,
    "SHA-256": hashlib.file_digest(binary.open("rb"), "sha256").hexdigest(),
    "file输出": subprocess.check_output(["/usr/bin/file", str(binary)], text=True).strip(),
    "解释器": "/lib/ld-musl-aarch64.so.1",
    "动态依赖": needed,
    "RPATH或RUNPATH": paths,
    "OHOS标识节": ".note.ohos.ident" in header,
    "签名与执行": "未签名，未执行；不代表商业鸿蒙 PC 接受加载器或 SDK 动态库",
}
(root / "产物摘要.json").write_text(json.dumps(meta, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(meta, ensure_ascii=False, indent=2))
