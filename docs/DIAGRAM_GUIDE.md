# Writing a diagram module

Each diagram family lives in `src/diagram/<name>.rs` and exposes one pure function:

```rust
pub fn render(body: &str, title: Option<String>, cfg: &MermaidConfig, pal: &Palette) -> Result<Scene>
```

It is wired up in `src/diagram/mod.rs` (detection) and `src/render.rs` (dispatch) already.

## Rules

- **Pure.** No I/O, no globals, no randomness, no time. Same input, same output.
- **No `unsafe`, no panics on user input.** Bad syntax returns `Error::parse(line, message)` with the
  1-based line number (the body keeps blank lines where front-matter/comments were removed, so line
  numbers match the user's file). `unwrap()` only where an invariant guarantees it.
- **Typed IR.** Parse into structs/enums first, then lay out, then draw. Keep the three stages separate
  functions so they can be unit-tested.
- **Draw with `scene` primitives only** (`Element::rect/path/line/text/group`, `Text::centered`, `Paint`).
  Arrowheads, arcs and curves must be geometry (`geom::polygon`, `geom::arc_cubics`,
  `geom::basis_curve`, `geom::ellipse`) — no SVG markers or filters — so the PDF writer can draw them.
- **Text** is measured with `text::TextBlock::label(raw, font_size)` / `text::line_width`. Text `y` is a
  baseline; `Text::centered` handles vertical centring.
- **Colours** come from `Palette` (`pal.main_bkg`, `pal.node_border`, `pal.line_color`, `pal.text_color`,
  `pal.scale_color(i)` / `pal.scale_text_color(i)` for per-section colours, `pal.dark`, ...). Do not edit
  `theme.rs`; derive what you need locally.
- **Config**: `cfg.section_f64("gantt", "barHeight", 20.0)`, `cfg.section_bool(...)`, `cfg.section(...)`.
- **View box**: accumulate a `geom::Bounds` of everything drawn and pad it (8–20px). Set
  `Scene { kind: DiagramKind::<Yours>, acc_title: None, acc_descr: None, font_family: pal.font_family.clone(), css: String::new(), .. }`.

## Layout engines

- **Box layout (Taffy, CSS flexbox/grid):** `crate::layout::boxes`. Build a `BoxNode` tree
  (`BoxNode::leaf(w, h)`, `BoxNode::new(style, children)`, helpers `row`, `column`, `grid`,
  `uniform_grid`, `wrap_row`, `padded`, `padded_with_title`, `stretched`, `grid_span`), call
  `boxes::compute(&root)` and read absolute `Placed { rect, children }`. Use this for anything that is
  nested rectangles, columns, rows, grids or wrapping lists.
- **Graph layout (layered/Sugiyama, network simplex):** for node-link diagrams either build a
  `diagram::graph::GraphDiagram` and call `diagram::graph::render`, or call `crate::layout::layout`
  directly with a `LayoutInput` and draw the result yourself.
- Charts (axes, polar, treemap, sankey) are plain geometry: compute coordinates directly.

## Done means

1. Parser covers the syntax on the mermaid docs page for the diagram, with unit tests including error cases.
2. At least one example in `examples/diagrams/<name>.mmd` using most features.
3. `cargo test` and `cargo clippy --all-targets -- -D warnings` pass.
4. You rendered the example to PNG (`cargo run -q -- -i examples/diagrams/<name>.mmd -o /tmp/<name>.png -s 2`)
   and **looked at it**; it is legible, nothing overlaps or is clipped, and it resembles mermaid's output.
