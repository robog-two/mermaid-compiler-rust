# mmdc-rs

A browser-free, pure-Rust reimplementation of [mermaid-cli](https://github.com/mermaid-js/mermaid-cli)'s `mmdc` command.

The upstream `mmdc` (v12) launches headless Chromium through Puppeteer and runs mermaid.js inside it. This project contains
**no browser, no JavaScript engine and no mermaid.js**. The diagram parsers, the layered graph layout engine, text measurement,
and the SVG and PDF writers are all written from scratch in Rust. PNG rasterisation uses [resvg](https://github.com/linebender/resvg)
(also pure Rust).

```sh
mmdc -i input.mmd -o output.svg
mmdc -i input.mmd -o output.png -t dark -b transparent -s 2
mmdc -i readme.template.md -o readme.md
cat diagram.mmd | mmdc -i - -o - -e svg > out.svg
```

## Status: what "compatible" means here

**The CLI is operationally compatible:** same flags, defaults, output naming, Markdown handling, stdin/stdout behaviour,
log lines, warnings, error messages and exit codes as mermaid-cli 12.0.0. Scripts and Makefiles that call `mmdc` should not
need changes for the supported diagram types.

**The images are not pixel-identical.** Upstream renders with mermaid.js + dagre in a browser; this is an independent
implementation of the same diagram languages, so layouts, spacing and fonts differ in detail. Byte-identical output is not
achievable without embedding a browser, which is what this project exists to avoid.

### Diagram support

Every mermaid diagram family is implemented. See **[docs/GALLERY.md](docs/GALLERY.md)** for a rendered screenshot of
each example. Each entry has its source in a collapsible block, which GitHub renders with real mermaid, so you can
compare the two directly.

| Diagram | Layout engine | Notes |
|---|---|---|
| Flowchart (`graph`, `flowchart`) | layered graph | all shapes, link types, subgraphs, styling |
| Sequence (`sequenceDiagram`) | sequence | actors, activations, notes, all frames, boxes, autonumber |
| State (`stateDiagram[-v2]`) | layered graph | composite states, fork/join/choice, notes |
| Class (`classDiagram`) | layered graph | members, generics, relations, cardinalities, namespaces |
| ER (`erDiagram`) | layered graph | attribute tables, keys, crow's-foot cardinalities |
| Requirement (`requirementDiagram`) | layered graph | all requirement/element types and relationships |
| Architecture (`architecture-beta`) | grid from side hints | groups, services, junctions. Generic icon glyphs only |
| Pie (`pie`) | geometry | `showData`, mermaid's geometry |
| Gantt (`gantt`) | time axis | date formats, `after`/`until`, excludes, milestones, markers |
| Timeline (`timeline`) | **Taffy** flex | sections, multi-event periods |
| User journey (`journey`) | **Taffy** flex | scores as faces, actors |
| Kanban (`kanban`) | **Taffy** flex | columns, cards, ticket/assignee/priority |
| Block (`block-beta`) | **Taffy** CSS grid | columns, spans, nested blocks, edges, block arrows |
| Packet (`packet-beta`) | **Taffy** CSS grid | bit ranges, `+N`, row splitting |
| C4 (`C4Context/Container/Component/Dynamic/Deployment`) | **Taffy** flex/wrap | boundaries, all element/rel kinds, style updates |
| xychart (`xychart-beta`) | geometry | bars/lines, horizontal, auto "nice" axes |
| Quadrant (`quadrantChart`) | geometry | points with styles/classes |
| Radar (`radar-beta`) | geometry | circle/polygon graticule, legend |
| Sankey (`sankey-beta`) | sankey (d3-style) | CSV input, relaxation, link colouring |
| gitGraph (`gitGraph`) | lanes | branches, merges, cherry-picks, tags, LR/TB/BT |
| Mindmap (`mindmap`) | two-sided tree | all node shapes |
| Treemap (`treemap-beta`) | squarified | sections, values, classDef |
| ZenUML (`zenuml`) | sequence (lowered) | translated into the sequence engine |
| `info` | — | prints the implementation version |

Shared features: front-matter (`title`, `config`), `%%{init: …}%%` directives, `%%` comments, `accTitle`/`accDescr`
(emitted as `<title>`/`<desc>` and used for Markdown alt text and titles, as upstream does), themes and `themeVariables`.

### Where Taffy is used, and where it isn't

[Taffy](https://github.com/DioxusLabs/taffy) implements the CSS box-layout algorithms (flexbox, grid, block). Diagrams
that are nested rectangles in rows, columns or grids are built as a box tree and laid out by Taffy through a small
pure wrapper (`src/layout/boxes.rs`): block, packet, kanban, timeline, journey, C4 boundaries and chart legends.

Taffy cannot lay out node-link graphs. CSS has no concept of ranking nodes, minimising edge crossings or routing
edges. Flowchart, state, class, ER and requirement diagrams therefore use the purpose-built layered engine
(`src/layout/`: network-simplex ranking, barycentric ordering, least-squares coordinates), just as mermaid uses
dagre rather than CSS for them.

### Flags

| Flag | Behaviour |
|---|---|
| `-i/--input`, `-o/--output`, `-e/--outputFormat`, `-a/--artefacts`, `-q/--quiet` | Same as upstream, including `-` for stdin/stdout and Markdown mode |
| `-t/--theme` | All 12 upstream names implemented, including `neo`, `neo-dark`, `redux`, `redux-dark`, `redux-color` and `redux-dark-color` (colours derived with khroma's HSL maths, plus `strokeWidth`, `radius`, fonts and the categorical palettes). Without `-t`, mermaid 12's per-diagram defaults apply: flowchart, sequence, class, state, ER and requirement diagrams render in `redux-color` with the `neo` look, everything else in `default` (resolved as mermaid's `resolveAppearance`: directives, then site config, then defaults; a diagram section's `theme`/`look` beats the top-level one within a layer) |
| `-c/--configFile`, `-C/--cssFile` | Layered exactly as upstream (config file assigned over `-t`, CSS becomes `themeCSS`) |
| `-b/--backgroundColor`, `-I/--svgId`, `-s/--scale`, `--size`, `--pdf-paper-format`, `-j/--jobs` | Same semantics |
| `--no-font-embed` | Accepted. This implementation never embeds fonts |
| `-p/--puppeteerConfigFile` | Validated (must exist and be JSON), otherwise ignored: there is no browser |
| `--iconPacks`, `--iconPacksNamesAndUrls` | Accepted with a warning. Icons are not rendered |
| `-w`, `-H`, `-f` (removed upstream) | Accepted with a warning, for old scripts |

### Known limitations

- **Labels are SVG `<text>`, not HTML `<foreignObject>`.** Markdown/HTML formatting inside labels is stripped to plain text
  (`<br>` line breaks are honoured). Long labels are not auto-wrapped.
- **Text is measured with Helvetica/Arial metrics**, not by a browser, so widths can be off by a few percent for other fonts.
- **PDF text uses the built-in Helvetica fonts** (nothing embedded), so characters outside Windows-1252 become `?` in PDFs.
  SVG and PNG handle any Unicode the system fonts cover.
- **PNG text needs system fonts.** macOS always has them. Minimal Linux containers may need a font package.
- **Layout:** network-simplex ranking (as in dagre/graphviz), barycentric crossing reduction and exact weighted
  least-squares coordinate assignment. Results are comparable to dagre but not identical. Network simplex recomputes the
  tree on every pivot (O(V·E)); graphs above 4,000 nodes fall back to a greedy ranker.
- `click`/`href`/callbacks are parsed and ignored (static images have no interactivity).
- **No rotated text** in the scene model, so the xychart y-axis title, gitGraph commit labels and quadrant
  y-labels are drawn horizontally.
- **Icons** (architecture, mindmap `::icon`, C4 sprites) are generic glyphs; icon packs are not loaded.
- **Themes / looks:** the colour themes' per-item palette (`data-color-id`) is resolved to concrete colours for flowchart
  subgraphs, state composites, class boxes, ER entities, requirement boxes and sequence actors/activations, in upstream's
  `colorIndex` order. Gitgraph, mindmap, journey and the other diagrams use the theme's colours but not its
  diagram-specific palette rules. `look: neo` currently changes paint only (stroke widths, corner radii, the drop
  shadow); its node padding/geometry is not applied yet. `look: handDrawn` is accepted and drawn as `classic`.
  The redux drop shadow (hard 4px offset, 6% opacity) is exact; the neo themes' blurred `drop-shadow()` is drawn
  without blur, and the `neo`/`neo-dark` gradient node borders (`useGradient`) are drawn with the solid `nodeBorder`.
  `borderColorArray`/`bkgColorArray` cannot be overridden through `themeVariables`.
- **Gantt is deterministic:** no today-marker (it would need the clock), and times are UTC.
- **Approximate layouts** where mermaid uses force-directed or browser layout: architecture (grid from side hints),
  mindmap (two-sided tree rather than force layout), and C4 relationships (straight lines that can pass behind cards).

## Design

**Functional core, imperative shell.**

```
src/
  main.rs            imperative shell: args, files, stdin/stdout, logging, exit codes, threads
  cli.rs             pure: argv → Command (commander.js semantics and messages)
  plan.rs            pure: output naming, format inference, relative paths
  markdown.rs        pure: mermaid-cli's Markdown regex, reimplemented exactly
  render.rs          pure: text + config → Scene
  diagram/           parsers (text → typed IR) and renderers (IR → Scene)
    graph.rs         shared graph IR + renderer for flowchart/state/class
  layout/            layered graph engine (rank.rs = network simplex) + boxes.rs = Taffy wrapper
  scene.rs           backend-neutral drawing model (arrowheads/arcs pre-resolved)
  output/            Scene → SVG / PDF (hand-written) / PNG (resvg)
  theme/             palettes (mod.rs: classic themes; neo.rs: neo/redux family)
  config.rs text.rs color.rs json.rs geom.rs
```

Every stage is a function from immutable input to new output. Only `main.rs` touches the filesystem, and only
`output::png::Rasterizer::new` reads system fonts.

**Safety:** `#![forbid(unsafe_code)]` in the library and binary. IDs are newtypes (`NodeId`, `ClusterId`), choices are
enums (`Format`, `ThemeName`, `Paper`, `Shape`, `Head`, `Direction`, …), and parse failures are typed `Error` values with line
numbers, never panics.

**Dependencies:** `taffy` (box layout; with only the flexbox/grid/block features it pulls in `slotmap` and
`smallvec`), plus `resvg` for PNG. Build with `--no-default-features` for a three-crate dependency tree that writes SVG,
PDF and Markdown (no PNG).

## Building (macOS Apple Silicon is the primary target)

```sh
cargo build --release                         # target/release/mmdc
cargo install --path .                        # puts `mmdc` on your PATH
cargo build --release --no-default-features   # zero dependencies, no PNG
```

CI runs on `macos-14` (Apple Silicon) and Ubuntu. Only portable crates and the standard library are used.

## Tests and speed

```sh
cargo test                                    # unit + end-to-end CLI tests + perf tests (debug budgets)
cargo test --release --test perf -- --nocapture   # speed tests with release budgets
cargo bench                                   # benchmark table (no external bench crate)
cargo run --release --example scaling         # layout scaling report
cargo run --release --example gallery         # re-render docs/gallery + docs/GALLERY.md
```

`tests/perf.rs` asserts time budgets per diagram type and checks *scaling*: a 4× larger graph must take well under 16× the
time, so accidental quadratic behaviour fails CI. This test caught a real ranking bug during development (longest-path
ranking produced 1,000-rank edges on some graphs), which led to the network-simplex ranker.

Typical release-build numbers (x86_64 Linux container; per diagram, parse + layout + SVG):

| Diagram | Time |
|---|---|
| flowchart, 100-node chain | 0.8 ms |
| flowchart, 200 nodes / 300 random edges | 4.8 ms |
| flowchart, 1000 nodes / 1500 random edges | 73 ms |
| flowchart, 20 subgraphs × 10 nodes | 1.6 ms |
| sequence, 20 actors / 500 messages | 3.6 ms |
| sequence, 50 actors / 5000 messages | 38 ms |
| state, 300 states | 5.3 ms |
| class, 150 classes | 2.0 ms |
| pie, 100 slices | 0.5 ms |

Every example in the gallery, across all 25 diagram types, renders in under 0.5 ms; the speed suite checks each one
stays under a budget, plus large inputs for gantt, ER, kanban (Taffy) and sankey.

A whole `mmdc -i flowchart.mmd -o out.svg` process takes about 2 ms. Upstream starts a Chromium instance per run.
