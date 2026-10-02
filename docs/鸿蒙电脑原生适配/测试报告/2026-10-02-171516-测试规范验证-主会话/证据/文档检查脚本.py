"""复现本报告的中文文档、链接、Shell、嵌入 Python 和 TOML 示例检查。"""

from pathlib import Path
import re
import subprocess
import tomllib
from urllib.parse import unquote


root = next(
    p for p in Path(__file__).resolve().parents if (p / "codex-rs/rust-toolchain.toml").is_file()
)
documents = sorted((root / "docs/鸿蒙电脑原生适配").rglob("*.md"))
documents.append(root / "docs/鸿蒙电脑原生适配可行性与设计方案.md")
counts = {"中文文档": len(documents), "本地链接": 0, "Shell 示例": 0, "Python 示例": 0, "TOML 示例": 0}
for path in documents:
    content = path.read_text()
    assert re.search(r"[\u4e00-\u9fff]", path.stem), path
    assert content.count("```") % 2 == 0, path
    assert all(not line.endswith(" ") for line in content.splitlines()), path
    for target in re.findall(r"\]\(([^)]+)\)", content):
        if "://" in target or target.startswith("#"):
            continue
        local = unquote(target.split("#")[0])
        assert (path.parent / local).exists(), (path, local)
        counts["本地链接"] += 1
    for language, body in re.findall(r"```([^\n]*)\n(.*?)```", content, re.S):
        language = language.strip()
        if language in ("sh", "bash"):
            result = subprocess.run([language, "-n"], input=body, text=True, capture_output=True)
            assert result.returncode == 0, (path, result.stderr)
            counts["Shell 示例"] += 1
            for script in re.findall(r"<<'PY'[^\n]*\n(.*?)\nPY\b", body, re.S):
                compile(script, str(path), "exec")
                counts["Python 示例"] += 1
        elif language == "toml":
            tomllib.loads(body)
            counts["TOML 示例"] += 1
for label, count in counts.items():
    print(f"{label}：{count}")
print("文档静态检查通过；示例语法检查不会执行示例中的构建或安装命令。")
