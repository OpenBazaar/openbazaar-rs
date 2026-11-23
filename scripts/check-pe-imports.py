#!/usr/bin/env python3
"""Refuse a Windows PE that imports the MSVC runtime DLL.

ob-node.exe used to import VCRUNTIME140.dll while the Tauri shell did not
(static_vcruntime). Clean Windows installs without the Visual C++ redistributable
then failed to start the sidecar. build.rs now static-links the CRT for
windows-msvc; this check keeps a regression from shipping.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

# Universal CRT (api-ms-win-crt-*) ships with Windows 10+. The MSVC runtime
# DLLs below need the redistributable unless statically linked.
FORBIDDEN = {
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "msvcp140.dll",
    "msvcp140_1.dll",
    "msvcp140_2.dll",
    "concrt140.dll",
}


def pe_imports(data: bytes) -> list[str]:
    if data[:2] != b"MZ":
        raise SystemExit("not a PE file")
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    if data[e_lfanew : e_lfanew + 4] != b"PE\0\0":
        raise SystemExit("bad PE signature")
    _machine, num_sections, _t, _p, _s, opt_size, _chars = struct.unpack_from(
        "<HHIIIHH", data, e_lfanew + 4
    )
    opt_off = e_lfanew + 24
    magic = struct.unpack_from("<H", data, opt_off)[0]
    pe32plus = magic == 0x20B
    dd_off = opt_off + (112 if pe32plus else 96)
    import_rva = struct.unpack_from("<I", data, dd_off + 8)[0]
    sec_off = opt_off + opt_size
    sections = []
    for _ in range(num_sections):
        _name = data[sec_off : sec_off + 8]
        _vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", data, sec_off + 8)
        sections.append((vaddr, rawptr, rawsize, _vsize))
        sec_off += 40

    def rva_to_off(rva: int) -> int | None:
        for vaddr, rawptr, rawsize, vsize in sections:
            if vaddr <= rva < vaddr + max(rawsize, vsize):
                return rawptr + (rva - vaddr)
        return None

    off = rva_to_off(import_rva)
    if off is None:
        return []
    dlls: list[str] = []
    while True:
        _ilt, _stamp, _chain, name_rva, _iat = struct.unpack_from("<IIIII", data, off)
        if _ilt == 0 and name_rva == 0:
            break
        no = rva_to_off(name_rva)
        name = data[no:].split(b"\0", 1)[0].decode("ascii", "replace") if no else "?"
        dlls.append(name)
        off += 20
    return dlls


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(f"usage: {argv[0]} <pe>...", file=sys.stderr)
        return 2
    bad = 0
    for arg in argv[1:]:
        path = Path(arg)
        print(f"checking {path}")
        for name in pe_imports(path.read_bytes()):
            key = name.lower()
            if key in FORBIDDEN:
                print(f"  FORBIDDEN: {name}", file=sys.stderr)
                bad += 1
            else:
                print(f"  import: {name}")
    if bad:
        print(
            f"{bad} MSVC runtime import(s). Link with static_vcruntime "
            "(see crates/node/build.rs) so the redistributable is not required.",
            file=sys.stderr,
        )
        return 1
    print("ok: no MSVC runtime DLL imports")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
