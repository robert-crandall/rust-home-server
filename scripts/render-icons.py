#!/usr/bin/env python3
"""Validate a square PNG (>= 512x512) and write 512x512 + 192x192 PWA icons."""

from __future__ import annotations

import argparse
import os
import struct
import sys
import zlib
from pathlib import Path

PNG_SIG = b"\x89PNG\r\n\x1a\n"
MIN_SIZE = 512
MAX_SIZE = 4096


class IconError(Exception):
    pass


def _paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def decode_png_rgba8(data: bytes) -> tuple[int, int, bytearray]:
    if len(data) < 33 or data[:8] != PNG_SIG:
        raise IconError("input must be a valid PNG file (non-PNG or SVG files are not supported)")

    pos = 8
    width = height = bit_depth = color_type = interlace = 0
    saw_ihdr = False
    saw_iend = False
    palette: bytes | None = None
    trns: bytes | None = None
    idat_parts: list[bytes] = []

    while pos + 12 <= len(data):
        length = struct.unpack(">I", data[pos : pos + 4])[0]
        ctype = data[pos + 4 : pos + 8]
        chunk_start = pos + 8
        chunk_end = chunk_start + length
        crc_end = chunk_end + 4
        if crc_end > len(data):
            raise IconError("truncated PNG chunk")
        chunk = data[chunk_start:chunk_end]
        expected_crc = struct.unpack(">I", data[chunk_end:crc_end])[0]
        if (zlib.crc32(ctype + chunk) & 0xFFFFFFFF) != expected_crc:
            raise IconError(f"corrupt PNG chunk CRC ({ctype.decode('ascii', 'replace')})")
        pos = crc_end

        if ctype == b"IHDR":
            if saw_ihdr or length != 13:
                raise IconError("invalid PNG IHDR chunk")
            width, height, bit_depth, color_type, compression, filt, interlace = struct.unpack(
                ">IIBBBBB", chunk
            )
            if compression != 0 or filt != 0:
                raise IconError("unsupported PNG compression or filter method")
            saw_ihdr = True
        elif ctype == b"PLTE":
            palette = chunk
        elif ctype == b"tRNS":
            trns = chunk
        elif ctype == b"IDAT":
            idat_parts.append(chunk)
        elif ctype == b"IEND":
            saw_iend = True
            break

    if not saw_ihdr or not saw_iend or not idat_parts:
        raise IconError("incomplete PNG file")
    if width == 0 or height == 0:
        raise IconError("PNG has zero dimensions")
    if width > MAX_SIZE or height > MAX_SIZE:
        raise IconError(f"icon must be at most {MAX_SIZE}x{MAX_SIZE}, got {width}x{height}")
    if interlace != 0:
        raise IconError("interlaced PNGs are not supported; re-export as a non-interlaced 8-bit PNG")

    if color_type not in (0, 2, 3, 4, 6):
        raise IconError(f"unsupported PNG color type {color_type}; re-export as 8-bit RGBA PNG")
    if color_type == 3:
        if bit_depth != 8:
            raise IconError("indexed PNGs must be 8-bit; re-export as 8-bit RGBA PNG")
        if palette is None or len(palette) % 3 != 0 or len(palette) == 0:
            raise IconError("indexed PNG is missing a valid PLTE palette")
    elif bit_depth not in (8, 16):
        raise IconError(f"unsupported PNG bit depth {bit_depth}; re-export as 8-bit RGBA PNG")

    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color_type]
    sample_bytes = bit_depth // 8
    bpp = channels * sample_bytes
    stride = width * bpp
    expected_raw_len = height * (1 + stride)

    try:
        decoder = zlib.decompressobj()
        raw = decoder.decompress(b"".join(idat_parts), expected_raw_len + 1)
    except zlib.error as exc:
        raise IconError(f"corrupt PNG image data: {exc}") from exc

    if len(raw) != expected_raw_len or not decoder.eof:
        raise IconError("PNG image data size does not match header dimensions")

    # Unfilter scanlines.
    unfiltered = bytearray(height * stride)
    prev = bytes(stride)
    for y in range(height):
        row_in_start = y * (stride + 1)
        ftype = raw[row_in_start]
        src = raw[row_in_start + 1 : row_in_start + 1 + stride]
        row = bytearray(stride)

        if ftype == 0:
            row[:] = src
        elif ftype == 1:
            for i in range(bpp):
                row[i] = src[i]
            for i in range(bpp, stride):
                row[i] = (src[i] + row[i - bpp]) & 0xFF
        elif ftype == 2:
            for i in range(stride):
                row[i] = (src[i] + prev[i]) & 0xFF
        elif ftype == 3:
            for i in range(bpp):
                row[i] = (src[i] + (prev[i] >> 1)) & 0xFF
            for i in range(bpp, stride):
                row[i] = (src[i] + ((row[i - bpp] + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:
            for i in range(bpp):
                row[i] = (src[i] + _paeth(0, prev[i], 0)) & 0xFF
            for i in range(bpp, stride):
                row[i] = (src[i] + _paeth(row[i - bpp], prev[i], prev[i - bpp])) & 0xFF
        else:
            raise IconError(f"unsupported PNG scanline filter {ftype}")

        out_start = y * stride
        unfiltered[out_start : out_start + stride] = row
        prev = row

    # Convert to RGBA8.
    if color_type == 6 and bit_depth == 8:
        return width, height, unfiltered

    rgba = bytearray(width * height * 4)
    pixel_count = width * height

    if color_type == 2 and bit_depth == 8:
        trns_rgb = tuple(struct.unpack(">HHH", trns)) if trns and len(trns) == 6 else None
        for p in range(pixel_count):
            si = p * 3
            di = p * 4
            r, g, b = unfiltered[si], unfiltered[si + 1], unfiltered[si + 2]
            a = 0 if trns_rgb == (r, g, b) else 255
            rgba[di : di + 4] = bytes((r, g, b, a))
    elif color_type == 3:
        assert palette is not None
        num_entries = len(palette) // 3
        lut = [
            bytes((
                palette[i * 3],
                palette[i * 3 + 1],
                palette[i * 3 + 2],
                trns[i] if trns is not None and i < len(trns) else 255,
            ))
            for i in range(num_entries)
        ]
        for p in range(pixel_count):
            idx = unfiltered[p]
            if idx >= num_entries:
                raise IconError("indexed PNG references out-of-range palette index")
            di = p * 4
            rgba[di : di + 4] = lut[idx]
    elif color_type == 0 and bit_depth == 8:
        trns_gray = struct.unpack(">H", trns)[0] if trns and len(trns) == 2 else None
        for p in range(pixel_count):
            v = unfiltered[p]
            a = 0 if trns_gray == v else 255
            di = p * 4
            rgba[di : di + 4] = bytes((v, v, v, a))
    elif color_type == 4 and bit_depth == 8:
        for p in range(pixel_count):
            si = p * 2
            v, a = unfiltered[si], unfiltered[si + 1]
            di = p * 4
            rgba[di : di + 4] = bytes((v, v, v, a))
    else:
        # 16-bit variants (color types 0, 2, 4, 6): take high byte per channel.
        trns_16 = (
            struct.unpack(">H", trns)[0]
            if color_type == 0 and trns and len(trns) == 2
            else (
                struct.unpack(">HHH", trns)
                if color_type == 2 and trns and len(trns) == 6
                else None
            )
        )
        for p in range(pixel_count):
            si = p * bpp
            di = p * 4
            if color_type == 6:
                rgba[di : di + 4] = bytes(
                    (unfiltered[si], unfiltered[si + 2], unfiltered[si + 4], unfiltered[si + 6])
                )
            elif color_type == 2:
                r16, g16, b16 = struct.unpack(">HHH", unfiltered[si : si + 6])
                a = 0 if trns_16 == (r16, g16, b16) else 255
                rgba[di : di + 4] = bytes((r16 >> 8, g16 >> 8, b16 >> 8, a))
            elif color_type == 4:
                v, a = unfiltered[si], unfiltered[si + 2]
                rgba[di : di + 4] = bytes((v, v, v, a))
            else:
                v16 = struct.unpack(">H", unfiltered[si : si + 2])[0]
                a = 0 if trns_16 == v16 else 255
                v = v16 >> 8
                rgba[di : di + 4] = bytes((v, v, v, a))

    return width, height, rgba


def _axis_spans(src_len: int, dst_len: int) -> list[list[tuple[int, float]]]:
    scale = src_len / dst_len
    spans: list[list[tuple[int, float]]] = []
    for d in range(dst_len):
        start = d * scale
        end = (d + 1) * scale
        i0 = int(start)
        i1 = min(src_len, int(end) + (1 if end > int(end) else 0))
        weights: list[tuple[int, float]] = []
        for s in range(i0, i1):
            overlap = min(end, s + 1.0) - max(start, float(s))
            if overlap > 0.0:
                weights.append((s, overlap / scale))
        spans.append(weights)
    return spans


def downsample_square_rgba8(src_size: int, rgba: bytearray, dst_size: int) -> bytearray:
    if src_size == dst_size:
        return bytearray(rgba)

    spans = _axis_spans(src_size, dst_size)

    # Pass 1: horizontal premultiplied area average (src_size x src_size -> dst_size x src_size).
    horiz = [0.0] * (src_size * dst_size * 4)
    src_row_stride = src_size * 4
    h_row_stride = dst_size * 4
    for y in range(src_size):
        srow = y * src_row_stride
        hrow = y * h_row_stride
        for dx, weights in enumerate(spans):
            pr = pg = pb = pa = 0.0
            for sx, w in weights:
                idx = srow + sx * 4
                a = rgba[idx + 3]
                wa = w * a
                pr += rgba[idx] * wa
                pg += rgba[idx + 1] * wa
                pb += rgba[idx + 2] * wa
                pa += wa
            hidx = hrow + dx * 4
            horiz[hidx] = pr
            horiz[hidx + 1] = pg
            horiz[hidx + 2] = pb
            horiz[hidx + 3] = pa

    # Pass 2: vertical area average + unpremultiply (dst_size x src_size -> dst_size x dst_size).
    out = bytearray(dst_size * dst_size * 4)
    for dy, weights in enumerate(spans):
        orow = dy * h_row_stride
        for dx in range(dst_size):
            pr = pg = pb = pa = 0.0
            dx4 = dx * 4
            for sy, w in weights:
                hidx = sy * h_row_stride + dx4
                pr += horiz[hidx] * w
                pg += horiz[hidx + 1] * w
                pb += horiz[hidx + 2] * w
                pa += horiz[hidx + 3] * w
            oidx = orow + dx4
            if pa <= 0.5:
                out[oidx : oidx + 4] = b"\x00\x00\x00\x00"
            else:
                r = min(255, max(0, int(pr / pa + 0.5)))
                g = min(255, max(0, int(pg / pa + 0.5)))
                b = min(255, max(0, int(pb / pa + 0.5)))
                a = min(255, max(0, int(pa + 0.5)))
                out[oidx : oidx + 4] = bytes((r, g, b, a))

    return out


def encode_png_rgba8(width: int, height: int, rgba: bytearray) -> bytes:
    def chunk(ctype: bytes, payload: bytes) -> bytes:
        crc = zlib.crc32(ctype + payload) & 0xFFFFFFFF
        return struct.pack(">I", len(payload)) + ctype + payload + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    stride = width * 4
    raw = bytearray(height * (1 + stride))
    for y in range(height):
        raw[y * (1 + stride)] = 0
        src_start = y * stride
        dst_start = y * (1 + stride) + 1
        raw[dst_start : dst_start + stride] = rgba[src_start : src_start + stride]

    idat = zlib.compress(bytes(raw), level=9)
    return PNG_SIG + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def render_icons(
    icon_path: Path, out_dir: Path, reject_same_as: Path | None = None
) -> None:
    expanded = Path(os.path.expanduser(str(icon_path)))
    if not expanded.is_file():
        raise IconError(f"icon file not found: {icon_path}")

    source_bytes = expanded.read_bytes()
    width, height, rgba = decode_png_rgba8(source_bytes)

    if width != height:
        raise IconError(f"icon must be square, got {width}x{height}")
    if width < MIN_SIZE:
        raise IconError(f"icon must be at least {MIN_SIZE}x{MIN_SIZE}, got {width}x{height}")
    if width > MAX_SIZE:
        raise IconError(f"icon must be at most {MAX_SIZE}x{MAX_SIZE}, got {width}x{height}")

    rgba_512 = downsample_square_rgba8(width, rgba, 512)

    if reject_same_as is not None and reject_same_as.is_file():
        ew, eh, existing_rgba = decode_png_rgba8(reject_same_as.read_bytes())
        if ew == 512 and eh == 512 and rgba_512 == existing_rgba:
            raise IconError(
                "ICON matches the default template icon; provide a new icon for your app"
            )

    rgba_192 = downsample_square_rgba8(512, rgba_512, 192)
    png_512 = encode_png_rgba8(512, 512, rgba_512)
    png_192 = encode_png_rgba8(192, 192, rgba_192)

    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "icon-512.png").write_bytes(png_512)
    (out_dir / "icon-192.png").write_bytes(png_192)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Validate a square PNG (>= 512x512) and generate 512x512 and 192x192 PWA icons."
    )
    parser.add_argument("icon", help="Path to a square PNG icon (>= 512x512)")
    parser.add_argument(
        "--out-dir",
        default="web/public",
        help="Directory to write icon-512.png and icon-192.png (default: web/public)",
    )
    parser.add_argument(
        "--reject-same-as",
        default=None,
        help="Reject if the rendered 512x512 icon matches this existing PNG",
    )
    args = parser.parse_args(argv)

    try:
        render_icons(
            Path(args.icon),
            Path(args.out_dir),
            Path(args.reject_same_as) if args.reject_same_as else None,
        )
    except IconError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
