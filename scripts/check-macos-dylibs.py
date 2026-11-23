#!/usr/bin/env python3
"""Refuse a macOS Mach-O that loads Homebrew or other non-portable dylibs.

Release runners have Homebrew xz installed, so liblzma-sys will dynamically
link /opt/homebrew/opt/xz/lib/liblzma.5.dylib unless we force a static build.
That path does not exist on most user machines and aborts the node at
startup. Run this on every shipped macOS binary (thin or universal).
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

LC_LOAD_DYLIB = 0xC
LC_LOAD_WEAK_DYLIB = 0x18
LC_REEXPORT_DYLIB = 0x1F
LC_LOAD_UPWARD_DYLIB = 0x23
LC_RPATH = 0x1C
LOAD_COMMANDS = {
    LC_LOAD_DYLIB,
    LC_LOAD_WEAK_DYLIB,
    LC_REEXPORT_DYLIB,
    LC_LOAD_UPWARD_DYLIB,
    LC_RPATH,
    0x80000018,  # LC_LOAD_WEAK_DYLIB | LC_REQ_DYLD
    0x8000001C,  # LC_RPATH | LC_REQ_DYLD
}

# Absolute install names that must never appear in a redistributed binary.
FORBIDDEN_PREFIXES = (
    "/opt/homebrew/",
    "/usr/local/opt/",
    "/usr/local/Cellar/",
    "/opt/local/",  # MacPorts
)


def u32(data: bytes, off: int, endian: str) -> int:
    return struct.unpack_from(endian + "I", data, off)[0]


def slices(data: bytes) -> list[tuple[int, int, int | None]]:
    magic = struct.unpack_from(">I", data, 0)[0]
    if magic == 0xCAFEBABE:
        nfat = struct.unpack_from(">I", data, 4)[0]
        out = []
        off = 8
        for _ in range(nfat):
            cputype, _cpusub, offset, size, _align = struct.unpack_from(">IIIII", data, off)
            out.append((offset, size, cputype))
            off += 20
        return out
    return [(0, len(data), None)]


def dylibs_in_slice(data: bytes, offset: int) -> list[str]:
    mag = u32(data, offset, "<")
    if mag in (0xFEEDFACE, 0xFEEDFACF):
        endian = "<"
        is64 = mag == 0xFEEDFACF
    elif mag in (0xCEFAEDFE, 0xCFFAEDFE):
        endian = "<"
        is64 = mag == 0xCFFAEDFE
    else:
        raise SystemExit(f"unknown mach-o magic {mag:#x} at {offset}")
    ncmds = u32(data, offset + 16, endian)
    cmd_off = offset + (32 if is64 else 28)
    names: list[str] = []
    for _ in range(ncmds):
        cmd = u32(data, cmd_off, endian)
        cmdsize = u32(data, cmd_off + 4, endian)
        if cmd in LOAD_COMMANDS:
            name_off = u32(data, cmd_off + 8, endian)
            raw = data[cmd_off + name_off : cmd_off + cmdsize]
            name = raw.split(b"\0", 1)[0].decode("utf-8", "replace")
            names.append(name)
        cmd_off += cmdsize
    return names


def forbidden(name: str) -> bool:
    if name.startswith(FORBIDDEN_PREFIXES):
        return True
    # Absolute liblzma anywhere outside the system library roots is wrong for
    # a redistributed binary (Homebrew Intel used to live under /usr/local).
    if "liblzma" in name and name.startswith("/") and not name.startswith("/usr/lib/"):
        return True
    return False


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(f"usage: {argv[0]} <mach-o>...", file=sys.stderr)
        return 2
    bad = 0
    for arg in argv[1:]:
        path = Path(arg)
        data = path.read_bytes()
        print(f"checking {path}")
        for offset, _size, cpu in slices(data):
            label = f"cpu {cpu:#x}" if cpu is not None else "thin"
            for name in dylibs_in_slice(data, offset):
                if forbidden(name):
                    print(f"  FORBIDDEN ({label}): {name}", file=sys.stderr)
                    bad += 1
                elif not name.startswith(("/usr/lib/", "/System/", "@")):
                    # Unusual but not automatically fatal; print for the log.
                    print(f"  note ({label}): {name}")
    if bad:
        print(
            f"{bad} non-portable dylib reference(s). Build with liblzma/static "
            "(or LZMA_API_STATIC=1) so xz is compiled into the binary.",
            file=sys.stderr,
        )
        return 1
    print("ok: no Homebrew or other non-portable dylibs")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
