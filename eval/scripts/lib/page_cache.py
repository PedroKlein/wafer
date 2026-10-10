#!/usr/bin/env python3
"""Records how much of the startup inputs sit in the page cache just before launch.

Usage: page_cache.py <startup-preparation.json> <runtime-binary> <config.toml> [plugin.wasm ...]

Adds `page_cache_residency` to the preparation receipt and prints the current
Unix-epoch nanoseconds, so the caller can use it as the runtime start time
without starting another process after the cache drop. The caller lists the
plugins itself, before the drop, so this probe never reads the config.
"""

from __future__ import annotations

import ctypes
import json
import mmap
import os
import sys
import time
from pathlib import Path


def resident_pages(path: Path) -> tuple[int, int] | None:
    """(resident, total) pages of the file, read with mincore without touching its pages."""
    try:
        libc = ctypes.CDLL(None, use_errno=True)
        libc.mmap.restype = ctypes.c_void_p
        libc.mmap.argtypes = [
            ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long
        ]
        libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
        fd = os.open(path, os.O_RDONLY)
    except (OSError, AttributeError):
        return None
    try:
        size = os.fstat(fd).st_size
        total = -(-size // mmap.PAGESIZE)
        if total == 0:
            return 0, 0
        address = libc.mmap(None, size, mmap.PROT_READ, mmap.MAP_SHARED, fd, 0)
        if address in (None, ctypes.c_void_p(-1).value):
            return None
        try:
            vector = (ctypes.c_ubyte * total)()
            if libc.mincore(address, size, vector) != 0:
                return None
            return sum(page & 1 for page in vector), total
        finally:
            libc.munmap(address, size)
    except OSError:
        return None
    finally:
        os.close(fd)


def residency(path: Path) -> dict[str, object]:
    pages = resident_pages(path)
    return {
        "path": str(path),
        "resident_pages": pages[0] if pages else None,
        "total_pages": pages[1] if pages else None,
    }


def main(argv: list[str]) -> int:
    if len(argv) < 4:
        print(
            f"usage: {argv[0]} <startup-preparation.json> <runtime-binary> <config.toml> [plugin.wasm ...]",
            file=sys.stderr,
        )
        return 2
    receipt_path = Path(argv[1])
    receipt = json.loads(receipt_path.read_text())
    receipt["page_cache_residency"] = {
        "runtime_binary": residency(Path(argv[2])),
        "config": residency(Path(argv[3])),
        "plugins": [residency(Path(plugin)) for plugin in argv[4:]],
    }
    receipt_path.write_text(json.dumps(receipt) + "\n")
    print(time.time_ns())
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
