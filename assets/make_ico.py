"""Packs PNG images into a Windows .ico (PNG-compressed entries, Windows Vista and later).

Usage: python3 make_ico.py out.ico 16.png 32.png 64.png 128.png 256.png
"""
import struct
import sys


def png_size(data: bytes) -> tuple[int, int]:
    # The IHDR chunk follows the 8-byte signature: width and height are big-endian u32s.
    return struct.unpack(">II", data[16:24])


def main(out: str, pngs: list[str]) -> None:
    images = [open(path, "rb").read() for path in pngs]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries, payload = b"", b""
    for data in images:
        width, height = png_size(data)
        entries += struct.pack(
            "<BBBBHHII", width % 256, height % 256, 0, 0, 1, 32, len(data), offset + len(payload)
        )
        payload += data
    with open(out, "wb") as file:
        file.write(header + entries + payload)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2:])
