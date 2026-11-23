#!/usr/bin/env python3
"""Refuse a Linux ELF that dynamically needs OpenSSL (or other non-portable SOs).

Arti's default features pull native-tls → openssl-sys. On Ubuntu runners that
links libssl.so.3 / libcrypto.so.3, so the home-node tarball and the .deb's
ob-node abort on machines without OpenSSL 3 (older Debian, Alpine-adjacent
chroots, many hardened templates). The node now builds Arti with rustls;
this check keeps a regression from shipping.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

FORBIDDEN = {
    "libssl.so",
    "libssl.so.1.1",
    "libssl.so.3",
    "libcrypto.so",
    "libcrypto.so.1.1",
    "libcrypto.so.3",
    "liblzma.so.5",  # same Homebrew-style footgun if a distro xz is linked
}


def needed(data: bytes) -> list[str]:
    if data[:4] != b"\x7fELF":
        raise SystemExit("not an ELF file")
    ei_class = data[4]  # 1=32, 2=64
    ei_data = data[5]  # 1=LE, 2=BE
    endian = "<" if ei_data == 1 else ">"
    if ei_class == 2:
        e_phoff = struct.unpack_from(endian + "Q", data, 32)[0]
        e_phentsize = struct.unpack_from(endian + "H", data, 54)[0]
        e_phnum = struct.unpack_from(endian + "H", data, 56)[0]
    else:
        e_phoff = struct.unpack_from(endian + "I", data, 28)[0]
        e_phentsize = struct.unpack_from(endian + "H", data, 42)[0]
        e_phnum = struct.unpack_from(endian + "H", data, 44)[0]

    dyn_off = dyn_size = None
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type = struct.unpack_from(endian + "I", data, off)[0]
        if p_type != 2:  # PT_DYNAMIC
            continue
        if ei_class == 2:
            p_offset = struct.unpack_from(endian + "Q", data, off + 8)[0]
            p_filesz = struct.unpack_from(endian + "Q", data, off + 32)[0]
        else:
            p_offset = struct.unpack_from(endian + "I", data, off + 4)[0]
            p_filesz = struct.unpack_from(endian + "I", data, off + 16)[0]
        dyn_off, dyn_size = p_offset, p_filesz
        break
    if dyn_off is None:
        return []

    # Collect DT_NEEDED (1) and DT_STRTAB (5) / DT_STRSZ (10).
    entry = 16 if ei_class == 2 else 8
    strtab_addr = strsz = None
    needed_offsets: list[int] = []
    pos = dyn_off
    end = dyn_off + dyn_size
    while pos + entry <= end:
        if ei_class == 2:
            d_tag, d_val = struct.unpack_from(endian + "qQ", data, pos)
        else:
            d_tag, d_val = struct.unpack_from(endian + "iI", data, pos)
        pos += entry
        if d_tag == 0:
            break
        if d_tag == 1:
            needed_offsets.append(d_val)
        elif d_tag == 5:
            strtab_addr = d_val
        elif d_tag == 10:
            strsz = d_val

    if strtab_addr is None:
        return []

    # STRTAB is a virtual address; map via PT_LOAD.
    strtab_off = None
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type = struct.unpack_from(endian + "I", data, off)[0]
        if p_type != 1:  # PT_LOAD
            continue
        if ei_class == 2:
            p_offset = struct.unpack_from(endian + "Q", data, off + 8)[0]
            p_vaddr = struct.unpack_from(endian + "Q", data, off + 16)[0]
            p_filesz = struct.unpack_from(endian + "Q", data, off + 32)[0]
        else:
            p_offset = struct.unpack_from(endian + "I", data, off + 4)[0]
            p_vaddr = struct.unpack_from(endian + "I", data, off + 8)[0]
            p_filesz = struct.unpack_from(endian + "I", data, off + 16)[0]
        if p_vaddr <= strtab_addr < p_vaddr + p_filesz:
            strtab_off = p_offset + (strtab_addr - p_vaddr)
            break
    if strtab_off is None:
        raise SystemExit("could not map DT_STRTAB")

    names = []
    for rel in needed_offsets:
        start = strtab_off + rel
        end = data.index(b"\0", start)
        names.append(data[start:end].decode("ascii", "replace"))
    return names


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(f"usage: {argv[0]} <elf>...", file=sys.stderr)
        return 2
    bad = 0
    for arg in argv[1:]:
        path = Path(arg)
        print(f"checking {path}")
        for name in needed(path.read_bytes()):
            base = name.split("/")[-1]
            # Match libssl.so.3 etc. by prefix against forbidden stems.
            forbidden = base in FORBIDDEN or any(
                base == f or base.startswith(f + ".") for f in ("libssl.so", "libcrypto.so", "liblzma.so")
            )
            if forbidden:
                print(f"  FORBIDDEN: {name}", file=sys.stderr)
                bad += 1
            else:
                print(f"  NEEDED: {name}")
    if bad:
        print(
            f"{bad} non-portable shared library reference(s). "
            "Build Arti with rustls (not native-tls) so OpenSSL is not linked.",
            file=sys.stderr,
        )
        return 1
    print("ok: no OpenSSL or other forbidden shared libraries")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
