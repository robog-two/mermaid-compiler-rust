//! Backend-neutral drawing model produced by every diagram renderer.
//!
//! Renderers are pure functions `Diagram -> Scene`; the SVG, PDF and PNG backends
//! only serialise a `Scene`. Arrowheads, arcs and curves are already resolved to
//! primitive paths, so all backends draw exactly the same geometry.

use crate::geom::{PathCmd, Point, Rect};

/// A CSS colour string as written by the user or theme (`#fff`, `hsl(...)`, `red`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Color(pub String);

impl Color {
    pub fn new(s: impl Into<String>) -> Self {
        Color(s.into())
    }
    pub fn css(&self) -> &str {
        &self.0
    }
    pub fn none() -> Self {
        Color("none".into())
    }
    pub fn is_none(&self) -> bool {
        self.0 == "none" || self.0 == "transparent"
    }
    /// The colour through CSS `filter: brightness(k)`: each channel scaled by
    /// `k`, rounded and clamped, alpha kept. Unparsable colours (and `none`)
    /// are returned as they are.
    pub fn brightened(&self, k: f64) -> Color {
        if k == 1.0 || self.is_none() {
            return self.clone();
        }
        match crate::color::parse(self.css()) {
            Some(x) => {
                let ch = |v: u8| (v as f64 * k).round().clamp(0.0, 255.0) as u8;
                let c = crate::color::Rgba {
                    r: ch(x.r),
                    g: ch(x.g),
                    b: ch(x.b),
                    a: x.a,
                };
                if x.a < 1.0 {
                    Color(format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, x.a))
                } else {
                    Color(c.to_hex())
                }
            }
            None => self.clone(),
        }
    }
}

/// Fill and stroke for a shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    pub fill: Color,
    pub stroke: Color,
    pub stroke_width: f64,
    pub dash: Option<Vec<f64>>,
    pub opacity: Option<f64>,
    /// `fill-opacity`: the fill alone made translucent (the stroke stays opaque).
    pub fill_opacity: Option<f64>,
    /// A blurred drop shadow behind the shape (CSS `filter: drop-shadow()`).
    pub shadow: Option<Shadow>,
    /// A horizontal linear gradient the stroke is painted with instead of `stroke`.
    pub stroke_gradient: Option<Gradient>,
    /// Composited with `mix-blend-mode: multiply`. SVG output gets this from the
    /// scene's CSS; PDF output, which has no CSS, sets the blend mode itself.
    pub multiply: bool,
}

/// A two-stop horizontal `<linearGradient>`: `from` at `x1` (and left of it),
/// `to` at `x2` (and right of it). With `bbox`, `x1` and `x2` are fractions of
/// the painted shape's bounding box (`gradientUnits="objectBoundingBox"`),
/// otherwise user-space coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub x1: f64,
    pub x2: f64,
    pub from: Color,
    pub to: Color,
    pub bbox: bool,
}

impl Gradient {
    /// `from` at the left edge of each shape's bounding box to `to` at its right
    /// edge (mermaid's `{id}-gradient`, `x1="0%" x2="100%"`).
    pub fn across_bbox(from: &Color, to: &Color) -> Self {
        Gradient {
            x1: 0.0,
            x2: 1.0,
            from: from.clone(),
            to: to.clone(),
            bbox: true,
        }
    }

    /// The gradient in user space for a shape of geometry `geom`; `None` where
    /// a bounding-box gradient has no box to span (a shape without width or
    /// height, which SVG then leaves unpainted).
    pub fn in_user_space(&self, geom: &Geometry) -> Option<Gradient> {
        if !self.bbox {
            return Some(self.clone());
        }
        let b = geom.bbox()?;
        (b.w > 0.0 && b.h > 0.0).then(|| Gradient {
            x1: b.x + self.x1 * b.w,
            x2: b.x + self.x2 * b.w,
            bbox: false,
            ..self.clone()
        })
    }
}

/// A CSS `drop-shadow(dx dy blur color)`: the shape's silhouette offset and
/// blurred with a Gaussian whose standard deviation is the `blur` length.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    pub dx: f64,
    pub dy: f64,
    /// The standard deviation of the blur.
    pub std_dev: f64,
    pub color: Color,
    /// An SVG `<filter>` of its own (`feDropShadow`) rather than a CSS
    /// filter function: it works in `linearRGB`, the shape it draws
    /// (`SourceGraphic`) quantised through that space as well.
    pub linear_rgb: bool,
    /// Written, in SVG output, as the CSS `filter: drop-shadow()` mermaid's
    /// stylesheet gives it, which a browser draws as it draws mermaid's
    /// (rasterisers get the equivalent `<filter>`).
    pub css: bool,
}

impl Shadow {
    /// The CSS `drop-shadow(dx dy blur color)` function.
    pub fn css_function(&self) -> String {
        use crate::output::svg::num;
        format!(
            "drop-shadow({}px {}px {}px {})",
            num(self.dx),
            num(self.dy),
            num(self.std_dev),
            self.color.css()
        )
    }
}

impl Paint {
    pub fn fill(c: &Color) -> Self {
        Paint {
            fill: c.clone(),
            stroke: Color::none(),
            stroke_width: 0.0,
            dash: None,
            opacity: None,
            fill_opacity: None,
            shadow: None,
            stroke_gradient: None,
            multiply: false,
        }
    }
    pub fn stroke(c: &Color, w: f64) -> Self {
        Paint {
            fill: Color::none(),
            stroke: c.clone(),
            stroke_width: w,
            dash: None,
            opacity: None,
            fill_opacity: None,
            shadow: None,
            stroke_gradient: None,
            multiply: false,
        }
    }
    pub fn both(fill: &Color, stroke: &Color, w: f64) -> Self {
        Paint {
            fill: fill.clone(),
            stroke: stroke.clone(),
            stroke_width: w,
            dash: None,
            opacity: None,
            fill_opacity: None,
            shadow: None,
            stroke_gradient: None,
            multiply: false,
        }
    }
    pub fn dashed(mut self, d: Vec<f64>) -> Self {
        self.dash = Some(d);
        self
    }
    pub fn with_opacity(mut self, o: f64) -> Self {
        self.opacity = Some(o);
        self
    }
    pub fn with_multiply(mut self) -> Self {
        self.multiply = true;
        self
    }
    pub fn with_fill_opacity(mut self, o: f64) -> Self {
        self.fill_opacity = Some(o);
        self
    }
    pub fn with_shadow(mut self, s: Option<Shadow>) -> Self {
        self.shadow = s;
        self
    }
    /// Stroke with a horizontal gradient (`stroke` stays as the solid fallback).
    pub fn with_stroke_gradient(mut self, g: Gradient) -> Self {
        self.stroke_gradient = Some(g);
        self
    }
}

/// Geometry of a shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Rect {
        rect: Rect,
        rx: f64,
    },
    Circle {
        c: Point,
        r: f64,
    },
    Path(Vec<PathCmd>),
    /// A straight `<line>` (mermaid's gantt today marker), for SVG output: a
    /// browser keeps a line's coordinates as given (a path's too, but a line
    /// far outside the canvas is measured from clamped ones). Every other
    /// consumer treats it as the two-point path [`Geometry::line_path`].
    Line {
        a: Point,
        b: Point,
    },
}

impl Geometry {
    /// A [`Geometry::Line`]'s two-point path.
    pub fn line_path(a: Point, b: Point) -> Vec<PathCmd> {
        vec![PathCmd::MoveTo(a), PathCmd::LineTo(b)]
    }
    /// The fill bounding box (SVG `getBBox()`: curves by their extremes, strokes
    /// left out); `None` for an empty path.
    pub fn bbox(&self) -> Option<Rect> {
        match self {
            Geometry::Rect { rect, .. } => Some(*rect),
            Geometry::Circle { c, r } => Some(Rect::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r)),
            Geometry::Path(cmds) => crate::geom::path_bbox(cmds),
            Geometry::Line { a, b } => crate::geom::path_bbox(&Geometry::line_path(*a, *b)),
        }
    }

    /// The same geometry moved by `(dx, dy)`.
    pub fn translated(&self, dx: f64, dy: f64) -> Geometry {
        let mv = |p: &Point| Point::new(p.x + dx, p.y + dy);
        match self {
            Geometry::Line { a, b } => Geometry::Line { a: mv(a), b: mv(b) },
            Geometry::Rect { rect, rx } => Geometry::Rect {
                rect: Rect::new(rect.x + dx, rect.y + dy, rect.w, rect.h),
                rx: *rx,
            },
            Geometry::Circle { c, r } => Geometry::Circle { c: mv(c), r: *r },
            Geometry::Path(cmds) => Geometry::Path(
                cmds.iter()
                    .map(|c| match c {
                        PathCmd::MoveTo(p) => PathCmd::MoveTo(mv(p)),
                        PathCmd::LineTo(p) => PathCmd::LineTo(mv(p)),
                        PathCmd::CubicTo(a, b, p) => PathCmd::CubicTo(mv(a), mv(b), mv(p)),
                        PathCmd::Close => PathCmd::Close,
                        PathCmd::MoveBy(d) => PathCmd::MoveBy(*d),
                        &PathCmd::ArcTo {
                            rx,
                            ry,
                            large,
                            sweep,
                            to,
                            rel,
                        } => PathCmd::ArcTo {
                            rx,
                            ry,
                            large,
                            sweep,
                            to: if rel { to } else { mv(&to) },
                            rel,
                        },
                    })
                    .collect(),
            ),
        }
    }
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// A block of text. `y` is the baseline of the first line, so every backend
/// places glyphs identically without needing `dominant-baseline` support.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub x: f64,
    pub y: f64,
    pub lines: Vec<String>,
    pub line_height: f64,
    pub font_size: f64,
    pub bold: bool,
    /// The CSS `font-weight` mermaid's markup computes for this text when that
    /// is not plain normal (400) or bold (700): ishikawa's head label (600), or
    /// a bold HTML label (700), whose `<strong>` runs are then `bolder` (900).
    /// SVG output writes it; layout keeps measuring with `bold`.
    pub weight: Option<u16>,
    pub anchor: Anchor,
    pub fill: Color,
    /// mermaid writes this one-line text as sequence's `byTspan` does: at its
    /// `dominant-baseline: central` (`y` less [`crate::text::baseline_shift`])
    /// with the words in a `<tspan dy="0">`. `y` is still the alphabetic
    /// baseline that its `getBBox()` implies; Chromium paints it elsewhere
    /// (see [`crate::text::painted_tspan_central`]), so SVG output writes
    /// mermaid's markup and a raster drawn from it the painted baseline.
    pub tspan_central: bool,
    /// mermaid writes this text as timeline's `wrap` does: `<text dy="1em"
    /// dominant-baseline="middle">` at `y` with one `<tspan x dy>` per line,
    /// the first `1em` and each further one `line_height` (in em) below the
    /// last. SVG output writes that markup, so a browser lays it out as it
    /// lays out mermaid's; the other backends draw each line at the baseline
    /// [`Text::line_y`] gives, Chromium's for it.
    pub dy_tspans: bool,
    /// An inline `font-family` (which wins over the stylesheet's), as
    /// mermaid's unknown icon writes its `?` in `ArialMT, Arial`, or a
    /// stylesheet's family for the text's class (usecase actors' `"Open
    /// Sans", sans-serif`). The text is measured in its face.
    pub family: Option<std::borrow::Cow<'static, str>>,
}

/// The first line's baseline below the top of a block of `line_height`
/// lines. An HTML label's line (`line-height: 1.5`, or `normal`) puts its
/// glyph box, the font's ascent and descent rounded to whole pixels, half
/// the leading below the line's top, rounded down to a pixel (measured in
/// Chromium for 8-32px). Other line heights keep the 0.35em approximation.
pub fn first_baseline(font_size: f64, line_height: f64) -> f64 {
    let html = (line_height - 1.5 * font_size).abs() < 1e-9
        || (line_height - crate::text::normal_line_height(font_size)).abs() < 1e-9;
    if html {
        let ascent = crate::text::ascent_px(font_size);
        let descent = crate::text::descent_px(font_size);
        ((line_height - ascent - descent) / 2.0 + 1e-9).floor() + ascent
    } else {
        line_height / 2.0 + font_size * 0.35
    }
}

impl Text {
    /// Lay out `lines` vertically centred on `center`.
    pub fn centered(
        center: Point,
        lines: &[String],
        font_size: f64,
        line_height: f64,
        fill: &Color,
    ) -> Text {
        let total = line_height * lines.len() as f64;
        Text {
            x: center.x,
            y: center.y - total / 2.0 + first_baseline(font_size, line_height),
            lines: lines.to_vec(),
            line_height,
            font_size,
            bold: false,
            weight: None,
            anchor: Anchor::Middle,
            fill: fill.clone(),
            tspan_central: false,
            dy_tspans: false,
            family: None,
        }
    }
    /// One line of SVG text centred on `center` with `dominant-baseline:
    /// middle` (its baseline half the x-height below the centre).
    pub fn middle(center: Point, line: &str, font_size: f64, fill: &Color) -> Text {
        Text {
            x: center.x,
            y: center.y + crate::text::baseline_shift(crate::text::Baseline::Middle, font_size),
            lines: vec![line.to_string()],
            line_height: font_size * 1.5,
            font_size,
            bold: false,
            weight: None,
            anchor: Anchor::Middle,
            fill: fill.clone(),
            tspan_central: false,
            dy_tspans: false,
            family: None,
        }
    }
    /// The alphabetic baseline of line `i`. For [`Text::dy_tspans`] text, as
    /// Chromium computes it: the `dy` lengths resolved and accumulated in
    /// single precision (`1em`, then `line_height / font_size` em per line)
    /// and the `middle` baseline shift added.
    pub fn line_y(&self, i: usize) -> f64 {
        if !self.dy_tspans {
            return self.y + i as f64 * self.line_height;
        }
        let em = self.font_size as f32;
        let step = (self.line_height / self.font_size) as f32 * em;
        let mut acc = em;
        for _ in 0..i {
            acc += step;
        }
        f64::from(self.y as f32 + acc)
            + crate::text::baseline_shift(crate::text::Baseline::Middle, self.font_size)
    }
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub fn anchored(mut self, anchor: Anchor, x: f64) -> Self {
        self.anchor = anchor;
        self.x = x;
        self
    }
}

/// Element tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Group {
        id: Option<String>,
        class: Option<String>,
        children: Vec<Element>,
    },
    Shape {
        geom: Geometry,
        paint: Paint,
        class: Option<String>,
    },
    Text {
        text: Text,
        class: Option<String>,
    },
    /// Text drawn with an outline of `width` in `color` under its fill (CSS
    /// `stroke` with `stroke-linejoin: round; paint-order: stroke`). SVG output
    /// writes plain text and leaves the outline to the scene's CSS, as mermaid
    /// does; PDF output, which has no CSS, strokes it itself.
    OutlinedText {
        text: Text,
        class: Option<String>,
        color: Color,
        width: f64,
    },
    /// Children drawn rotated by `deg` degrees (clockwise, SVG `rotate(deg cx cy)`)
    /// about `about` — e.g. vertical axis titles or labels running along a line.
    Rotated {
        deg: f64,
        about: Point,
        children: Vec<Element>,
    },
    /// Children cut off outside `rect`, as a `<foreignObject>` clips the HTML
    /// label that overflows it (mid-glyph). SVG output writes a nested `<svg>`
    /// viewport over the same user coordinates.
    Clipped {
        rect: Rect,
        children: Vec<Element>,
    },
    /// Children given in a local frame whose origin is `(dx, dy)` (SVG
    /// `translate(dx, dy)`): mermaid draws some shapes about their own origin
    /// and moves them with a transform, and the browser measures and paints
    /// such a shape in single precision in that frame, not where it lands.
    Translated {
        dx: f64,
        dy: f64,
        children: Vec<Element>,
    },
    /// A raster image (`<image href preserveAspectRatio="none">`) stretched
    /// over `rect` (imageSquare.ts).
    Image {
        rect: Rect,
        href: String,
        /// The image's bytes as a `data:` URI when `href` is remote and the
        /// caller fetched it: PNG and PDF output draw this, SVG output keeps
        /// the link, as mmdc's browser does.
        inline: Option<String>,
    },
    /// An Iconify icon as mermaid inserts it (`<g><svg …>…</svg></g>`, see
    /// [`crate::iconify`]): `svg` is the `<svg>` element, whose viewport
    /// (its `width` × `height`) lands on `rect`; `color` is the `color` its
    /// `currentColor` takes (the wrapper's `style="color: …"`).
    Icon {
        rect: Rect,
        color: Option<Color>,
        svg: String,
    },
    /// Children composited as one layer at `opacity` (SVG `<g opacity>`): where
    /// they overlap they do not darken each other, as the silhouette a filter
    /// offsets for a whole group's drop shadow does not.
    Faded {
        opacity: f64,
        class: Option<String>,
        children: Vec<Element>,
    },
    /// Children composited as one layer and drawn through a drop shadow
    /// filter: the layer's silhouette offset (and blurred) under it, as a
    /// filter on a whole group casts it (mindmap's neo nodes). PDF output,
    /// which has no filters, casts each shape's own shadow instead
    /// ([`Element::with_shape_shadows`]).
    Shadowed {
        shadow: Shadow,
        children: Vec<Element>,
    },
    /// Children composited as one layer and drawn through CSS `filter:
    /// brightness(factor)`: every colour channel of the layer scaled by
    /// `factor` and clamped, antialiased edges included (timeline's
    /// `.eventWrapper`). SVG output writes a `<g class>` and a stylesheet
    /// rule for that class, as mermaid does; PDF output, which has no
    /// filters, draws the children with their colours scaled instead
    /// ([`Element::brightened`]).
    Brightened {
        factor: f64,
        class: String,
        children: Vec<Element>,
    },
}

impl Element {
    pub fn group(class: impl Into<String>, children: Vec<Element>) -> Self {
        Element::Group {
            id: None,
            class: Some(class.into()),
            children,
        }
    }
    pub fn shape(geom: Geometry, paint: Paint) -> Self {
        Element::Shape {
            geom,
            paint,
            class: None,
        }
    }
    pub fn classed_shape(geom: Geometry, paint: Paint, class: impl Into<String>) -> Self {
        Element::Shape {
            geom,
            paint,
            class: Some(class.into()),
        }
    }
    pub fn text(text: Text) -> Self {
        Element::Text { text, class: None }
    }
    pub fn classed_text(text: Text, class: impl Into<String>) -> Self {
        Element::Text {
            text,
            class: Some(class.into()),
        }
    }
    pub fn rect(rect: Rect, rx: f64, paint: Paint) -> Self {
        Element::shape(Geometry::Rect { rect, rx }, paint)
    }
    pub fn path(cmds: Vec<PathCmd>, paint: Paint) -> Self {
        Element::shape(Geometry::Path(cmds), paint)
    }
    pub fn line(a: Point, b: Point, paint: Paint) -> Self {
        Element::path(vec![PathCmd::MoveTo(a), PathCmd::LineTo(b)], paint)
    }
    /// This element with every colour it paints passed through CSS
    /// `brightness(k)` ([`Color::brightened`]): what the filter makes of
    /// flat colours, though not of the antialiased edges where they meet.
    /// Images are left as they are.
    /// This element with `shadow` cast by each of its shapes that casts
    /// none of its own.
    pub fn with_shape_shadows(&self, shadow: &Shadow) -> Element {
        let mut e = self.clone();
        fn walk(e: &mut Element, sh: &Shadow) {
            match e {
                Element::Shape { paint, .. } => {
                    if paint.shadow.is_none() {
                        paint.shadow = Some(sh.clone());
                    }
                }
                Element::Group { children, .. }
                | Element::Rotated { children, .. }
                | Element::Clipped { children, .. }
                | Element::Translated { children, .. }
                | Element::Faded { children, .. }
                | Element::Shadowed { children, .. }
                | Element::Brightened { children, .. } => {
                    children.iter_mut().for_each(|c| walk(c, sh))
                }
                _ => {}
            }
        }
        walk(&mut e, shadow);
        e
    }

    pub fn brightened(&self, k: f64) -> Element {
        let all = |children: &[Element]| children.iter().map(|c| c.brightened(k)).collect();
        let paint = |p: &Paint| Paint {
            fill: p.fill.brightened(k),
            stroke: p.stroke.brightened(k),
            shadow: p.shadow.as_ref().map(|s| Shadow {
                color: s.color.brightened(k),
                ..s.clone()
            }),
            stroke_gradient: p.stroke_gradient.as_ref().map(|g| Gradient {
                from: g.from.brightened(k),
                to: g.to.brightened(k),
                ..g.clone()
            }),
            ..p.clone()
        };
        let text = |t: &Text| Text {
            fill: t.fill.brightened(k),
            ..t.clone()
        };
        match self {
            Element::Group {
                id,
                class,
                children,
            } => Element::Group {
                id: id.clone(),
                class: class.clone(),
                children: all(children),
            },
            Element::Shape {
                geom,
                paint: p,
                class,
            } => Element::Shape {
                geom: geom.clone(),
                paint: paint(p),
                class: class.clone(),
            },
            Element::Text { text: t, class } => Element::Text {
                text: text(t),
                class: class.clone(),
            },
            Element::OutlinedText {
                text: t,
                class,
                color,
                width,
            } => Element::OutlinedText {
                text: text(t),
                class: class.clone(),
                color: color.brightened(k),
                width: *width,
            },
            Element::Rotated {
                deg,
                about,
                children,
            } => Element::Rotated {
                deg: *deg,
                about: *about,
                children: all(children),
            },
            Element::Clipped { rect, children } => Element::Clipped {
                rect: *rect,
                children: all(children),
            },
            Element::Translated { dx, dy, children } => Element::Translated {
                dx: *dx,
                dy: *dy,
                children: all(children),
            },
            Element::Image { .. } => self.clone(),
            Element::Icon { rect, color, svg } => Element::Icon {
                rect: *rect,
                color: color.as_ref().map(|c| c.brightened(k)),
                svg: svg.clone(),
            },
            Element::Faded {
                opacity,
                class,
                children,
            } => Element::Faded {
                opacity: *opacity,
                class: class.clone(),
                children: all(children),
            },
            Element::Brightened {
                factor,
                class,
                children,
            } => Element::Brightened {
                factor: *factor,
                class: class.clone(),
                children: all(children),
            },
            Element::Shadowed { shadow, children } => Element::Shadowed {
                shadow: Shadow {
                    color: shadow.color.brightened(k),
                    ..shadow.clone()
                },
                children: all(children),
            },
        }
    }

    /// This element (and its children) moved by `(dx, dy)`.
    pub fn translated(self, dx: f64, dy: f64) -> Self {
        let all =
            |children: Vec<Element>| children.into_iter().map(|c| c.translated(dx, dy)).collect();
        match self {
            Element::Group {
                id,
                class,
                children,
            } => Element::Group {
                id,
                class,
                children: all(children),
            },
            Element::Clipped { rect, children } => Element::Clipped {
                rect: Rect::new(rect.x + dx, rect.y + dy, rect.w, rect.h),
                children: all(children),
            },
            Element::Faded {
                opacity,
                class,
                children,
            } => Element::Faded {
                opacity,
                class,
                children: all(children),
            },
            Element::Brightened {
                factor,
                class,
                children,
            } => Element::Brightened {
                factor,
                class,
                children: all(children),
            },
            Element::Shadowed { shadow, children } => Element::Shadowed {
                shadow,
                children: all(children),
            },
            Element::Image { rect, href, inline } => Element::Image {
                rect: Rect::new(rect.x + dx, rect.y + dy, rect.w, rect.h),
                href,
                inline,
            },
            Element::Icon { rect, color, svg } => Element::Icon {
                rect: Rect::new(rect.x + dx, rect.y + dy, rect.w, rect.h),
                color,
                svg,
            },
            Element::Shape { geom, paint, class } => Element::Shape {
                geom: geom.translated(dx, dy),
                paint,
                class,
            },
            Element::Text { mut text, class } => {
                text.x += dx;
                text.y += dy;
                Element::Text { text, class }
            }
            Element::OutlinedText {
                mut text,
                class,
                color,
                width,
            } => {
                text.x += dx;
                text.y += dy;
                Element::OutlinedText {
                    text,
                    class,
                    color,
                    width,
                }
            }
            Element::Rotated {
                deg,
                about,
                children,
            } => Element::Rotated {
                deg,
                about: Point::new(about.x + dx, about.y + dy),
                children: all(children),
            },
            Element::Translated {
                dx: x,
                dy: y,
                children,
            } => Element::Translated {
                dx: x + dx,
                dy: y + dy,
                children,
            },
        }
    }
}

/// Which diagram produced a scene (drives `aria-roledescription` and CSS class).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramKind {
    Flowchart,
    Sequence,
    Pie,
    State,
    Class,
    Er,
    Gantt,
    Journey,
    GitGraph,
    Mindmap,
    Timeline,
    Quadrant,
    Requirement,
    C4,
    Sankey,
    XyChart,
    Block,
    Packet,
    Architecture,
    Kanban,
    Radar,
    Treemap,
    ZenUml,
    Info,
    Agentflow,
    Swimlane,
    EventModeling,
    TreeView,
    Ishikawa,
    Railroad,
    RailroadEbnf,
    RailroadAbnf,
    RailroadPeg,
    Venn,
    Wardley,
    Cynefin,
    Usecase,
}

impl DiagramKind {
    /// mermaid's `aria-roledescription` value.
    pub fn role(self) -> &'static str {
        match self {
            DiagramKind::Flowchart => "flowchart-v2",
            DiagramKind::Sequence => "sequence",
            DiagramKind::Pie => "pie",
            DiagramKind::State => "stateDiagram",
            DiagramKind::Class => "classDiagram",
            DiagramKind::Er => "er",
            DiagramKind::Gantt => "gantt",
            DiagramKind::Journey => "journey",
            DiagramKind::GitGraph => "gitGraph",
            DiagramKind::Mindmap => "mindmap",
            DiagramKind::Timeline => "timeline",
            DiagramKind::Quadrant => "quadrantChart",
            DiagramKind::Requirement => "requirement",
            DiagramKind::C4 => "c4",
            DiagramKind::Sankey => "sankey",
            DiagramKind::XyChart => "xychart",
            DiagramKind::Block => "block",
            DiagramKind::Packet => "packet",
            DiagramKind::Architecture => "architecture",
            DiagramKind::Kanban => "kanban",
            DiagramKind::Radar => "radar",
            DiagramKind::Treemap => "treemap",
            DiagramKind::ZenUml => "zenuml",
            DiagramKind::Info => "info",
            DiagramKind::Agentflow => "agentflow",
            DiagramKind::Swimlane => "swimlane",
            DiagramKind::EventModeling => "eventmodeling",
            DiagramKind::TreeView => "treeView",
            DiagramKind::Ishikawa => "ishikawa",
            DiagramKind::Railroad => "railroad",
            DiagramKind::RailroadEbnf => "railroadEbnf",
            DiagramKind::RailroadAbnf => "railroadAbnf",
            DiagramKind::RailroadPeg => "railroadPeg",
            DiagramKind::Venn => "venn",
            DiagramKind::Wardley => "wardley",
            DiagramKind::Cynefin => "cynefin",
            DiagramKind::Usecase => "usecase",
        }
    }
    /// CSS class mermaid puts on the root `<svg>`.
    pub fn svg_class(self) -> &'static str {
        match self {
            DiagramKind::Flowchart => "flowchart",
            DiagramKind::Sequence => "sequence",
            DiagramKind::Pie => "pie",
            DiagramKind::State => "statediagram",
            DiagramKind::Class => "classDiagram",
            DiagramKind::Er => "erDiagram",
            DiagramKind::Gantt => "gantt",
            DiagramKind::Journey => "journey",
            DiagramKind::GitGraph => "gitGraph",
            DiagramKind::Mindmap => "mindmap",
            DiagramKind::Timeline => "timeline",
            DiagramKind::Quadrant => "quadrantChart",
            DiagramKind::Requirement => "requirementDiagram",
            DiagramKind::C4 => "c4",
            DiagramKind::Sankey => "sankey",
            DiagramKind::XyChart => "xychart",
            DiagramKind::Block => "block",
            DiagramKind::Packet => "packet",
            DiagramKind::Architecture => "architecture",
            DiagramKind::Kanban => "kanban",
            DiagramKind::Radar => "radar",
            DiagramKind::Treemap => "treemap",
            DiagramKind::ZenUml => "zenuml",
            DiagramKind::Info => "info",
            DiagramKind::Agentflow => "agentflow",
            // Drawn by the flowchart renderer.
            DiagramKind::Swimlane => "flowchart",
            DiagramKind::EventModeling => "eventmodeling",
            DiagramKind::TreeView => "treeView",
            DiagramKind::Ishikawa => "ishikawa",
            DiagramKind::Railroad
            | DiagramKind::RailroadEbnf
            | DiagramKind::RailroadAbnf
            | DiagramKind::RailroadPeg => "railroad-diagram",
            DiagramKind::Venn => "venn",
            DiagramKind::Wardley => "wardley",
            DiagramKind::Cynefin => "cynefin",
            DiagramKind::Usecase => "usecase",
        }
    }
}

/// A complete, positioned drawing.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub kind: DiagramKind,
    pub view_box: Rect,
    pub elements: Vec<Element>,
    /// Accessible title (`accTitle`) → `<title>`.
    pub acc_title: Option<String>,
    /// Accessible description (`accDescr`) → `<desc>`.
    pub acc_descr: Option<String>,
    /// Font family used by text in this scene.
    pub font_family: String,
    /// Extra CSS appended to the generated `<style>` (theme CSS, classDefs).
    pub css: String,
}

impl Scene {
    /// Count of all elements (recursively) – handy for tests and benchmarks.
    pub fn element_count(&self) -> usize {
        fn count(e: &[Element]) -> usize {
            e.iter()
                .map(|el| match el {
                    Element::Group { children, .. }
                    | Element::Rotated { children, .. }
                    | Element::Brightened { children, .. }
                    | Element::Shadowed { children, .. }
                    | Element::Translated { children, .. } => 1 + count(children),
                    _ => 1,
                })
                .sum()
        }
        count(&self.elements)
    }
}
