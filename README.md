# Mermaid Diagram Compiler (rust)

I built this after having issues with the way that mermaid works in terms of the attack surface, memory footprint, etc. I heard that they use puppeteer because it allows them to take advantage of the Blink layout engine in Chrome(ium). I figured that there were some rust crates that could do layouts, I had remembered seeing them when working on another project. So, I found taffy and I pointed my agent at the Mermaid source code and told it to use that and implement as much as possible. Issues are open, I will do my best to get to them but this is a project that I basically 1-shotted in the middle of my databases class, so I can't promise anything in terms of maintaining.

However, at least for the couple of diagrams that I've made through my academic career, it seems to be pretty good at matching the actual mermaid layout, even if it is lacking in the style department. Pull the code, `cargo build`, and run `mmdc` on your diagrams. Maybe it's good 'nough to replace the real deal. Enjoy!



This project comes with no warranty, and if you would like to use it in your project,
it is under the CC0 license in the public domain. Attribution is appreciated,
but not required.

Inquiries: `sam@robog.net`.

![This code was written with the help of LLM coding tools](https://robog.net/badges/assisted.svg)

[I used Claude Code with Opus 5.5 to develop all of the code in this project.](https://robog.net/docs/generative-ai-usage)

Finally, some slop for your reading pleasure:

---

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

**The images are not pixel-identical.** Upstream renders with mermaid.js in a browser, and since mermaid 12.0 lays out
flowchart, state and class diagrams with ELK by default (dagre before that); this is an independent
implementation of the same diagram languages, so layouts, spacing and fonts differ in detail. Byte-identical output is not
achievable without embedding a browser, which is what this project exists to avoid.

### Diagram support

Every diagram family in mermaid 12.3.0 (the version `mmdc` 12 installs today) is implemented. See **[docs/GALLERY.md](docs/GALLERY.md)** for a rendered screenshot of
each example. Each entry has its source in a collapsible block, which GitHub renders with real mermaid, so you can
compare the two directly.

| Diagram | Layout engine | Notes |
|---|---|---|
| Flowchart (`graph`, `flowchart`) | layered graph | all shapes, link types, subgraphs, styling |
| Sequence (`sequenceDiagram`) | sequence | actors, activations, notes, all frames, boxes, autonumber |
| State (`stateDiagram[-v2]`) | layered graph | composite states, fork/join/choice, notes |
| Class (`classDiagram`) | layered graph | members, generics, relations, cardinalities, namespaces |
| ER (`erDiagram`) | layered graph | attribute tables, keys, crow's-foot cardinalities, aliases, styling, subgraphs (nested, per-group `direction`, relationships to a group) |
| Requirement (`requirementDiagram`) | layered graph | all requirement/element types and relationships |
| Architecture (`architecture-beta`) | grid from port sides, nested groups | groups (`in`, nesting, icons), services, junctions, `{group}` edges, `align row/column`. Generic icon glyphs only |
| Pie (`pie`) | geometry | `showData`, mermaid's geometry |
| Gantt (`gantt`) | time axis | date formats, `after`/`until`, excludes, milestones, markers |
| Timeline (`timeline`) | **Taffy** flex | sections, multi-event periods |
| User journey (`journey`) | **Taffy** flex | scores as faces, actors |
| Kanban (`kanban`) | **Taffy** flex | columns, cards, ticket/assignee/priority |
| Block (`block-beta`) | port of mermaid's block layout | nested `block:id:span`, `columns`, `space:N`, shapes, block arrows, edges, styles/classes |
| Packet (`packet-beta`) | **Taffy** CSS grid | bit ranges, `+N`, row splitting |
| C4 (`C4Context/Container/Component/Dynamic/Deployment`) | **Taffy** flex/wrap | unified shapes (person, cylinder, queue), Structurizr type lines, boundaries, all element/rel kinds, curved rels, style updates |
| xychart (`xychart-beta`) | port of mermaid's chartBuilder (d3 band/linear scales) | bar and line series, horizontal, numeric/category x-axis, legend, data and point labels, axis toggles and label rotation, themes |
| Quadrant (`quadrantChart`) | geometry | points with styles/classes |
| Radar (`radar-beta`) | geometry | circle/polygon graticule, legend |
| Sankey (`sankey-beta`) | sankey (d3-style) | CSV input, relaxation, link colouring |
| gitGraph (`gitGraph`) | lanes | branches, merges, cherry-picks, tags, LR/TB/BT |
| Mindmap (`mindmap`) | two-sided tree | all node shapes |
| Treemap (`treemap-beta`) | squarified | sections, values, classDef |
| ZenUML (`zenuml`) | sequence (lowered) | translated into the sequence engine |
| Agentflow (`agentflow-beta`) | layered graph (upstream default is ELK) | flow/global containers with nesting and collapse, connectors, task/tool/input/decision/refdoc/action shapes, sequence/reference/failure edges, `@{}` metadata, per-kind colours |
| Swimlane (`swimlane-beta`) | lane-aware layered (port of mermaid's swimlanes engine) + orthogonal router | lanes from subgraphs, default lane, TB/BT/LR/RL, cross-lane handoffs, edge labels, line hops, flowchart shapes and styling |
| Event Modeling (`eventmodeling`) | timeline/lane positioning ported from upstream | `tf`/`rf` frames, ui/pcr/cmd/rmo/evt lanes, namespaced lanes, `->>` sources, inline/typed data and data blocks, inferred relations |
| Use case (`usecase-beta`) | layered graph (upstream default is ELK) | actors, ellipse/rect/business use cases, system boundaries (rect/package), associations, include/extend/generalization, notes, JSON tables, styling |
| Railroad (`railroad-beta`, `railroad-ebnf-beta`, `railroad-abnf-beta`, `railroad-peg-beta`) | recursive box/rail layout (upstream renderer port) | IR constructors, EBNF, ABNF repeats/num-vals, PEG predicates; choice, optional, loops, special boxes |
| Wardley map (`wardley-beta`) | fixed canvas, coordinate projection | components/anchors, flows and labels, evolve, pipelines, build/buy/outsource/market, inertia, custom stages, notes, annotations, accelerators |
| TreeView (`treeView-beta`) | indented rows (upstream `drawTree`) | box-drawing or indented input, file/folder icons, `:::highlight`, `##` descriptions |
| Ishikawa (`ishikawa`, `ishikawa-beta`) | fishbone (upstream geometry) | effect head, alternating cause categories, nested sub-causes, word wrap |
| Venn (`venn-beta`) | area-proportional circles (venn.js port) | sets, unions with sizes/labels, text nodes, styles, theme palettes |
| Cynefin (`cynefin-beta`) | fixed five-domain framework | domain items, overflow badge, labelled transitions, seeded wavy boundaries, cliff |
| `info` | — | prints the implementation version |
| Subgraphs / composites / namespaces | layered + per-cluster sub-layout (ELK container rules) | nested clusters, per-cluster `direction`, cluster-to-cluster edges, concurrency regions, dotted namespaces, empty/styled/classed subgraphs |

Shared features: front-matter (`title`, `config`), `%%{init: …}%%` directives, `%%` comments, `accTitle`/`accDescr`
(emitted as `<title>`/`<desc>` and used for Markdown alt text and titles, as upstream does), themes and `themeVariables`.

### Where Taffy is used, and where it isn't

[Taffy](https://github.com/DioxusLabs/taffy) implements the CSS box-layout algorithms (flexbox, grid, block). Diagrams
that are nested rectangles in rows, columns or grids are built as a box tree and laid out by Taffy through a small
pure wrapper (`src/layout/boxes.rs`): packet, kanban, timeline, journey, C4 boundaries and chart legends.

Taffy cannot lay out node-link graphs. CSS has no concept of ranking nodes, minimising edge crossings or routing
edges. Flowchart, state, class, ER and requirement diagrams therefore use the purpose-built layered engine
(`src/layout/`: network-simplex ranking, barycentric ordering, Brandes–Köpf coordinates as in dagre), just as mermaid uses
dagre/ELK rather than CSS for them. Block diagrams use a direct port of mermaid's own block layout, because its
quirks (siblings taking a composite's height, `space:N` as N slots) do not map onto CSS grid.

### Flags

| Flag | Behaviour |
|---|---|
| `-i/--input`, `-o/--output`, `-e/--outputFormat`, `-a/--artefacts`, `-q/--quiet` | Same as upstream, including `-` for stdin/stdout and Markdown mode |
| `-t/--theme` | All 12 upstream names implemented, including `neo`, `neo-dark`, `redux`, `redux-dark`, `redux-color` and `redux-dark-color` (colours derived with khroma's HSL maths, plus `strokeWidth`, `radius`, fonts and the categorical palettes). Without `-t`, mermaid 12's per-diagram defaults apply: flowchart, sequence, class, state, ER, requirement, agentflow, swimlane, use case and venn diagrams render in `redux-color` with the `neo` look, everything else in `default` (resolved as mermaid's `resolveAppearance`: directives, then site config, then defaults; a diagram section's `theme`/`look` beats the top-level one within a layer) |
| `-c/--configFile`, `-C/--cssFile` | Layered exactly as upstream (config file assigned over `-t`, CSS becomes `themeCSS`) |
| `-b/--backgroundColor`, `-I/--svgId`, `-s/--scale`, `--size`, `--pdf-paper-format`, `-j/--jobs` | Same semantics |
| `--no-font-embed` | Accepted. This implementation never embeds fonts |
| `-p/--puppeteerConfigFile` | Validated (must exist and be JSON), otherwise ignored: there is no browser |
| `--iconPacks`, `--iconPacksNamesAndUrls` | Accepted with a warning. Icons are not rendered |
| `-w`, `-H`, `-f` (removed upstream) | Accepted with a warning, for old scripts |

### Known limitations

- **Labels are SVG `<text>`, not HTML `<foreignObject>`.** Markdown/HTML formatting inside labels is stripped to plain text
  (emphasis markers are removed by CommonMark's rules, but bold/italic are not drawn)
  (`<br>` line breaks are honoured). Flowchart, state, swimlane, agentflow and use-case node labels
  wrap at `wrappingWidth` and are widened to `minNodeWidth` (both 120, as in mermaid 12), so short-label nodes share one width.
  Wrapping follows the browser's line breaking (CJK between characters, after hyphens); edge labels wrap at 200px.
  Quoted and markdown strings may span lines.
- **Text is measured with Helvetica/Arial metrics**, not by a browser, so widths can be off by a few percent for other fonts.
- **PDF text uses the built-in Helvetica fonts** (nothing embedded), so characters outside Windows-1252 become `?` in PDFs.
  SVG and PNG handle any Unicode the system fonts cover.
- **PNG text needs system fonts.** macOS always has them. Minimal Linux containers may need a font package.
- **Layout engine:** mermaid 12 defaults to ELK (`layout: elk`) for graph diagrams; this implementation uses a
  dagre-style layered layout and reproduces ELK's container rules instead: a subgraph with no edge crossing its border
  is laid out on its own in its own `direction`, with ELK's padding (24) and spacing (50 between nodes, 24 between
  layers). Under ELK (the default) the root graph uses ELK's spacing (40 between nodes and layers, the diagram's
  `nodeSpacing` / `rankSpacing` and edge lengths such as `--->` are ignored, as upstream) and edges are routed
  orthogonally and drawn with radius-5 rounded corners (mermaid's `rounded` curve, whatever `curve` is set): ports
  spread over the facing node side, one slot per bend between layers, labels on the route, self-loops on the
  west / north side. Node placement is still the dagre-style one, so positions can differ from ELK's; ELK's line hops
  at crossings, its side-of-edge label placement and its edges attached to a container's frame border (ours run to a
  member node and are cut at the frame) are not reproduced. `layout: dagre` (or any other non-ELK `layout`) gives
  dagre's spacing and the configured curve. ELK's packing of disconnected parts inside a container is not
  reproduced, `@{ view: collapsed }` folds a subgraph into one node as upstream.
- **Layout:** network-simplex ranking (as in dagre/graphviz), barycentric crossing reduction and Brandes–Köpf
  coordinate assignment ported from dagre's `position/bk` (every edge gets a midpoint vertex, as with dagre's
  `makeSpaceForEdgeLabels`, which gives mermaid's curved "S" edges). Crossing reduction differs from dagre's, so
  node order can differ. Network simplex recomputes the
  tree on every pivot (O(V·E)); graphs above 4,000 nodes fall back to a greedy ranker.
- `click`/`href`/callbacks are parsed and ignored (static images have no interactivity).
- **Rotated text** follows mermaid (xychart left-axis title and `xAxis.labelRotation`, gitGraph `rotateCommitLabel`
  and TB/BT tags, quadrant y-labels, wardley, swimlane). With `rotateCommitLabel: false` the LR gitGraph widens its
  commit step so horizontal labels don't overlap (mermaid lets them overlap).
- **Icons** (architecture, mindmap `::icon`) are generic glyphs; icon packs are not loaded.
- **Themes / looks:** the colour themes' per-item palette (`data-color-id`) is resolved to concrete colours for flowchart
  subgraphs, state composites, class boxes, ER entities, requirement boxes and sequence actors/activations, in upstream's
  `colorIndex` order. Gitgraph, mindmap, journey and the other diagrams use the theme's colours but not its
  diagram-specific palette rules. `look: neo` applies the upstream shapes' neo label paddings (`node.look === 'neo'`
  branches), the `*_neo` ER/requirement markers, stroke widths, corner radii and the drop shadow. `look: handDrawn` is accepted and drawn as `classic`.
  The redux drop shadow (hard 4px offset, 6% opacity) is exact; the neo themes' blurred `drop-shadow()` is drawn
  without blur, and the `neo`/`neo-dark` gradient node borders (`useGradient`) are drawn with the solid `nodeBorder`.
  `borderColorArray`/`bkgColorArray` cannot be overridden through `themeVariables`.
- **Mermaid 12.1–12.3 diagrams:** agentflow and use case use the layered layout rather than upstream's default ELK
  (curved rather than right-angled edges). Swimlane has a simplified orthogonal router (no crossing cleanup or
  `automaticLaneOrdering`, no pools). Cynefin's wavy borders are seeded from the SVG id (stable output; set `cynefin.seed`
  for upstream's exact curves). Venn's 4+-set solver uses a fixed seed (deterministic). Wardley uses a serif font as
  mermaid's output does. Icon-pack icons (treeView, use case) draw mermaid's "?" placeholder. Eventmodeling data is not monospaced.
- **Upstream bugs not reproduced:** mermaid 12.3 throws `this.calcIntersect is not a function` for a block-diagram
  edge to or from a diamond (`id{"…"}`) node; this renders it.
- **Parse error line numbers** follow mermaid: they count lines after front-matter, directives and `%%` comment lines
  are removed, so they can be lower than the line in your file. Where jison only notices an error at the next token,
  mermaid reports that token's line; this reports the statement's own line.
- **Gantt is deterministic:** no today-marker (it would need the clock), and times are UTC.
- **Approximate layouts** where mermaid uses force-directed or browser layout: architecture (grid from side hints),
  mindmap (two-sided tree rather than force layout), and C4 (flex rows rather than mermaid's grid; relationships can pass behind cards).

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
