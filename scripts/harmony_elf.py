"""校验 OHOS ELF 的静态属性；不加载或执行被检查的程序。"""

import hashlib
from pathlib import Path
import struct


INTERPRETER = "/lib/ld-musl-aarch64.so.1"


def inspect_ohos_elf(path: Path) -> dict:
    size = path.stat().st_size
    with path.open("rb") as stream:

        def read(offset: int, count: int) -> bytes:
            if (
                offset < 0
                or count < 0
                or offset + count > size
                or count > 16 * 1024 * 1024
            ):
                raise RuntimeError(f"ELF 数据范围非法：{path}")
            stream.seek(offset)
            data = stream.read(count)
            if len(data) != count:
                raise RuntimeError(f"ELF 数据不完整：{path}")
            return data

        def string(data: bytes, offset: int) -> str:
            if not 0 <= offset < len(data):
                raise RuntimeError(f"ELF 字符串索引非法：{path}")
            end = data.find(b"\0", offset)
            if end < 0:
                raise RuntimeError(f"ELF 字符串未终止：{path}")
            return data[offset:end].decode("utf-8", errors="strict")

        header = read(0, 64)
        if header[:7] != b"\x7fELF\x02\x01\x01":
            raise RuntimeError(f"不是 ELF64 小端程序：{path}")
        fields = struct.unpack("<HHIQQQIHHHHHH", header[16:])
        (
            kind,
            machine,
            version,
            _,
            phoff,
            shoff,
            _,
            ehsize,
            phsize,
            phnum,
            shsize,
            shnum,
            shstrings,
        ) = fields
        if kind not in (2, 3) or machine != 183 or version != 1 or ehsize != 64:
            raise RuntimeError(f"不是 ARM64 ELF 可执行程序：{path}")
        if phsize != 56 or shsize != 64 or not 0 < phnum < 4096 or not 0 < shnum < 4096:
            raise RuntimeError(f"不支持的 ELF 表结构：{path}")
        if not 0 <= shstrings < shnum:
            raise RuntimeError(f"ELF 节名称表索引非法：{path}")
        sections = [
            struct.unpack("<IIQQQQIIQQ", read(shoff + i * shsize, shsize))
            for i in range(shnum)
        ]
        names_section = sections[shstrings]
        names = read(names_section[4], names_section[5])
        by_name = {string(names, section[0]): section for section in sections}
        note = by_name.get(".note.ohos.ident")
        if note is None or note[1] != 7:
            raise RuntimeError(f"缺少 OHOS 标识，拒绝混入普通 Linux 产物：{path}")
        note_bytes = read(note[4], note[5])
        if len(note_bytes) < 12:
            raise RuntimeError(f"OHOS 标识不完整：{path}")
        owner_size, descriptor_size, note_type = struct.unpack("<III", note_bytes[:12])
        if (
            owner_size < 4
            or 12 + ((owner_size + 3) & ~3) + descriptor_size > len(note_bytes)
            or note_type != 1
            or not note_bytes[12 : 12 + owner_size].startswith(b"OHOS\0")
        ):
            raise RuntimeError(f"无效的 OHOS 标识：{path}")
        interpreter = None
        for index in range(phnum):
            program = struct.unpack("<IIQQQQQQ", read(phoff + index * phsize, phsize))
            if program[0] == 3:
                interpreter = string(read(program[2], program[5]), 0)
        if interpreter != INTERPRETER:
            raise RuntimeError(f"OHOS 加载器不匹配：{path}: {interpreter!r}")
        needed = []
        rpaths = []
        dynamic = by_name.get(".dynamic")
        strings = by_name.get(".dynstr")
        if dynamic is not None:
            if strings is None or dynamic[5] % 16:
                raise RuntimeError(f"ELF 动态表不完整：{path}")
            dynstrings = read(strings[4], strings[5])
            for tag, value in struct.iter_unpack("<qQ", read(dynamic[4], dynamic[5])):
                if tag == 0:
                    break
                if tag == 1:
                    needed.append(string(dynstrings, value))
                elif tag in (15, 29):
                    rpaths.append(string(dynstrings, value))
        if rpaths:
            raise RuntimeError(f"OHOS 包不能含未声明的 RPATH/RUNPATH：{path}: {rpaths}")
        signed = by_name.get(".codesign")
        has_code_signature = signed is not None and signed[5] > 0
    with path.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    return {
        "文件": str(path),
        "字节数": size,
        "SHA-256": checksum,
        "架构": "AArch64",
        "解释器": interpreter,
        "动态依赖": needed,
        "OHOS标识": True,
        "签名节存在": has_code_signature,
        "验证边界": "静态属性；签名节存在不等于设备接受或程序运行通过",
    }
