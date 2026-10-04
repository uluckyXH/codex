from pathlib import Path
import hashlib, re
root = Path(__file__).resolve().parents[4]
report = Path(__file__).resolve().parents[1]
root = report.parents[3]
names = ["2026-10-02-202646-鸿蒙能力约束与原生剪贴板-终端会话", "2026-10-02-203520-鸿蒙能力与剪贴板复测-终端会话", report.name]
documents = [root / "docs/鸿蒙电脑原生适配/鸿蒙能力约束与原生文本剪贴板交付.md"]
documents += [root / "docs/鸿蒙电脑原生适配/测试报告" / name / "测试报告.md" for name in names]
for document in documents:
    for target in re.findall(r"\]\(([^)]+)\)", document.read_text()):
        if target.startswith(("http://", "https://")):
            continue
        assert (document.parent / target.split("#", 1)[0]).exists(), (document, target)
for line in (report / "证据/源码摘要.txt").read_text().splitlines():
    expected, name = line.split("  ", 1)
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
print("4 份交付/报告的本地引用存在；28 个最终源文件摘要与被测版本一致。")
