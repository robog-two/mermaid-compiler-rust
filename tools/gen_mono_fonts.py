#!/usr/bin/env python3
"""Build the bundled monospace fonts (assets/fonts/dejavu-sans-mono-*.ttf and
liberation-mono-*.ttf) from the system packages fonts-dejavu-core and
fonts-liberation, so PNG and PDF output draw `monospace` / `DejaVu Sans Mono`
and `Courier New` / `courier` / `Liberation Mono` text the way Chromium does.

Each file is subset to Basic Latin, Latin-1, Latin Extended-A, general
punctuation, arrows, box drawing and a few symbols (the check marks, minus,
comparisons), without hinting or layout tables. DejaVu keeps its names (the
Bitstream Vera licence only reserves "Bitstream" and "Vera"). Liberation Mono
carries the Reserved Font Name "Liberation" (SIL OFL 1.1), so its subset is
renamed "Courier Metric Mono" (PostScript names CourierMetricMono-*).

Requires fontTools. Usage: gen_mono_fonts.py <out dir>
"""
import os
import sys

from fontTools import subset
from fontTools.ttLib import TTFont

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)

UNICODES = (
    list(range(0x20, 0x7F))
    + list(range(0xA0, 0x180))
    + list(range(0x2000, 0x2070))
    + list(range(0x2190, 0x2200))
    + list(range(0x2500, 0x2580))
    + [0x2212, 0x2260, 0x2264, 0x2265, 0x2713, 0x2714, 0x2717, 0x2718, 0xFFFD]
)

STYLES = [('Regular', 'regular'), ('Bold', 'bold'), ('Italic', 'italic'),
          ('BoldItalic', 'bolditalic')]
DEJAVU = {'Regular': 'DejaVuSansMono', 'Bold': 'DejaVuSansMono-Bold',
          'Italic': 'DejaVuSansMono-Oblique', 'BoldItalic': 'DejaVuSansMono-BoldOblique'}


def build(src, dst, rename=None):
    font = TTFont(src)
    opts = subset.Options()
    opts.hinting = False
    opts.layout_features = []
    opts.glyph_names = False
    opts.notdef_outline = True
    opts.name_IDs = [0, 1, 2, 3, 4, 5, 6, 13, 14]
    opts.drop_tables += ['DSIG', 'GSUB', 'GPOS', 'GDEF', 'kern', 'gasp', 'hdmx', 'LTSH', 'VDMX']
    s = subset.Subsetter(opts)
    s.populate(unicodes=UNICODES)
    s.subset(font)
    if rename:
        family, style = rename
        ps = family.replace(' ', '') + '-' + style
        for rec in font['name'].names:
            if rec.nameID == 1:
                rec.string = family
            elif rec.nameID == 3:
                rec.string = ps
            elif rec.nameID == 4:
                rec.string = family + ('' if style == 'Regular' else ' ' + style)
            elif rec.nameID == 6:
                rec.string = ps
    font.save(dst)
    print(dst, os.path.getsize(dst))


for style, slug in STYLES:
    build('/usr/share/fonts/truetype/dejavu/%s.ttf' % DEJAVU[style],
          os.path.join(OUT, 'dejavu-sans-mono-%s.ttf' % slug))
    build('/usr/share/fonts/truetype/liberation/LiberationMono-%s.ttf' % style,
          os.path.join(OUT, 'liberation-mono-%s.ttf' % slug),
          ('Courier Metric Mono', style))
