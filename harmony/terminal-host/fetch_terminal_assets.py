#!/usr/bin/env python3
"""Fetch pinned, integrity-checked xterm assets; no install scripts run."""
import base64
import hashlib
import io
import json
from pathlib import Path
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parent
OUTPUT = ROOT / "entry/src/main/resources/rawfile"
PACKAGES = [
    ("xterm", "https://registry.npmjs.org/@xterm/xterm/-/xterm-6.0.0.tgz", "sha512-TQwDdQGtwwDt+2cgKDLn0IRaSxYu1tSUjgKarSDkUM0ZNiSRXFpjxEsvc/Zgc5kq5omJ+V0a8/kIM2WD3sMOYg==", {"package/lib/xterm.js": "xterm.js", "package/css/xterm.css": "xterm.css", "package/LICENSE": "xterm-LICENSE.txt"}),
    ("addon-fit", "https://registry.npmjs.org/@xterm/addon-fit/-/addon-fit-0.11.0.tgz", "sha512-jYcgT6xtVYhnhgxh3QgYDnnNMYTcf8ElbxxFzX0IZo+vabQqSPAjC3c1wJrKB5E19VwQei89QCiZZP86DCPF7g==", {"package/lib/addon-fit.js": "addon-fit.js", "package/LICENSE": "addon-fit-LICENSE.txt"}),
]

def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    manifest = []
    for name, url, integrity, files in PACKAGES:
        with urllib.request.urlopen(url, timeout=30) as response:
            blob = response.read(8 * 1024 * 1024 + 1)
        if len(blob) > 8 * 1024 * 1024:
            raise ValueError("Package exceeds bound")
        expected = "sha512-" + base64.b64encode(hashlib.sha512(blob).digest()).decode()
        if expected != integrity:
            raise ValueError("Package integrity mismatch")
        record = {"name": name, "url": url, "integrity": integrity, "files": {}}
        with tarfile.open(fileobj=io.BytesIO(blob), mode="r:gz") as archive:
            for source, target in files.items():
                member = archive.getmember(source)
                if not member.isfile() or member.size > 4 * 1024 * 1024:
                    raise ValueError("Invalid package member")
                data = archive.extractfile(member).read()
                (OUTPUT / target).write_bytes(data)
                record["files"][target] = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
        manifest.append(record)
    (OUTPUT / "terminal-assets.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print("Pinned assets verified:", len(manifest), "packages")

if __name__ == "__main__":
    main()
