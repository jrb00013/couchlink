"""Encode/decode CLPD pad frames for tests."""

from __future__ import annotations
import struct
from dataclasses import dataclass

MAGIC = b"CLPD"
VERSION = 1


@dataclass
class PadFrame:
    seq: int = 0
    buttons: int = 0
    lx: int = 128
    ly: int = 128
    rx: int = 128
    ry: int = 128
    l2: int = 0
    r2: int = 0

    def encode(self) -> bytes:
        body = struct.pack(
            "<BI4B2B3hBHHB",
            VERSION,
            self.seq,
            self.buttons,
            self.lx,
            self.ly,
            self.rx,
            self.ry,
            self.l2,
            self.r2,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        )
        # Manual pack to match Rust layout closely
        return MAGIC + struct.pack(
            "<BI4B2B",
            VERSION,
            self.seq & 0xFFFFFFFF,
            self.buttons & 0xFFFFFFFF,
            self.lx,
            self.ly,
            self.rx,
            self.ry,
            self.l2,
            self.r2,
        ) + struct.pack("<hhhBHHB", 0, 0, 0, 0, 0, 0, 0)
