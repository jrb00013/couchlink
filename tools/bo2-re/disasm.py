#!/usr/bin/env python3
"""Disassemble PPC64 big-endian guest memory dumped by gdb_dump.ps1.

  disasm.py <name>.<hexaddr>.hex [--from 73d6ec --to 73d7b4]
  disasm.py --selftest         # checks the toolchain on the shipped patch words
"""
import sys
from capstone import Cs, CS_ARCH_PPC, CS_MODE_64, CS_MODE_BIG_ENDIAN

md = Cs(CS_ARCH_PPC, CS_MODE_64 | CS_MODE_BIG_ENDIAN)

def dis(data: bytes, base: int, lo=None, hi=None):
    for i in md.disasm(data, base):
        if (lo is None or i.address >= lo) and (hi is None or i.address <= hi):
            print(f"{i.address:08x}  {i.bytes.hex()}  {i.mnemonic:8s} {i.op_str}")

def main(argv):
    if "--selftest" in argv:
        for addr, word, want in ((0x41c3cc, 0x38600000, "li"), (0x41c414, 0x60000000, "nop")):
            got = next(md.disasm(word.to_bytes(4, "big"), addr)).mnemonic
            assert got == want, (hex(addr), got, want)
            print(f"ok {addr:x} {word:08x} -> {got}")
        return 0
    path = argv[0]
    base = int(path.rsplit(".", 2)[-2], 16)
    lo = int(argv[argv.index("--from") + 1], 16) if "--from" in argv else None
    hi = int(argv[argv.index("--to") + 1], 16) if "--to" in argv else None
    dis(bytes.fromhex(open(path).read().strip()), base, lo, hi)
    return 0

sys.exit(main(sys.argv[1:]))
