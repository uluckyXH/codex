from pathlib import Path
import hashlib,json,tarfile
root=Path.cwd();w=root/'.harmony-build/挂载修复';release=Path((w/'本次发布目录.txt').read_text())
summary=json.loads((release/'交付摘要.json').read_text());archive=release/summary['归档']
assert hashlib.sha256(archive.read_bytes()).hexdigest()==summary['SHA-256']
with tarfile.open(archive) as tf:
    record=json.load(tf.extractfile('鸿蒙Codex/构建与签名记录.json'))
assert record['版本']==summary['版本']
summary['源码']=record['源码']['提交']
summary['源码身份来源']='压缩包内 构建与签名记录.json；交付整理时补充外层摘要，不修改压缩包'
report=root/(w/'发布报告位置.txt').read_text()
paths=[release/'交付摘要.json',release/'鸿蒙Codex安装/交付摘要.json',root/'docs/鸿蒙电脑原生适配/交付资料'/release.name/'交付摘要.json',report/'证据/交付摘要.json']
for p in paths:p.write_text(json.dumps(summary,ensure_ascii=False,indent=2)+'\n')
assert hashlib.sha256(archive.read_bytes()).hexdigest()==summary['SHA-256']
assert hashlib.sha256((release/'鸿蒙Codex安装'/archive.name).read_bytes()).hexdigest()==summary['SHA-256']
print(json.dumps({'源码':summary['源码'],'压缩包未改变':True,'摘要副本一致':len(paths)},ensure_ascii=False))
