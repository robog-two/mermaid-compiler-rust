//! Box layout (CSS flexbox / grid / block) powered by [Taffy].
//!
//! Taffy implements the CSS box-layout algorithms; it is the right tool for
//! diagrams made of nested rectangles (block, kanban, C4 boundaries, legends,
//! timelines). It does **not** do graph layout (ranking, crossing reduction,
//! edge routing); node-link diagrams use the layered engine in [`crate::layout`].
//!
//! This module is a pure wrapper: build an immutable [`BoxNode`] tree, call
//! [`compute`], get absolute rectangles back. Taffy's mutable tree never leaks.
//!
//! [Taffy]: https://github.com/DioxusLabs/taffy

use crate::geom::Rect;
pub use taffy;
use taffy::prelude::*;

/// A box with a Taffy style and children. Leaves normally carry a fixed size.
#[derive(Debug, Clone, Default)]
pub struct BoxNode {
    pub style: Style,
    pub children: Vec<BoxNode>,
}

impl BoxNode {
    /// Container with children.
    pub fn new(style: Style, children: Vec<BoxNode>) -> Self {
        BoxNode { style, children }
    }

    /// Fixed-size leaf (e.g. a measured text label or shape).
    pub fn leaf(width: f64, height: f64) -> Self {
        BoxNode {
            style: fixed(width, height),
            children: Vec::new(),
        }
    }
}

/// A laid-out box in absolute coordinates, mirroring the input tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub rect: Rect,
    pub children: Vec<Placed>,
}

/// Lay out `root` at the origin with unconstrained (max-content) width.
pub fn compute(root: &BoxNode) -> Placed {
    compute_within(root, None)
}

/// Lay out `root`, optionally constraining the available width.
pub fn compute_within(root: &BoxNode, width: Option<f64>) -> Placed {
    let mut tree: TaffyTree<()> = TaffyTree::new();
    let id = build(&mut tree, root);
    let available = Size {
        width: width.map_or(AvailableSpace::MaxContent, |w| {
            AvailableSpace::Definite(w as f32)
        }),
        height: AvailableSpace::MaxContent,
    };
    // Building from a well-formed tree cannot fail; fall back to a zero layout if it did.
    if tree.compute_layout(id, available).is_err() {
        return zero(root);
    }
    read(&tree, id, root, 0.0, 0.0)
}

fn build(tree: &mut TaffyTree<()>, node: &BoxNode) -> NodeId {
    let children: Vec<NodeId> = node.children.iter().map(|c| build(tree, c)).collect();
    tree.new_with_children(node.style.clone(), &children)
        .unwrap_or_else(|_| {
            tree.new_leaf(Style::default())
                .expect("allocating a taffy leaf")
        })
}

fn read(tree: &TaffyTree<()>, id: NodeId, node: &BoxNode, ox: f64, oy: f64) -> Placed {
    let (x, y, w, h) = match tree.layout(id) {
        Ok(l) => (
            ox + l.location.x as f64,
            oy + l.location.y as f64,
            l.size.width as f64,
            l.size.height as f64,
        ),
        Err(_) => (ox, oy, 0.0, 0.0),
    };
    let kids = tree.children(id).unwrap_or_default();
    let children = kids
        .iter()
        .zip(&node.children)
        .map(|(&k, c)| read(tree, k, c, x, y))
        .collect();
    Placed {
        rect: Rect::new(x, y, w, h),
        children,
    }
}

fn zero(node: &BoxNode) -> Placed {
    Placed {
        rect: Rect::default(),
        children: node.children.iter().map(zero).collect(),
    }
}

// ---------------------------------------------------------------------------
// Style helpers (small, composable, pure)
// ---------------------------------------------------------------------------

/// Fixed width × height.
pub fn fixed(width: f64, height: f64) -> Style {
    Style {
        size: Size {
            width: length(width as f32),
            height: length(height as f32),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// Horizontal flex container with `gap` between children, items top-aligned.
pub fn row(gap: f64) -> Style {
    Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FLEX_START,
        gap: Size {
            width: length(gap as f32),
            height: length(gap as f32),
        },
        ..Default::default()
    }
}

/// Vertical flex container with `gap` between children, items left-aligned.
pub fn column(gap: f64) -> Style {
    Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::FLEX_START,
        gap: Size {
            width: length(gap as f32),
            height: length(gap as f32),
        },
        ..Default::default()
    }
}

/// Wrapping row (like CSS `flex-wrap: wrap`) constrained to `max_width`.
pub fn wrap_row(gap: f64, max_width: f64) -> Style {
    Style {
        flex_wrap: FlexWrap::Wrap,
        max_size: Size {
            width: length(max_width as f32),
            height: auto(),
        },
        ..row(gap)
    }
}

/// CSS grid with `columns` auto-sized tracks and `gap` spacing.
pub fn grid(columns: usize, gap: f64) -> Style {
    Style {
        display: Display::Grid,
        grid_template_columns: (0..columns.max(1)).map(|_| auto()).collect(),
        gap: Size {
            width: length(gap as f32),
            height: length(gap as f32),
        },
        ..Default::default()
    }
}

/// CSS grid with `columns` equal-width tracks of `track_width` each.
pub fn uniform_grid(columns: usize, track_width: f64, gap: f64) -> Style {
    Style {
        grid_template_columns: (0..columns.max(1))
            .map(|_| length(track_width as f32))
            .collect(),
        ..grid(columns, gap)
    }
}

/// Make children stretch across the cross axis (equal-height rows / full-width columns).
pub fn stretched(mut style: Style) -> Style {
    style.align_items = AlignItems::STRETCH;
    style
}

/// Uniform padding.
pub fn padded(mut style: Style, pad: f64) -> Style {
    let p = length(pad as f32);
    style.padding = taffy::geometry::Rect {
        left: p,
        right: p,
        top: p,
        bottom: p,
    };
    style
}

/// Padding with a larger top (room for a title band).
pub fn padded_with_title(mut style: Style, pad: f64, title: f64) -> Style {
    let p = length(pad as f32);
    style.padding = taffy::geometry::Rect {
        left: p,
        right: p,
        top: length((pad + title) as f32),
        bottom: p,
    };
    style
}

/// Minimum size.
pub fn min_size(mut style: Style, width: f64, height: f64) -> Style {
    style.min_size = Size {
        width: length(width as f32),
        height: length(height as f32),
    };
    style
}

/// Place a grid item at `column` (1-based) spanning `span` tracks.
pub fn grid_span(mut style: Style, column: i16, span: u16) -> Style {
    style.grid_column = Line {
        start: line(column),
        end: taffy::style_helpers::span(span),
    };
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_places_children_side_by_side() {
        let root = BoxNode::new(
            padded(row(10.0), 5.0),
            vec![BoxNode::leaf(40.0, 20.0), BoxNode::leaf(30.0, 50.0)],
        );
        let p = compute(&root);
        assert_eq!(p.rect, Rect::new(0.0, 0.0, 90.0, 60.0));
        assert_eq!(p.children[0].rect, Rect::new(5.0, 5.0, 40.0, 20.0));
        assert_eq!(p.children[1].rect, Rect::new(55.0, 5.0, 30.0, 50.0));
    }

    #[test]
    fn nested_coordinates_are_absolute() {
        let inner = BoxNode::new(
            padded(column(4.0), 3.0),
            vec![BoxNode::leaf(10.0, 10.0), BoxNode::leaf(10.0, 10.0)],
        );
        let root = BoxNode::new(padded(row(0.0), 7.0), vec![inner]);
        let p = compute(&root);
        assert_eq!(
            p.children[0].children[1].rect,
            Rect::new(10.0, 24.0, 10.0, 10.0)
        );
    }

    #[test]
    fn grid_aligns_columns() {
        let root = BoxNode::new(
            grid(2, 10.0),
            vec![
                BoxNode::leaf(50.0, 10.0),
                BoxNode::leaf(20.0, 10.0),
                BoxNode::leaf(30.0, 10.0),
                BoxNode::leaf(70.0, 10.0),
            ],
        );
        let p = compute(&root);
        // Column widths are max of members: 50 and 70.
        assert_eq!(p.children[1].rect.x, 60.0);
        assert_eq!(p.children[3].rect.x, 60.0);
        assert_eq!(p.children[2].rect.y, 20.0);
        assert_eq!(p.rect.w, 130.0);
    }

    #[test]
    fn wrapping_row_wraps() {
        let root = BoxNode::new(
            wrap_row(10.0, 100.0),
            (0..3).map(|_| BoxNode::leaf(40.0, 10.0)).collect(),
        );
        let p = compute(&root);
        assert_eq!(
            p.children[2].rect.y, 20.0,
            "third item wraps to the next line"
        );
    }
}
