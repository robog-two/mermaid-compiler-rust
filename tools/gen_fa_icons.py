#!/usr/bin/env python3
"""Generate assets/fontawesome/icons.bin from the Font Awesome Free npm package.

mmdc loads `@fortawesome/fontawesome-free` (^7) into its page, so labels such as
`fa:fa-car` draw the Font Awesome glyph. This script packs every free icon's
outline (from `svgs/{solid,regular,brands}`) and every CSS class name (from
`css/fontawesome.css` and `css/brands.css`, aliases included) into one compact
file that `src/icons.rs` reads.

Usage (standard library only):

    npm pack @fortawesome/fontawesome-free@7 && tar xzf fortawesome-fontawesome-free-*.tgz
    python3 tools/gen_fa_icons.py package assets/fontawesome/icons.bin

Format (all integers little-endian):

    "FAI1"                         magic
    u8 len, bytes                  package version (e.g. "7.3.1")
    u32 n                          class names, sorted by name
    n x (u8 len, bytes, u32 cp)    `fa-<name>` without the `fa-` prefix -> codepoint
    3 x style (solid, regular, brands):
      u32 m                        glyphs, sorted by codepoint
      m x (u32 cp, u16 advance, u32 path offset)
    3 x (u16 advance, u32 offset)  each style's `.notdef` glyph
    u32 len, bytes                 path data

Advances and coordinates are in font units (512 per em, ascent 448), with y
pointing down from the top of the em box as in the SVGs. A path is a sequence of
commands, each one byte followed by zigzag LEB128 varints of tenths of a unit,
relative to the current point:

    0 end, 1 M dx dy, 2 L dx dy, 3 C dx1 dy1 dx2 dy2 dx dy,
    4 A rx ry rotation flags(large*2+sweep, plain varint) dx dy, 5 Z
"""

import json
import os
import re
import struct
import sys

STYLES = ["solid", "regular", "brands"]
# The fonts' `.notdef` glyphs (drawn for a class whose glyph the style's font
# lacks, e.g. `fa:fa-github`), read from webfonts/fa-*.woff2 with fontTools and
# flipped to SVG coordinates (y = 448 - font y). Advance 384.
_CROSS = (
    "M64 390 154 256 64 122V390ZM103 448H282L192 314L103 448ZM231 256 320 390V122L231 256Z"
    "M282 64H103L192 198L282 64Z"
)
_CROSS_REGULAR = (
    "M48 422 163 256 48 90V422ZM77 464H307L192 298L77 464ZM221 256 336 422V90L221 256Z"
    "M307 48H77L192 214L307 48Z"
)
_BOX = "M0 48C0 22 22 0 48 0H336C363 0 384 22 384 48V464C384 491 363 512 336 512H48C22 512 0 491 0 464V48Z"
NOTDEF = {"solid": _CROSS + _BOX, "regular": _CROSS_REGULAR + _BOX, "brands": _CROSS + _BOX}
TOKEN = re.compile(r"[MmLlHhVvCcSsQqTtAaZz]|[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?")


def tenths(v):
    t = round(v * 10)
    assert abs(t - v * 10) < 1e-6, v
    return t


def zigzag(v):
    return (v << 1) ^ (v >> 63) if v < 0 else v << 1


def varint(v):
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def encode_path(d):
    toks = TOKEN.findall(d)
    out = bytearray()
    # Current point and subpath start, in tenths (exact integers).
    cx = cy = sx = sy = 0
    prev_ctrl = None  # second control point of the previous cubic, for S
    i = 0
    cmd = None

    def num():
        nonlocal i
        v = float(toks[i])
        i += 1
        return tenths(v)

    def emit(code, *vals):
        out.append(code)
        for v in vals:
            out.extend(varint(zigzag(v)))

    while i < len(toks):
        t = toks[i]
        if t.isalpha():
            cmd = t
            i += 1
            if cmd in "Zz":
                emit(5)
                cx, cy = sx, sy
                prev_ctrl = None
                continue
        elif cmd is None:
            raise ValueError(d)
        rel = cmd.islower()
        c = cmd.upper()
        ox, oy = (cx, cy) if rel else (0, 0)
        if c == "M":
            x, y = num() + ox, num() + oy
            emit(1, x - cx, y - cy)
            cx, cy = sx, sy = x, y
            cmd = "l" if rel else "L"
            prev_ctrl = None
        elif c == "L":
            x, y = num() + ox, num() + oy
            emit(2, x - cx, y - cy)
            cx, cy = x, y
            prev_ctrl = None
        elif c == "H":
            x = num() + ox
            emit(2, x - cx, 0)
            cx = x
            prev_ctrl = None
        elif c == "V":
            y = num() + oy
            emit(2, 0, y - cy)
            cy = y
            prev_ctrl = None
        elif c in "CS":
            if c == "C":
                x1, y1 = num() + ox, num() + oy
            else:
                x1, y1 = (2 * cx - prev_ctrl[0], 2 * cy - prev_ctrl[1]) if prev_ctrl else (cx, cy)
            x2, y2 = num() + ox, num() + oy
            x, y = num() + ox, num() + oy
            emit(3, x1 - cx, y1 - cy, x2 - cx, y2 - cy, x - cx, y - cy)
            prev_ctrl = (x2, y2)
            cx, cy = x, y
        elif c == "A":
            rx, ry, rot = num(), num(), num()
            large, sweep = int(float(toks[i])), int(float(toks[i + 1]))
            i += 2
            x, y = num() + ox, num() + oy
            out.append(4)
            out.extend(varint(zigzag(rx)))
            out.extend(varint(zigzag(ry)))
            out.extend(varint(zigzag(rot)))
            out.extend(varint(large * 2 + sweep))
            out.extend(varint(zigzag(x - cx)))
            out.extend(varint(zigzag(y - cy)))
            cx, cy = x, y
            prev_ctrl = None
        else:
            raise ValueError("unsupported command %s in %s" % (cmd, d[:60]))
    out.append(0)
    return bytes(out)


def css_classes(path):
    css = open(path, encoding="utf-8").read()
    out = {}
    for sel, val in re.findall(r"((?:\.fa-[\w-]+\s*,\s*)*\.fa-[\w-]+)\s*\{\s*--fa:\s*\"((?:\\.|[^\"\\])*)\"", css):
        m = re.fullmatch(r"\\([0-9a-fA-F]+) ?", val)
        if m:
            cp = int(m.group(1), 16)
        elif len(val) == 1:
            cp = ord(val)
        elif len(val) == 2 and val[0] == "\\":
            cp = ord(val[1])
        else:
            raise ValueError(val)
        for name in re.findall(r"\.fa-([\w-]+)", sel):
            out[name] = cp
    return out


def main(pkg, dest):
    version = json.load(open(os.path.join(pkg, "package.json")))["version"]
    meta = json.load(open(os.path.join(pkg, "metadata", "icon-families.json")))
    classes = {}
    # mmdc links brands.css before fontawesome.css, so the latter wins a tie.
    for f in ["brands.css", "fontawesome.css"]:
        classes.update(css_classes(os.path.join(pkg, "css", f)))
    glyphs = {s: {} for s in STYLES}
    for name, icon in meta.items():
        cp = int(icon["unicode"], 16)
        for style in STYLES:
            svg = os.path.join(pkg, "svgs", style, name + ".svg")
            if not os.path.exists(svg):
                continue
            text = open(svg, encoding="utf-8").read()
            vb = [float(v) for v in re.search(r'viewBox="([^"]*)"', text).group(1).split()]
            assert vb[0] == 0 and vb[1] == 0 and vb[3] == 512, (name, vb)
            paths = re.findall(r' d="([^"]*)"', text)
            assert len(paths) == 1, name
            glyphs[style][cp] = (int(vb[2]), encode_path(paths[0]))
    blob = bytearray()
    out = bytearray(b"FAI1")
    out += bytes([len(version)]) + version.encode()
    out += struct.pack("<I", len(classes))
    for name in sorted(classes):
        b = name.encode()
        out += bytes([len(b)]) + b + struct.pack("<I", classes[name])
    for style in STYLES:
        g = glyphs[style]
        out += struct.pack("<I", len(g))
        for cp in sorted(g):
            adv, path = g[cp]
            out += struct.pack("<IHI", cp, adv, len(blob))
            blob += path
    for style in STYLES:
        out += struct.pack("<HI", 384, len(blob))
        blob += encode_path(NOTDEF[style])
    out += struct.pack("<I", len(blob)) + blob
    os.makedirs(os.path.dirname(dest) or ".", exist_ok=True)
    open(dest, "wb").write(out)
    print(
        "%s: Font Awesome Free %s, %d classes, %s glyphs, %d bytes"
        % (dest, version, len(classes), "/".join(str(len(glyphs[s])) for s in STYLES), len(out))
    )


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
