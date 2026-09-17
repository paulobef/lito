#!/usr/bin/env python3
"""Minimal PNG pixel sampler (8-bit RGB/RGBA, non-interlaced), stdlib only.

Reads exact pixel values out of a `make ui-shot` render. Use it instead of
eyeballing a screenshot whenever the claim is about a specific color or a 1px
alignment: the PNG is downscaled before an agent sees it, and antialiasing at
that scale misleads. See .claude/skills/slint/SKILL.md, "Seeing the pixels".

    python3 crates/ui/ui/pxsample.py target/ui-shots/demo-dark.png \\
        sidebar=110,400 chat-bg=500,400

prints one `name (x,y) -> #rrggbb` line per point.
"""
import struct, zlib, sys


def load(path):
    data = open(path, 'rb').read()
    assert data[:8] == b'\x89PNG\r\n\x1a\n'
    pos, idat, w = 8, b'', None
    while pos < len(data):
        ln, typ = struct.unpack('>I4s', data[pos:pos + 8])
        chunk = data[pos + 8:pos + 8 + ln]
        if typ == b'IHDR':
            w, h, depth, ctype = struct.unpack('>IIBB', chunk[:10])
            assert depth == 8 and ctype in (2, 6), (depth, ctype)
            nch = 3 if ctype == 2 else 4
        elif typ == b'IDAT':
            idat += chunk
        elif typ == b'IEND':
            break
        pos += 12 + ln
    raw = zlib.decompress(idat)
    stride = w * nch
    out, prev = [], bytearray(stride)
    p = 0
    for _ in range(h):
        f = raw[p]; p += 1
        line = bytearray(raw[p:p + stride]); p += stride
        for i in range(stride):
            a = line[i - nch] if i >= nch else 0
            b = prev[i]
            c = prev[i - nch] if i >= nch else 0
            if f == 1: line[i] = (line[i] + a) & 255
            elif f == 2: line[i] = (line[i] + b) & 255
            elif f == 3: line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 255
        out.append(bytes(line)); prev = line
    return w, h, nch, out


def px(img, x, y):
    w, h, nch, rows = img
    o = x * nch
    r, g, b = rows[y][o], rows[y][o + 1], rows[y][o + 2]
    return '#%02x%02x%02x' % (r, g, b)


if __name__ == '__main__':
    img = load(sys.argv[1])
    print(f'{sys.argv[1]}  ({img[0]}x{img[1]})')
    for spec in sys.argv[2:]:
        name, xy = spec.split('=')
        x, y = map(int, xy.split(','))
        print(f'  {name:<22} ({x},{y}) -> {px(img, x, y)}')
