#!/usr/bin/env python3
"""Generate src/text/webfont.rs: the metrics of the web fonts mmdc loads.

mermaid-cli 12.2.0 links `@fontsource-variable/recursive/index.css` and
`@fontsource/open-sans/{400,400-italic}.css` before rendering, so text in
`"Recursive Variable"` (the redux themes' font) and `"Open Sans"` (sequence actors,
gantt tasks, C4) is measured in those faces. This script reads the fontsource
packages' CSS (for each face file's `unicode-range`) and woff2 files and writes:

* per character the face file that draws it (the last `@font-face` whose range
  covers it and whose font has the glyph, as the CSS Fonts spec searches), its
  advance (HarfBuzz, default features) and its ink box (`glyf` extents at the
  variable font's `wght` 400 and 700 instances);
* the kerning HarfBuzz applies between two characters drawn from the same file
  (`kern` feature; each pair shaped on its own), compressed into left / right
  classes (characters with identical rows / columns) and a class matrix;
* the ligatures (`liga`: Open Sans' ff, fi, fl, ffi, ffl) and their advances;
* the vertical metrics Chromium reads (hhea / OS/2, MVAR-varied x-height).

Recursive's Sans Linear advances and kerning do not change with `wght` (checked
here for 300..1000), so one advance table serves every weight. Open Sans has only
a 400 face; Chromium synthesises bold without changing advances.

Requires fontTools (with brotli) and uharfbuzz.
Usage: gen_font_tables.py <node_modules dir> <out.rs> [<assets dir>]
With an assets dir it also writes the bundled TrueType files (see `bundle`).
"""
import re
import sys

import uharfbuzz as hb
from fontTools.ttLib import TTFont

NM = sys.argv[1]
OUT = sys.argv[2]
ASSETS = sys.argv[3] if len(sys.argv) > 3 else None

FACES = {
    # name: (css files in mmdc's order, variable?)
    'RECURSIVE': (['@fontsource-variable/recursive/index.css'], True),
    'OPEN_SANS': (['@fontsource/open-sans/400.css'], False),
    'OPEN_SANS_ITALIC': (['@fontsource/open-sans/400-italic.css'], False),
}


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


def faces_of(css_rel):
    """[(woff2 path, ranges)] in definition order."""
    path = f'{NM}/{css_rel}'
    css = open(path).read()
    out = []
    for block in re.findall(r'@font-face\s*{([^}]*)}', css):
        src = re.search(r"url\(\./files/([^)]+?\.woff2)\)", block).group(1)
        rng = re.search(r'unicode-range:\s*([^;]+);', block)
        base = css_rel.rsplit('/', 1)[0]
        out.append((f'{NM}/{base}/files/{src}', parse_ranges(rng.group(1)) if rng else [(0, 0x10FFFF)]))
    return out


class File:
    def __init__(self, path, ranges, variable):
        self.path = path
        self.ranges = ranges
        tt = TTFont(path)
        tt.flavor = None
        self.tt = tt
        self.cmap = tt.getBestCmap()
        import io
        buf = io.BytesIO()
        tt.save(buf)
        self.data = buf.getvalue()
        self.face = hb.Face(hb.Blob(self.data))
        self.upem = self.face.upem
        self.variable = variable
        self.fonts = {}

    def font(self, wght):
        if wght not in self.fonts:
            f = hb.Font(self.face)
            if self.variable:
                f.set_variations({'wght': wght})
            self.fonts[wght] = f
        return self.fonts[wght]

    def covers(self, cp):
        return cp in self.cmap and any(a <= cp <= b for a, b in self.ranges)

    def shape(self, s, wght=400):
        buf = hb.Buffer()
        buf.add_str(s)
        buf.guess_segment_properties()
        hb.shape(self.font(wght), buf, {})
        return buf

    def advances(self, s, wght=400):
        return [p.x_advance for p in self.shape(s, wght).glyph_positions]

    def ink(self, cp, wght):
        gid = self.cmap[cp]
        g = self.tt.getGlyphID(gid)
        e = self.font(wght).get_glyph_extents(g)
        if e is None or e.width == 0:
            return (0, 0, 0, 0)
        return (e.x_bearing, e.x_bearing + e.width, e.y_bearing + e.height, e.y_bearing)


def build(name, css_files, variable):
    files = []
    for css in css_files:
        files += [File(p, r, variable) for p, r in faces_of(css)]
    # the last face whose range covers a character (and has its glyph) draws it
    owner = {}
    for fi in reversed(range(len(files))):
        for cp in files[fi].cmap:
            if cp >= 0x20 and cp not in owner and files[fi].covers(cp):
                owner[cp] = fi
    cps = sorted(c for c in owner if c <= 0xFFFF and not (0xD800 <= c <= 0xDFFF))
    upem = files[0].upem
    adv = {}
    for cp in cps:
        a = files[owner[cp]].advances(chr(cp))
        adv[cp] = sum(a)
        if variable:
            for w in (300, 700, 1000):
                assert sum(files[owner[cp]].advances(chr(cp), w)) == adv[cp], (name, cp, w)
    # kerning within one file; marks (zero-advance combining) are left out
    kc = [c for c in cps if adv[c] > 0 and not (0x300 <= c <= 0x36F)]
    kern = {}
    ligs = {}
    for a in kc:
        fa = files[owner[a]]
        for b in kc:
            if owner[b] != owner[a]:
                continue
            s = chr(a) + chr(b)
            buf = fa.shape(s)
            w = [p.x_advance for p in buf.glyph_positions]
            if len(w) != 2:
                ligs[s] = sum(w)
                continue
            k = sum(w) - adv[a] - adv[b]
            if variable and k:
                for wg in (300, 700, 1000):
                    assert sum(fa.advances(s, wg)) - adv[a] - adv[b] == k, (name, s, wg)
            if k:
                kern[(a, b)] = k
    # three-character ligatures over the two-character ones (ffi, ffl)
    for s in list(ligs):
        for c in kc:
            if owner[c] != owner[ord(s[0])]:
                continue
            t = s + chr(c)
            w = files[owner[ord(s[0])]].advances(t)
            if len(w) == 1:
                ligs[t] = w[0]
    # equivalence classes: characters with identical kerning rows (left) / columns (right)
    rows = {a: tuple(kern.get((a, b), 0) for b in kc) for a in kc}
    left_of, lrows = {}, {}
    for a in kc:
        r = rows[a]
        if r not in lrows:
            lrows[r] = len(lrows)
        left_of[a] = lrows[r]
    cols = {b: tuple(kern.get((a, b), 0) for a in kc) for b in kc}
    right_of, rcols = {}, {}
    for b in kc:
        c = cols[b]
        if c not in rcols:
            rcols[c] = len(rcols)
        right_of[b] = rcols[c]
    zero_row = tuple(0 for _ in kc)
    zero_col = tuple(0 for _ in kc)
    # class 0 is "no kerning" on both sides (zero row / column), when such exist
    def reorder(classes, zero):
        order = sorted(classes.items(), key=lambda kv: (kv[0] != zero, kv[1]))
        return {old: new for new, (_, old) in enumerate(order)}
    lmap = reorder(lrows, zero_row)
    rmap = reorder(rcols, zero_col)
    nl, nr = len(lmap), len(rmap)
    assert nl < 256 and nr < 256, (nl, nr)
    matrix = [0] * (nl * nr)
    for (a, b), k in kern.items():
        matrix[lmap[left_of[a]] * nr + rmap[right_of[b]]] = k
    left = [lmap[left_of[c]] if c in left_of else 0 for c in cps]
    right = [rmap[right_of[c]] if c in right_of else 0 for c in cps]
    # check the compression
    for (a, b), k in kern.items():
        assert matrix[left[cps.index(a)] * nr + right[cps.index(b)]] == k
    ink = [files[owner[c]].ink(c, 400) for c in cps]
    ink_bold = [files[owner[c]].ink(c, 700) for c in cps] if variable else None
    # vertical metrics (the latin file's; MVAR varies the x-height with the weight)
    lat = files[-1]
    tt = lat.tt
    hhea, os2 = tt['hhea'], tt['OS/2']
    xh = [os2.sxHeight, os2.sxHeight]
    if variable:
        from fontTools.varLib import instancer
        for i, w in enumerate((400, 700)):
            inst = instancer.instantiateVariableFont(TTFont(lat.path), {'wght': w})
            xh[i] = inst['OS/2'].sxHeight
    return dict(name=name, upem=upem, cps=cps, adv=adv, owner=owner, left=left, right=right, nl=nl, nr=nr,
                matrix=matrix, ink=ink, ink_bold=ink_bold, ligs=ligs, ascent=hhea.ascent, descent=-hhea.descent,
                line_gap=hhea.lineGap, x_height=xh, npairs=len(kern), files=files)


def rs_face(d):
    out = []
    n = d['name']
    out.append(f'/// {len(d["cps"])} characters, {d["npairs"]} kerning pairs in {d["nl"]} x {d["nr"]} classes.')
    out.append(f'pub(super) static {n}: FaceData = FaceData {{')
    out.append(f'    upem: {d["upem"]}.0,')
    out.append(f'    ascent: {d["ascent"]},')
    out.append(f'    descent: {d["descent"]},')
    out.append(f'    line_gap: {d["line_gap"]},')
    out.append(f'    x_height: [{d["x_height"][0]}, {d["x_height"][1]}],')
    out.append(f'    chars: &{n}_CHARS,')
    out.append(f'    ink: &{n}_INK,')
    out.append(f'    ink_bold: {"&" + n + "_INK_BOLD" if d["ink_bold"] else "&" + n + "_INK"},')
    out.append(f'    left: &{n}_LEFT,')
    out.append(f'    right: &{n}_RIGHT,')
    out.append(f'    classes: {d["nr"]},')
    out.append(f'    kern: &{n}_KERN,')
    out.append(f'    ligatures: &[{", ".join(f"({s!r}, {w})".replace(chr(39), chr(34)) for s, w in sorted(d["ligs"].items()))}],')
    out.append('};')
    out.append('')

    def arr(name, ty, items, per):
        out.append(f'#[rustfmt::skip]')
        out.append(f'static {name}: [{ty}; {len(items)}] = [')
        for i in range(0, len(items), per):
            out.append('    ' + ' '.join(f'{x},' for x in items[i:i + per]))
        out.append('];')
        out.append('')
    arr(f'{n}_CHARS', '(u16, u16)', [f'(0x{c:04X}, {d["adv"][c]})' for c in d['cps']], 8)
    arr(f'{n}_INK', '(i16, i16, i16, i16)', [f'({a}, {b}, {c}, {e})' for a, b, c, e in d['ink']], 6)
    if d['ink_bold']:
        arr(f'{n}_INK_BOLD', '(i16, i16, i16, i16)', [f'({a}, {b}, {c}, {e})' for a, b, c, e in d['ink_bold']], 6)
    arr(f'{n}_LEFT', 'u8', [str(x) for x in d['left']], 32)
    arr(f'{n}_RIGHT', 'u8', [str(x) for x in d['right']], 32)
    arr(f'{n}_KERN', 'i16', [str(x) for x in d['matrix']], 24)
    return '\n'.join(out)


def main():
    built = [build(n, css, var) for n, (css, var) in FACES.items()]
    head = '''//! Metrics of the web fonts mermaid-cli loads: Recursive Variable (its
//! fontsource `wght` subsets, Sans Linear) and Open Sans 400 / 400 italic.
//! Generated by `tools/gen_font_tables.py` from @fontsource-variable/recursive
//! and @fontsource/open-sans (SIL Open Font License 1.1); do not edit.

use super::webfonts::FaceData;

'''
    open(OUT, 'w').write(head + '\n'.join(rs_face(d) for d in built))
    for d in built:
        print(d['name'], 'chars', len(d['cps']), 'pairs', d['npairs'], 'classes', d['nl'], d['nr'], 'ligs', d['ligs'], 'xh', d['x_height'])


main()
