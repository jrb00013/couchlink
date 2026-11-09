#!/usr/bin/env python3
"""Resolve RVAs in an MSVC build to the nearest public symbol using its PDB.

  symbolize.py rpcs3.pdb 0x7ab562 0x7ab25e ...

RVA = crash address - module base (RPCS3's fatal dump prints both).
Self-contained MSF 7.0 + DBI + S_PUB32 reader (no third-party packages).
Names are MSVC-mangled.
"""
import bisect
import struct
import sys


def read_msf(path):
    data = open(path, "rb").read()
    assert data[:26] == b"Microsoft C/C++ MSF 7.00\r\n", "not an MSF 7.0 PDB"
    bs, _free, nblocks, dir_bytes, _unk, map_addr = struct.unpack_from("<6I", data, 32)
    ndir_blocks = -(-dir_bytes // bs)
    dir_block_ids = struct.unpack_from(f"<{ndir_blocks}I", data, map_addr * bs)
    directory = b"".join(data[b * bs:(b + 1) * bs] for b in dir_block_ids)[:dir_bytes]
    nstreams = struct.unpack_from("<I", directory, 0)[0]
    sizes = struct.unpack_from(f"<{nstreams}I", directory, 4)
    off = 4 + 4 * nstreams
    streams = []
    for sz in sizes:
        if sz == 0xFFFFFFFF:
            streams.append(None)
            continue
        nb = -(-sz // bs)
        ids = struct.unpack_from(f"<{nb}I", directory, off)
        off += 4 * nb
        streams.append(b"".join(data[b * bs:(b + 1) * bs] for b in ids)[:sz])
    return streams


def load_symbols(path):
    streams = read_msf(path)
    dbi = streams[3]
    (_sig, _ver, _age, _gsi, _build, _psi, _pdbdll, sym_rec, _rbld,
     modinfo, seccontrib, secmap, srcinfo, tsmap, _mfc, optdbg, ecsub) = \
        struct.unpack_from("<iIIHHHHHHiiiiiIii", dbi, 0)
    opt_off = 64 + modinfo + seccontrib + secmap + srcinfo + tsmap + ecsub
    dbg = struct.unpack_from(f"<{optdbg // 2}H", dbi, opt_off)
    sec_stream = dbg[5]  # section headers (no OMAP in MSVC release builds)
    sec = streams[sec_stream]
    sects = []
    for i in range(len(sec) // 40):
        _name, _vsize, vaddr = struct.unpack_from("<8sII", sec, i * 40)
        sects.append(vaddr)
    recs = streams[sym_rec]
    syms, pos = [], 0
    while pos + 4 <= len(recs):
        rlen, rtype = struct.unpack_from("<HH", recs, pos)
        if rtype == 0x110E:  # S_PUB32
            _flags, offset, seg = struct.unpack_from("<IIH", recs, pos + 4)
            name = recs[pos + 14:pos + 2 + rlen].split(b"\0")[0].decode("latin-1")
            if 1 <= seg <= len(sects):
                syms.append((sects[seg - 1] + offset, name))
        pos += 2 + rlen
    syms.sort()
    return syms


def main(argv):
    syms = load_symbols(argv[0])
    keys = [s[0] for s in syms]
    print(f"{len(syms)} public symbols")
    for arg in argv[1:]:
        rva = int(arg, 16)
        i = bisect.bisect_right(keys, rva) - 1
        if i < 0:
            print(f"{rva:#x}  <no symbol>")
        else:
            print(f"{rva:#x}  {syms[i][1]} +{rva - syms[i][0]:#x}")


if __name__ == "__main__":
    main(sys.argv[1:])
