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
}

/// Fill and stroke for a shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    pub fill: Color,
    pub stroke: Color,
    pub stroke_width: f64,
    pub dash: Option<Vec<f64>>,
    pub opacity: Option<f64>,
}

impl Paint {
    pub fn fill(c: &Color) -> Self {
        Paint {
            fill: c.clone(),
            stroke: Color::none(),
            stroke_width: 0.0,
            dash: None,
            opacity: None,
        }
    }
    pub fn stroke(c: &Color, w: f64) -> Self {
        Paint {
            fill: Color::none(),
            stroke: c.clone(),
            stroke_width: w,
            dash: None,
            opacity: None,
        }
    }
    pub fn both(fill: &Color, stroke: &Color, w: f64) -> Self {
        Paint {
            fill: fill.clone(),
            stroke: stroke.clone(),
            stroke_width: w,
            dash: None,
            opacity: None,
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
}

/// Geometry of a shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Rect { rect: Rect, rx: f64 },
    Circle { c: Point, r: f64 },
    Path(Vec<PathCmd>),
}

impl Geometry {
    /// The same geometry moved by `(dx, dy)`.
    pub fn translated(&self, dx: f64, dy: f64) -> Geometry {
        let mv = |p: &Point| Point::new(p.x + dx, p.y + dy);
        match self {
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
    pub anchor: Anchor,
    pub fill: Color,
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
            y: center.y - total / 2.0 + line_height / 2.0 + font_size * 0.35,
            lines: lines.to_vec(),
            line_height,
            font_size,
            bold: false,
            anchor: Anchor::Middle,
            fill: fill.clone(),
        }
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
    /// Children drawn rotated by `deg` degrees (clockwise, SVG `rotate(deg cx cy)`)
    /// about `about` — e.g. vertical axis titles or labels running along a line.
    Rotated {
        deg: f64,
        about: Point,
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
            DiagramKind::Swimlane => "swimlane",
            DiagramKind::EventModeling => "eventmodeling",
            DiagramKind::TreeView => "treeView",
            DiagramKind::Ishikawa => "ishikawa",
            DiagramKind::Railroad => "railroad",
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
                    Element::Group { children, .. } | Element::Rotated { children, .. } => {
                        1 + count(children)
                    }
                    _ => 1,
                })
                .sum()
        }
        count(&self.elements)
    }
}
