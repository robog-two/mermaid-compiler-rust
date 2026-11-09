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

| Diagram | Status |
|---|---|
| Flowchart (`graph`, `flowchart`) | All directions, every classic node shape, `A@{ shape: … }` (common shapes), all link types (`-->`, `---`, `-.->`, `==>`, `--o`, `--x`, `<-->`, `~~~`, longer links), both label syntaxes, `&` chains, subgraphs (nested, edges to subgraphs), `classDef`/`class`/`:::`/`style`/`linkStyle`, front-matter titles |
| Sequence (`sequenceDiagram`) | participants/actors, aliases, all arrow types incl. bidirectional, activations (`+`/`-`, `activate`), notes (left/right/over), `loop`/`alt`/`else`/`opt`/`par`/`and`/`critical`/`option`/`break`/`rect`, `box`, `autonumber`, `title` |
| State (`stateDiagram`, `-v2`) | `[*]`, transitions with labels, composite states, `<<fork>>`/`<<join>>`/`<<choice>>`, descriptions, notes, `direction`, `classDef` |
| Class (`classDiagram`) | classes, members, generics, annotations, all relation types and cardinalities, namespaces, notes, `cssClass`/`style` |
| Pie (`pie`) | `showData`, `title`, mermaid's geometry and legend |
| erDiagram, gantt, gitGraph, mindmap, timeline, journey, C4, sankey, xychart, block, packet, architecture, kanban, radar, treemap, quadrant, requirement, zenuml | **Not implemented.** These are detected and fail with a clear message and exit code 1, never with silent garbage. |

Shared features: front-matter (`title`, `config`), `%%{init: …}%%` directives, `%%` comments, `accTitle`/`accDescr`
(emitted as `<title>`/`<desc>` and used for Markdown alt text and titles, as upstream does), themes and `themeVariables`.

### Flags

| Flag | Behaviour |
|---|---|
| `-i/--input`, `-o/--output`, `-e/--outputFormat`, `-a/--artefacts`, `-q/--quiet` | Same as upstream, including `-` for stdin/stdout and Markdown mode |
| `-t/--theme` | All 12 upstream names accepted. `default`, `forest`, `dark`, `neutral` and `base` are implemented. `neo*`/`redux*` map to `default`/`dark` palettes |
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
  layout/            layered layout engine; rank.rs = network simplex
  scene.rs           backend-neutral drawing model (arrowheads/arcs pre-resolved)
  output/            Scene → SVG / PDF (hand-written) / PNG (resvg)
  config.rs theme.rs text.rs color.rs json.rs geom.rs
```

Every stage is a function from immutable input to new output. Only `main.rs` touches the filesystem, and only
`output::png::Rasterizer::new` reads system fonts.

**Safety:** `#![forbid(unsafe_code)]` in the library and binary. IDs are newtypes (`NodeId`, `ClusterId`), choices are
enums (`Format`, `ThemeName`, `Paper`, `Shape`, `Head`, `Direction`, …), and parse failures are typed `Error` values with line
numbers, never panics.

**Dependencies:** none by default except `resvg` for PNG. Build with `--no-default-features` for a **zero-dependency**
binary that writes SVG, PDF and Markdown (no PNG).

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

A whole `mmdc -i flowchart.mmd -o out.svg` process takes about 2 ms. Upstream starts a Chromium instance per run.
