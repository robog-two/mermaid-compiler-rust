#!/usr/bin/env python3
"""Build the bundled web fonts (assets/fonts/*.ttf) from the fontsource packages
mermaid-cli 12.2.0 loads, so PNG and PDF output draw the text mmdc draws in them.

* Recursive Variable (@fontsource-variable/recursive, the `wght` subsets mmdc's
  index.css links): each subset instanced at wght 400 and 700 (Sans Linear; the
  other axes are already fixed in these files), merged into one static font per
  weight, named "Recursive Variable" Regular / Bold; and those outlines slanted
  by Blink's synthetic oblique (x += y / 4) as Italic / Bold Italic.
* Open Sans (@fontsource/open-sans 400 and 400 italic): the subsets merged into
  one font per style. mmdc loads no bold face, so Chromium emboldens the regular
  outline (Skia: FreeType's FT_Outline_EmboldenXY by em/24, which grows the ink
  em/48 on each side and keeps the advances); the Bold and Bold Italic files
  are those emboldened outlines, so a rasteriser that picks a bold face draws
  what Chromium draws.

Hinting is dropped (resvg and PDF viewers draw unhinted). The fonts are
licensed under the SIL Open Font License 1.1 (no Reserved Font Names).

Requires fontTools (with brotli). Usage: gen_bundled_fonts.py <node_modules> <out dir>
"""
import io
import math
import os
import sys

from fontTools import merge, subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

NM, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)

REC_SUBSETS = ['latin', 'latin-ext', 'vietnamese', 'cyrillic-ext']
OS_SUBSETS = ['latin', 'latin-ext', 'vietnamese', 'greek', 'greek-ext', 'cyrillic', 'cyrillic-ext',
              'math', 'symbols']


def load(path):
    f = TTFont(path)
    f.flavor = None
    f.recalcTimestamp = False
    buf = io.BytesIO()
    f.save(buf)
    buf.seek(0)
    return TTFont(buf)


def merged(fonts):
    paths = []
    for i, f in enumerate(fonts):
        p = os.path.join(OUT, f'.part{i}.ttf')
        f.save(p)
        paths.append(p)
    m = merge.Merger().merge(paths)
    for p in paths:
        os.remove(p)
    return m


def shrink(font):
    """Keep one glyph per character (the merge's duplicates go), no hinting."""
    opts = subset.Options()
    opts.hinting = False
    opts.layout_features = ['*']
    opts.name_IDs = ['*']
    opts.name_languages = ['*']
    opts.notdef_outline = True
    opts.glyph_names = False
    opts.drop_tables += ['STAT', 'MVAR', 'HVAR', 'gasp']
    s = subset.Subsetter(opts)
    s.populate(unicodes=font.getBestCmap().keys())
    s.subset(font)
    return font


def rename(font, family, style, weight, italic, stamp):
    # reproducible files: the source's timestamps, not the build's
    font.recalcTimestamp = False
    font['head'].created = font['head'].modified = stamp
    name = font['name']
    full = family if style == 'Regular' else f'{family} {style}'
    ps = (family + '-' + style).replace(' ', '')
    for rec in list(name.names):
        if rec.nameID in (16, 17, 21, 22, 25):
            name.removeNames(nameID=rec.nameID)
    for nid, val in ((1, family), (2, style), (3, f'{ps};mmdc-bundled'), (4, full), (6, ps)):
        name.setName(val, nid, 3, 1, 0x409)
        name.setName(val, nid, 1, 0, 0)
    os2 = font['OS/2']
    os2.usWeightClass = weight
    sel = os2.fsSelection & ~0b1100001  # italic, bold, regular
    sel |= (1 if italic else 0) | (0b100000 if weight >= 700 else 0)
    if not italic and weight < 700:
        sel |= 0b1000000
    os2.fsSelection = sel
    font['head'].macStyle = (1 if weight >= 700 else 0) | (2 if italic else 0)


def embolden(font, strength_units):
    """FreeType's FT_Outline_EmboldenXY (both axes) on every glyph, then the outline
    moved back by half the strength, so the ink grows strength/2 on each side and
    the advances stay (Skia's fake bold, as Chromium draws a missing bold face)."""
    glyf = font['glyf']
    xs = ys = strength_units / 2.0
    for gname in font.getGlyphOrder():
        g = glyf[gname]
        if g.isComposite() or g.numberOfContours <= 0:
            continue
        coords = [list(p) for p in g.coordinates]
        ends = g.endPtsOfContours
        # orientation: TrueType outlines are clockwise (filled on the right)
        area = 0.0
        first = 0
        for last in ends:
            for i in range(first, last + 1):
                j = i + 1 if i < last else first
                area += coords[i][0] * coords[j][1] - coords[j][0] * coords[i][1]
            first = last + 1
        truetype = area < 0
        new = [p[:] for p in coords]
        first = 0
        for last in ends:
            n = last - first + 1
            if n < 2:
                first = last + 1
                continue
            pts = coords[first:last + 1]
            for idx in range(n):
                prev = pts[idx - 1]
                cur = pts[idx]
                nxt = pts[(idx + 1) % n]
                # in / out unit directions, skipping coincident neighbours
                k = 1
                while (prev[0], prev[1]) == (cur[0], cur[1]) and k < n:
                    k += 1
                    prev = pts[idx - k]
                k = 1
                while (nxt[0], nxt[1]) == (cur[0], cur[1]) and k < n:
                    k += 1
                    nxt = pts[(idx + k) % n]
                ix, iy = cur[0] - prev[0], cur[1] - prev[1]
                ox, oy = nxt[0] - cur[0], nxt[1] - cur[1]
                li, lo = math.hypot(ix, iy), math.hypot(ox, oy)
                if li == 0 or lo == 0:
                    continue
                ix, iy, ox, oy = ix / li, iy / li, ox / lo, oy / lo
                d = ix * ox + iy * oy
                sx = sy = 0.0
                if d > -0.9375:
                    d += 1.0
                    sx, sy = iy + oy, ix + ox
                    if truetype:
                        sx = -sx
                    else:
                        sy = -sy
                    q = ox * iy - oy * ix
                    if truetype:
                        q = -q
                    lmin = min(li, lo)
                    sx = sx * xs / d if xs * q <= lmin * d else sx * lmin / q
                    sy = sy * ys / d if ys * q <= lmin * d else sy * lmin / q
                new[first + idx][0] = cur[0] + sx
                new[first + idx][1] = cur[1] + sy
            first = last + 1
        g.coordinates = type(g.coordinates)([(round(x), round(y)) for x, y in new])
        g.recalcBounds(glyf)


def build_recursive():
    files = [f'{NM}/@fontsource-variable/recursive/files/recursive-{s}-wght-normal.woff2' for s in REC_SUBSETS]
    stamp = load(files[0])['head'].modified
    for w, style in ((400, 'Regular'), (700, 'Bold')):
        parts = [instancer.instantiateVariableFont(load(p), {'wght': w}, updateFontNames=False) for p in files]
        f = shrink(merged(parts))
        rename(f, 'Recursive Variable', style, w, False, stamp)
        out = os.path.join(OUT, f'recursive-{w}.ttf')
        f.save(out)
        print(out, os.path.getsize(out), len(f.getGlyphOrder()))
        # No italic face is loaded either: Blink slants the upright one
        # (`setSkewX(-1/4)`); the Italic files hold those slanted outlines.
        f = load_copy(f)
        oblique(f, 0.25)
        rename(f, 'Recursive Variable', 'Bold Italic' if w >= 700 else 'Italic', w, True, stamp)
        f['post'].italicAngle = -14.036
        f['hhea'].caretSlopeRise, f['hhea'].caretSlopeRun = 4, 1
        out = os.path.join(OUT, f'recursive-{w}-italic.ttf')
        f.save(out)
        print(out, os.path.getsize(out), len(f.getGlyphOrder()))


def oblique(font, k):
    """Slant every outline by x += k * y (components' offsets too)."""
    glyf = font['glyf']
    for gname in font.getGlyphOrder():
        g = glyf[gname]
        if g.isComposite():
            for c in g.components:
                c.x = round(c.x + k * c.y)
        elif g.numberOfContours > 0:
            g.coordinates = type(g.coordinates)([(round(x + k * y), y) for x, y in g.coordinates])
    for gname in font.getGlyphOrder():
        glyf[gname].recalcBounds(glyf)


def build_open_sans():
    for italic in (False, True):
        tag = 'italic' if italic else 'normal'
        files = [f'{NM}/@fontsource/open-sans/files/open-sans-{s}-400-{tag}.woff2' for s in OS_SUBSETS]
        stamp = load(files[0])['head'].modified
        base = shrink(merged([load(p) for p in files]))
        for bold in (False, True):
            f = load_copy(base)
            style = ('Bold ' if bold else '') + ('Italic' if italic else '')
            style = style.strip() or 'Regular'
            if bold:
                embolden(f, f['head'].unitsPerEm / 24.0)
            rename(f, 'Open Sans', style, 700 if bold else 400, italic, stamp)
            name = 'open-sans-' + ('700' if bold else '400') + ('-italic' if italic else '') + '.ttf'
            out = os.path.join(OUT, name)
            f.save(out)
            print(out, os.path.getsize(out), len(f.getGlyphOrder()))


def load_copy(font):
    buf = io.BytesIO()
    font.save(buf)
    buf.seek(0)
    return TTFont(buf)


build_recursive()
build_open_sans()
