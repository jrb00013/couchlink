"""Enumerate/read DualSense — mirrors dualsensekit python/dualsensekit/device.py."""

from __future__ import annotations
from dataclasses import dataclass
from typing import List, Optional
import struct

try:
    import hid
except ImportError as e:  # pragma: no cover
    raise SystemExit("pip install hidapi") from e

SONY_VID = 0x054C
PID_DUALSENSE = 0x0CE6
PID_EDGE = 0x0DF2
INPUT_USB = 0x01
INPUT_BT = 0x31


@dataclass
class DeviceInfo:
    path: bytes
    product_id: int
    interface_number: int
    connection: str


def enumerate_devices() -> List[DeviceInfo]:
    out: List[DeviceInfo] = []
    for d in hid.enumerate(SONY_VID):
        if d["product_id"] not in (PID_DUALSENSE, PID_EDGE):
            continue
        iface = d.get("interface_number", -1)
        usage_page = d.get("usage_page")
        usage = d.get("usage")
        if usage_page == 1 and usage == 5:
            pass
        elif iface in (-1, 3):
            pass
        else:
            continue
        conn = "bluetooth" if iface is not None and iface < 0 else "usb"
        out.append(
            DeviceInfo(
                path=d["path"],
                product_id=d["product_id"],
                interface_number=iface if iface is not None else -1,
                connection=conn,
            )
        )
    return out


class DualSense:
    def __init__(self, path: Optional[bytes] = None):
        devices = enumerate_devices()
        if not devices:
            raise RuntimeError("no DualSense found")
        devices = sorted(devices, key=lambda x: 0 if x.connection == "usb" else 1)
        path = path or devices[0].path
        self.info = next(d for d in devices if d.path == path)
        self._dev = hid.device()
        self._dev.open_path(path)

    def read_raw(self, timeout_ms: int = 16) -> bytes:
        return bytes(self._dev.read(128, timeout_ms) or b"")
