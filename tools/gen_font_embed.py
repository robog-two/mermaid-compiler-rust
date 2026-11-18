#!/usr/bin/env python3
"""Generate assets/fontembed/ from SVGs that real mmdc (mermaid-cli) rendered.

mmdc embeds the web fonts a diagram uses into its SVG as extra `<style>`
elements (mermaid-cli `src/browser/fontEmbedder.js`): one per stylesheet it
loads, holding the licence comment and the stylesheet's rules with each
`@font-face` source replaced by a woff2 `data:` URL. This script cuts those
elements out of mmdc's output, so the Rust writer can reproduce them byte for
byte: the CSS goes to `<name>.css` with every data URL replaced by `@@FONT@@`,
and the decoded woff2 files go to `<name>-<n>.woff2` (n counts the markers).

Usage (standard library only; every SVG must come from mmdc 12.2.0 / mermaid
12.3.0 with fonts embedded, and together they must use every font):

    mmdc -i flow.mmd -o recursive_fa.svg      # `fa:fa-car`, `fab:fa-github`, `far:fa-user`
    mmdc -i c4.mmd   -o open_sans.svg         # C4 text is Open Sans
    python3 tools/gen_font_embed.py assets/fontembed recursive_fa.svg open_sans.svg
"""

import base64
import re
import sys

STYLE = re.compile(r"<style[^>]*>(.*?)</style>", re.S)
DATA = re.compile(r'url\("data:font/woff2;base64,([A-Za-z0-9+/=]+)"\)')
FACE = re.compile(
    r'font-family: "([^"]*)"; font-style: (\w+); (?:font-display: \w+; )?font-weight: ([^;]*);'
)


def name_of(css):
    """The sheet's name, from its first @font-face (none: Font Awesome's base sheet)."""
    m = FACE.search(css)
    if not m:
        return "fontawesome" if css.startswith("/* Fonticons") else None
    fam, style, weight = m.groups()
    if fam == "Recursive Variable":
        return "recursive"
    if fam == "Open Sans":
        return "open-sans-italic" if style == "italic" else "open-sans"
    if fam == "Font Awesome 7 Brands":
        return "fa-brands"
    if fam == "Font Awesome 7 Free":
        return "fa-solid" if weight == "900" else "fa-regular"
    return None


def main():
    out, svgs = sys.argv[1], sys.argv[2:]
    found = {}
    for path in svgs:
        text = open(path, encoding="utf-8").read()
        # The first <style> is mermaid's own; the rest are the embedded sheets.
        for m in list(STYLE.finditer(text))[1:]:
            css = m.group(1)
            name = name_of(css)
            if name:
                found[name] = css
    want = {
        "recursive", "open-sans", "open-sans-italic",
        "fontawesome", "fa-solid", "fa-regular", "fa-brands",
    }
    if set(found) != want:
        sys.exit(f"missing sheets: {sorted(want - set(found))}")
    for name, css in sorted(found.items()):
        fonts = DATA.findall(css)
        for i, b64 in enumerate(fonts):
            with open(f"{out}/{name}-{i}.woff2", "wb") as f:
                f.write(base64.b64decode(b64))
        tmpl = DATA.sub('url("@@FONT@@")', css)
        with open(f"{out}/{name}.css", "w", encoding="utf-8", newline="") as f:
            f.write(tmpl)
        print(f"{name}: {len(fonts)} fonts, {len(tmpl)} bytes of CSS")


if __name__ == "__main__":
    main()
