#!/usr/bin/env python3
"""Generate src/text/webfont_synthetic.rs: what Chromium's synthetic styles of
the web fonts mmdc loads need to measure a glyph's ink, as `getBBox()` does.

* Oblique Recursive. Recursive Variable as mmdc loads it (the fontsource
  `wght` subsets) has no italic, so Blink asks Skia for a synthetic oblique
  (`setSkewX(-1/4)`). A slanted glyph's box is the control box of its outline
  after each point is scaled to 26.6 fixed point and sheared (`x + y / 4`, in
  floating point), rounded out to whole pixels. Which point is outermost hangs
  on that rounding only between points within a few font units of each other,
  so per character and weight (`wght` 400 and 700) this keeps the points whose
  `x + y / 4` lies within `SLANT_TOL` units of the least and of the greatest
  (font units, as HarfBuzz draws the instance: single precision).

* Bold Open Sans. mmdc loads Open Sans 400 and 400 italic only, so Skia makes
  the bold by stroking the outline (and filling it) `size * k` wide, `k` going
  from 1/24 at 9px to 1/32 at 36px (`kStdFakeBoldInterpKeys`): miter joins
  (limit 4), each quadratic's offset a quadratic whose control point is where
  the offset tangents at its ends meet. The stroked path's control box is
  rounded out to whole pixels. Every point of it is a point of the outline
  moved by the stroke's half width `r` times a factor `a` fixed by the
  outline's directions there: `x + r * a`. Per character this keeps the pairs
  `(x, a)` that come within `BOLD_TOL` units of the least and of the greatest
  `x + r * a` for some `r` between em/64 and em/48.

The characters are those of the tables in src/text/webfont_data.rs, each from
the face file the CSS gives it.

Requires fontTools (with brotli) and uharfbuzz.
Usage: gen_synthetic_tables.py <node_modules dir> <out.rs>
"""
import io
import math
import re
import struct
import sys

import uharfbuzz as hb
from fontTools.ttLib import TTFont

NM = sys.argv[1]
OUT = sys.argv[2]
SLANT_TOL = 3.5
BOLD_TOL = 4.0
MITER_LIMIT = 4.0


def parse_ranges(spec):
    out = []
    for part in spec.split(','):
        part = part.strip().upper().removeprefix('U+')
        if '-' in part:
            a, b = part.split('-')
            out.append((int(a, 16), int(b, 16)))
        elif '?' in part:
            out.append((int(part.replace('?', '0'), 16), int(part.replace('?', 'F'), 16)))
        else:
            out.append((int(part, 16), int(part, 16)))
    return out


def faces(css_rel):
    css = open(f'{NM}/{css_rel}').read()
    base = css_rel.rsplit('/', 1)[0]
    out = []
    for block in re.findall(r'@font-face\s*{([^}]*)}', css):
        src = re.search(r"url\(\./files/([^)]+?\.woff2)\)", block).group(1)
        rng = re.search(r'unicode-range:\s*([^;]+);', block)
        tt = TTFont(f'{NM}/{base}/files/{src}')
        tt.flavor = None
        buf = io.BytesIO()
        tt.save(buf)
        face = hb.Face(hb.Blob(buf.getvalue()))
        ranges = parse_ranges(rng.group(1)) if rng else [(0, 0x10FFFF)]
        out.append((tt.getBestCmap(), ranges, face))
    return out


def owners(files):
    """the characters of a face (as webfont_data.rs lists them) and the file
    drawing each: the last face whose range covers it and has its glyph"""
    owner = {}
    for fi in reversed(range(len(files))):
        cmap, ranges, _ = files[fi]
        for cp in cmap:
            if cp >= 0x20 and cp not in owner and any(a <= cp <= b for a, b in ranges):
                owner[cp] = fi
    cps = sorted(c for c in owner if c <= 0xFFFF and not (0xD800 <= c <= 0xDFFF))
    return cps, owner


class Pen:
    """contours as lists of (x, y, on_curve)"""

    def __init__(self):
        self.cs = []

    def moveTo(self, p):
        self.cs.append([(p[0], p[1], True)])

    def lineTo(self, p):
        self.cs[-1].append((p[0], p[1], True))

    def qCurveTo(self, *ps):
        for p in ps[:-1]:
            self.cs[-1].append((p[0], p[1], False))
        self.cs[-1].append((ps[-1][0], ps[-1][1], True))

    def curveTo(self, *ps):
        raise ValueError('cubic outline')

    def closePath(self):
        c = self.cs[-1]
        if len(c) > 1 and c[0][:2] == c[-1][:2]:
            c.pop()

    def endPath(self):
        pass


def outline(font, cp, gid=None):
    if gid is None:
        gid = font.get_nominal_glyph(cp)
    pen = Pen()
    if gid is not None:
        font.draw_glyph_with_pen(gid, pen)
    return pen.cs


def norm(v):
    length = math.hypot(*v)
    return (v[0] / length, v[1] / length) if length else (0.0, 0.0)


def segments(c):
    """a closed contour as lines ('L', p0, p1) and quadratics ('Q', p0, c, p1)"""
    start = next(i for i in range(len(c)) if c[i][2])
    pts = c[start:] + c[:start]
    segs = []
    cur = pts[0][:2]
    pend = None
    for (x, y, on) in pts[1:] + [pts[0]]:
        if on:
            segs.append(('L', cur, (x, y)) if pend is None else ('Q', cur, pend, (x, y)))
            pend = None
            cur = (x, y)
        else:
            if pend is not None:
                mid = ((pend[0] + x) / 2, (pend[1] + y) / 2)
                segs.append(('Q', cur, pend, mid))
                cur = mid
            pend = (x, y)
    return [s for s in segs if not (s[0] == 'L' and s[1] == s[2])]


def tangents(s):
    if s[0] == 'L':
        t = norm((s[2][0] - s[1][0], s[2][1] - s[1][1]))
        return t, t
    p0, c, p1 = s[1], s[2], s[3]
    chord = norm((p1[0] - p0[0], p1[1] - p0[1]))
    t0 = norm((c[0] - p0[0], c[1] - p0[1]))
    t1 = norm((p1[0] - c[0], p1[1] - c[1]))
    t0 = t0 if t0 != (0.0, 0.0) else chord
    t1 = t1 if t1 != (0.0, 0.0) else chord
    return t0, t1


def stroke_pairs(cs):
    """(x, a): every point of the stroked outline is x + r * a"""
    out = []
    for c in cs:
        segs = segments(c)
        for k, s in enumerate(segs):
            t0, t1 = tangents(s)
            n0 = (-t0[1], t0[0])
            n1 = (-t1[1], t1[0])
            for sgn in (1, -1):
                out.append((s[1][0], sgn * n0[0]))
                out.append((s[-1][0], sgn * n1[0]))
                if s[0] == 'Q':
                    d = 1 + n0[0] * n1[0] + n0[1] * n1[1]
                    if d > 1e-9:
                        out.append((s[2][0], sgn * (n0[0] + n1[0]) / d))
            ta, _ = tangents(segs[(k + 1) % len(segs)])
            na = (-ta[1], ta[0])
            dot = n1[0] * na[0] + n1[1] * na[1]
            if dot < 1 - 1e-9:
                cos_half = math.sqrt((1 + dot) / 2)
                if cos_half > 1e-9 and 1 / cos_half <= MITER_LIMIT:
                    bis = norm((n1[0] + na[0], n1[1] + na[1]))
                    for sgn in (1, -1):
                        out.append((s[-1][0], sgn * bis[0] / cos_half))
        for (x, _, _) in c:
            out.append((x, 0.0))
    return out


def bold_candidates(pairs, upem):
    lo, hi = set(), set()
    for i in range(21):
        r = (1 / 64 + (1 / 48 - 1 / 64) * i / 20) * upem
        vals = [x + r * a for x, a in pairs]
        top, bottom = max(vals), min(vals)
        for p, v in zip(pairs, vals):
            if v >= top - BOLD_TOL:
                hi.add(p)
            if v <= bottom + BOLD_TOL:
                lo.add(p)
    return sorted(lo), sorted(hi)


def f32(v):
    """the shortest decimal that reads back as the same single-precision value"""
    f = struct.unpack('f', struct.pack('f', v))[0]
    for digits in range(1, 12):
        s = f'{f:.{digits}g}'
        if struct.unpack('f', struct.pack('f', float(s)))[0] == f:
            break
    return repr(float(s))


def table(out, name, ty, rows, per, doc):
    out.append(doc)
    out.append('#[rustfmt::skip]')
    out.append(f'pub(super) static {name}: [{ty}; {len(rows)}] = [')
    for i in range(0, len(rows), per):
        out.append('    ' + ' '.join(f'{r},' for r in rows[i:i + per]))
    out.append('];')
    out.append('')


def main():
    out = ['''//! What Chromium's synthetic styles of the web fonts need to measure a
//! glyph's ink: the outline points that bound Recursive slanted for
//! `font-style: italic`, and the stroke offsets that bound Open Sans
//! emboldened for bold. Generated by `tools/gen_synthetic_tables.py` from
//! @fontsource-variable/recursive and @fontsource/open-sans (SIL Open Font
//! License 1.1); do not edit.
''']
    # oblique Recursive
    files = faces('@fontsource-variable/recursive/index.css')
    cps, owner = owners(files)
    assert len(cps) == 616, len(cps)
    fonts = {}
    pts_out = []
    rows = {}
    for wght in (400, 700):
        rows[wght] = []
        for cp in cps:
            key = (owner[cp], wght)
            if key not in fonts:
                f = hb.Font(files[owner[cp]][2])
                f.set_variations({'wght': wght})
                fonts[key] = f
            pts = [(x, y) for c in outline(fonts[key], cp) for (x, y, _) in c]
            if not pts:
                rows[wght].append((len(pts_out), 0, 0))
                continue
            v = [x + y / 4 for x, y in pts]
            lo, hi = min(v), max(v)
            low = sorted({p for p, s in zip(pts, v) if s <= lo + SLANT_TOL})
            high = sorted({p for p, s in zip(pts, v) if s >= hi - SLANT_TOL})
            assert len(low) < 256 and len(high) < 256
            rows[wght].append((len(pts_out), len(low), len(high)))
            pts_out.extend(low + high)
    assert len(pts_out) < 65536
    table(out, 'RECURSIVE_SLANT', '(u16, u8, u8)', [f'({a}, {b}, {c})' for a, b, c in rows[400]], 8,
          '''/// Per character of `RECURSIVE_CHARS` at `wght` 400: the index of its first
/// point in [`RECURSIVE_SLANT_POINTS`], how many bound it on the left (least
/// `x + y / 4`) and how many on the right, which follow.''')
    table(out, 'RECURSIVE_SLANT_BOLD', '(u16, u8, u8)', [f'({a}, {b}, {c})' for a, b, c in rows[700]], 8,
          '/// [`RECURSIVE_SLANT`] at `wght` 700.')
    table(out, 'RECURSIVE_SLANT_POINTS', '(f32, f32)', [f'({f32(x)}, {f32(y)})' for x, y in pts_out], 4,
          '/// Outline points in font units, as HarfBuzz draws the instance.')
    # bold Open Sans
    pairs_out = []
    lig_tables = []
    for name, css in (('OPEN_SANS_BOLD', '@fontsource/open-sans/400.css'),
                      ('OPEN_SANS_ITALIC_BOLD', '@fontsource/open-sans/400-italic.css')):
        files = faces(css)
        cps, owner = owners(files)
        assert len(cps) == 979, len(cps)
        fonts = [hb.Font(f[2]) for f in files]
        rws = []
        for cp in cps:
            cs = outline(fonts[owner[cp]], cp)
            if not cs:
                rws.append((len(pairs_out), 0, 0))
                continue
            upem = files[owner[cp]][2].upem
            lo, hi = bold_candidates(stroke_pairs(cs), upem)
            assert len(lo) < 256 and len(hi) < 256
            rws.append((len(pairs_out), len(lo), len(hi)))
            pairs_out.extend(lo + hi)
        # the `liga` ligatures, one glyph each (drawn from the face of their f)
        font = fonts[owner[ord('f')]]
        upem = files[owner[ord('f')]][2].upem
        ligs = []
        for lig in ('ff', 'fi', 'fl', 'ffi', 'ffl'):
            buf = hb.Buffer()
            buf.add_str(lig)
            buf.guess_segment_properties()
            hb.shape(font, buf, {})
            assert len(buf.glyph_infos) == 1, lig
            gid = buf.glyph_infos[0].codepoint
            e = font.get_glyph_extents(gid)
            lo, hi = bold_candidates(stroke_pairs(outline(font, None, gid)), upem)
            ligs.append(f'("{lig}", {e.x_bearing}, {e.x_bearing + e.width}, {len(pairs_out)}, {len(lo)}, {len(hi)})')
            pairs_out.extend(lo + hi)
        lig_tables.append((name.replace('_BOLD', '_LIGATURES'), ligs))
        doc = ('''/// Per character of `OPEN_SANS_CHARS`: the index of its first pair in
/// [`OPEN_SANS_BOLD_PAIRS`], how many bound its emboldened ink on the left and
/// how many on the right, which follow.''' if name == 'OPEN_SANS_BOLD' else
               '/// [`OPEN_SANS_BOLD`] for `OPEN_SANS_ITALIC_CHARS`.')
        table(out, name, '(u16, u8, u8)', [f'({a}, {b}, {c})' for a, b, c in rws], 8, doc)
    for name, ligs in lig_tables:
        doc = ('''/// Open Sans' `liga` ligatures, each one glyph: its text, its ink's `xMin`
/// and `xMax` (font units), and its emboldened ink as [`OPEN_SANS_BOLD`] has
/// a character's.''' if name == 'OPEN_SANS_LIGATURES' else
               '/// [`OPEN_SANS_LIGATURES`] of the italic.')
        table(out, name, '(&str, i16, i16, u16, u8, u8)', ligs, 1, doc)
    assert len(pairs_out) < 65536
    assert all(2 * x == int(2 * x) for x, _ in pairs_out)
    table(out, 'OPEN_SANS_BOLD_PAIRS', '(i16, f32)', [f'({int(2 * x)}, {f32(a)})' for x, a in pairs_out], 6,
          '''/// `(x2, a)`: an outline point's `x` in half font units (a quadratic's
/// implied on-curve point between two control points falls on a half) and
/// how far the stroke moves it per unit of its half width.''')
    open(OUT, 'w').write('\n'.join(out).rstrip('\n') + '\n')
    print('slant points', len(pts_out), 'bold pairs', len(pairs_out))


main()
