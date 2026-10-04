#!/usr/bin/env python3
"""编译实际启动加固源码；宿主在最小环境的独立进程执行，OHOS 只链接。"""

import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

REPORT = Path(__file__).resolve().parent
REPO = REPORT.parents[3]
RUN = REPO / ".harmony-build/测试批次" / REPORT.name


def main():
    ohos = sys.argv[1:] == ["--ohos"]
    if sys.argv[1:] not in ([], ["--ohos"]):
        raise SystemExit("只接受可选 --ohos")
    target = "aarch64-unknown-linux-ohos" if ohos else "aarch64-apple-darwin"
    target_root = RUN / ("鸿蒙构建/target" if ohos else "宿主产物")
    deps = target_root / target / "debug/deps"
    libraries = list(deps.glob("liblibc-*.rlib"))
    if len(libraries) != 1:
        raise RuntimeError(f"需要唯一的本批目标 libc rlib，实际找到 {len(libraries)} 个")
    output = RUN / ("鸿蒙启动加固探针" if ohos else "宿主启动加固探针")
    command = ["rustup", "run", "1.95.0", "rustc", "--edition=2024",
               "--crate-name", "harmony_hardening_probe", "--target", target,
               "--extern", f"libc={libraries[0]}", "-L", f"dependency={deps}"]
    if ohos:
        command += ["-C", f"linker={RUN / '鸿蒙构建/toolchain/ohos-clang'}"]
    command += [str(REPORT / "启动加固探针.rs"), "-o", str(output)]
    print("命令：" + shlex.join(command), flush=True)
    subprocess.run(command, cwd=REPO, check=True)
    print("libc SHA256：" + hashlib.sha256(libraries[0].read_bytes()).hexdigest(), flush=True)
    if not ohos:
        print("仅在独立宿主进程验证，不加载 Codex 配置或认证数据", flush=True)
        subprocess.run([str(output)], cwd=RUN, env={
            "PATH": os.environ["PATH"], "TMPDIR": os.environ["TMPDIR"],
        }, check=True)
    else:
        print("只完成 OHOS 链接，未运行目标程序", flush=True)
    metadata = {"产物": str(output), "目标": target,
                "SHA256": hashlib.sha256(output.read_bytes()).hexdigest(), "运行": not ohos}
    (REPORT / ("鸿蒙加固产物.json" if ohos else "宿主加固产物.json")).write_text(
        json.dumps(metadata, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
