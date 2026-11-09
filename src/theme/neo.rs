//! The neo / redux theme family (`theme-{neo,neo-dark,redux,redux-dark,redux-color,
//! redux-dark-color}.js`), derived with the same khroma operations upstream uses.
//!
//! Every value below is the result of upstream's constructor + `updateColors()` with no
//! user overrides. Two quirks are reproduced on purpose:
//!
//! * none of these themes sets `darkMode`, so `mkBorder` always takes its light branch
//!   (`adjust(c, { s: -40, l: -10 })`) and the dark variants darken their colour scale
//!   by 25 points (which takes the near-black `primaryColor` hues to black);
//! * the dark variants' `edgeLabelBackground` is the plain `secondaryColor` for the
//!   same reason.

use super::{CategoricalPalette, DropShadow, Palette, ThemeName};
use crate::color::{self, Rgba};
use crate::scene::Color;

/// Which member of the family is being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Variant {
    dark: bool,
    /// `redux*` (as opposed to `neo*`).
    redux: bool,
    /// `redux-color` / `redux-dark-color`: carries the categorical palette.
    color: bool,
}

impl Variant {
    fn of(theme: ThemeName) -> Variant {
        let (dark, redux, color) = match theme {
            ThemeName::Neo => (false, false, false),
            ThemeName::NeoDark => (true, false, false),
            ThemeName::Redux => (false, true, false),
            ThemeName::ReduxDark => (true, true, false),
            ThemeName::ReduxColor => (false, true, true),
            ThemeName::ReduxDarkColor => (true, true, true),
            _ => (false, false, false),
        };
        Variant { dark, redux, color }
    }
}

fn rgb(s: &str) -> Rgba {
    color::parse(s).unwrap_or(Rgba::BLACK)
}

fn hex(c: Rgba) -> Color {
    Color::new(c.to_hex())
}

fn c(s: &str) -> Color {
    Color::new(s)
}

/// `theme-helpers.js` `mkBorder(col, darkMode)` with `darkMode` unset.
pub(crate) fn mk_border(col: Rgba) -> Rgba {
    col.adjust_hsl(0.0, -40.0, -10.0)
}

/// `borderColorArray` of the colour themes (Tailwind 400 shades).
pub(crate) const BORDER_COLORS: [&str; 12] = [
    "#E879F9", "#2DD4BF", "#FB923C", "#22D3EE", "#4ADE80", "#A78BFA", "#F87171", "#FACC15",
    "#818CF8", "#A3E635", "#38BDF8", "#FB7185",
];

/// `bkgColorArray` of `redux-color` (Tailwind 50 shades). `redux-dark-color` ships `[]`.
pub(crate) const BKG_COLORS: [&str; 12] = [
    "#FDF4FF", "#F0FDFA", "#FFF7ED", "#ECFEFF", "#F0FDF4", "#F5F3FF", "#FEF2F2", "#FEFCE8",
    "#EEF2FF", "#F7FEE7", "#F0F9FF", "#FFF1F2",
];

/// `cScale0..11` of the colour themes (Tailwind 300 shades).
const COLOR_SCALE: [&str; 12] = [
    "#f4a8ff", "#46ecd5", "#ffb86a", "#dab2ff", "#7bf1a8", "#c4b4ff", "#ffa2a2", "#ffdf20",
    "#a3b3ff", "#bbf451", "#74d4ff", "#ffa1ad",
];

/// mermaid's twelve-entry pie/cScale recipe from a primary/secondary/tertiary triple.
fn pie_from(p: Rgba, s: Rgba, t: Rgba) -> Vec<Color> {
    [
        p,
        s,
        t,
        p.adjust_hsl(0.0, 0.0, -10.0),
        s.adjust_hsl(0.0, 0.0, -10.0),
        t.adjust_hsl(0.0, 0.0, -10.0),
        p.adjust_hsl(60.0, 0.0, -10.0),
        p.adjust_hsl(-60.0, 0.0, -10.0),
        p.adjust_hsl(120.0, 0.0, 0.0),
        p.adjust_hsl(60.0, 0.0, -20.0),
        p.adjust_hsl(-60.0, 0.0, -20.0),
        p.adjust_hsl(120.0, 0.0, -10.0),
    ]
    .into_iter()
    .map(hex)
    .collect()
}

/// `cScale0..11` from a primary/secondary/tertiary triple (before any darkening).
fn scale_from(p: Rgba, s: Rgba, t: Rgba) -> Vec<Rgba> {
    let mut v = vec![p, s, t];
    for (h, l) in [
        (30.0, 0.0),
        (60.0, 0.0),
        (90.0, 0.0),
        (120.0, 0.0),
        (150.0, 0.0),
        (210.0, 150.0),
        (270.0, 0.0),
        (300.0, 0.0),
        (330.0, 0.0),
    ] {
        v.push(p.adjust_hsl(h, 0.0, l));
    }
    v
}

/// Build the palette for one of the six family members. `base` supplies the fields this
/// family does not set (inherited from the default palette as mermaid inherits nothing
/// diagram-visible there).
pub(super) fn palette(theme: ThemeName, base: Palette) -> Palette {
    let v = Variant::of(theme);
    let mut p = base;

    // Gantt-block locals of the light themes, also used by their pie and neo cScale.
    let g_primary = rgb("#ECECFE");
    let g_secondary = rgb("#E9E9F1");
    let g_tertiary = g_primary.adjust_hsl(180.0, 0.0, 5.0);

    p.font_family = if v.redux {
        "\"Recursive Variable\", arial, sans-serif".into()
    } else {
        "arial, sans-serif".into()
    };
    p.font_size = 14.0;
    p.radius = Some(if v.redux { 12.0 } else { 3.0 });
    p.stroke_width = Some(if v.dark && !v.redux { 1.0 } else { 2.0 });
    p.er_row_odd = None; // rowOdd/rowEven are not defined by this family.

    if !v.dark {
        let background = rgb("#ffffff");
        let primary = rgb("#cccccc");
        let main_bkg = rgb("#ffffff");
        let tertiary = rgb("#ffffff");
        let secondary = primary.adjust_hsl(-120.0, 0.0, 0.0);
        let primary_text = if v.redux { "#28253D" } else { "#333" };
        let primary_border = if v.redux {
            mk_border(rgb("#28253D"))
        } else {
            mk_border(primary)
        };
        let line = background.invert();
        let node_border = if v.redux { "#28253D" } else { "#000000" };
        let actor_border = if v.redux {
            c("#28253D")
        } else {
            hex(primary_border)
        };

        p.background = hex(background);
        p.main_bkg = hex(main_bkg);
        p.node_border = c(node_border);
        p.node_text = c(primary_text);
        p.line_color = hex(line);
        p.text_color = c(primary_text);
        p.title_color = hex(tertiary.invert());
        if v.redux {
            p.cluster_bkg = c("#F9F9FB");
            p.cluster_border = c("#BDBCCC");
        } else {
            p.cluster_bkg = hex(tertiary);
            p.cluster_border = hex(mk_border(tertiary));
        }
        p.edge_label_background = hex(secondary);
        p.actor_bkg = hex(main_bkg);
        p.actor_border = actor_border.clone();
        p.actor_text = c(primary_text);
        p.actor_line = actor_border.clone();
        p.signal_color = c(primary_text);
        p.signal_text = c(primary_text);
        p.label_box_bkg = hex(main_bkg);
        p.label_box_border = actor_border;
        p.label_text = c(primary_text);
        p.loop_text = c(primary_text);
        p.note_bkg = c("#fff5ad");
        p.note_text = c(primary_text);
        p.note_border = if v.redux {
            c("#FACC15")
        } else {
            hex(mk_border(rgb("#fff5ad")))
        };
        p.activation_bkg = hex(secondary);
        p.activation_border = hex(secondary.darken(10.0));
        p.sequence_number = hex(line.invert());
        p.special_state = hex(line);
        p.scale = if v.color {
            COLOR_SCALE.iter().map(|s| c(s)).collect()
        } else if v.redux {
            vec![hex(main_bkg.darken(25.0)); 12]
        } else {
            scale_from(g_primary, g_secondary, g_tertiary)
                .into_iter()
                .map(|x| hex(x.darken(25.0)))
                .collect()
        };
        p.pie_colors = if v.color {
            p.scale.clone()
        } else {
            pie_from(g_primary, g_secondary, g_tertiary)
        };
        p.pie_section_text = c(primary_text);
        p.color_slots = v.color.then(|| CategoricalPalette {
            border: BORDER_COLORS.iter().map(|s| c(s)).collect(),
            bkg: BKG_COLORS.iter().map(|s| c(s)).collect(),
        });
        p.er_edge_label_background = v.color.then(|| c("#FFFFFF"));
    } else {
        let background = rgb("#333");
        let primary = rgb("#1f2020");
        let secondary = primary.lighten(16.0);
        let tertiary = primary.adjust_hsl(-160.0, 0.0, 0.0);
        let primary_border = background.invert();
        let primary_text = primary.invert();
        let main_bkg = if v.redux {
            rgb("#111113")
        } else {
            rgb("#2a2020")
        };
        let text = c("#ccc");
        let line = background.invert();
        let node_border = if v.redux { c("#FFFFFF") } else { c("#ccc") };
        let actor_border = if v.redux {
            c("#FFFFFF")
        } else {
            hex(primary_border)
        };

        p.background = hex(background);
        p.main_bkg = hex(main_bkg);
        p.node_border = node_border;
        p.node_text = hex(primary_text);
        p.line_color = hex(line);
        p.text_color = text.clone();
        p.title_color = hex(tertiary.invert());
        if v.redux {
            p.cluster_bkg = c("#1E1A2E");
            p.cluster_border = c("#BDBCCC");
        } else {
            p.cluster_bkg = hex(tertiary);
            p.cluster_border = hex(mk_border(tertiary));
        }
        p.edge_label_background = hex(secondary);
        p.actor_bkg = hex(main_bkg);
        p.actor_border = actor_border.clone();
        p.actor_text = hex(primary_text);
        p.actor_line = actor_border.clone();
        p.signal_color = if v.redux { c("#FFFFFF") } else { text.clone() };
        p.signal_text = text.clone();
        p.label_box_bkg = hex(main_bkg);
        p.label_box_border = if v.redux { c("#BDBCCC") } else { actor_border };
        p.label_text = hex(primary_text);
        p.loop_text = hex(primary_text);
        if v.redux {
            p.note_bkg = c("#FEF9C3");
            p.note_text = c("#28253D");
            p.note_border = c("#FACC15");
        } else {
            p.note_bkg = c("#fff5ad");
            p.note_text = c("#333");
            p.note_border = hex(mk_border(rgb("#fff5ad")));
        }
        p.activation_bkg = hex(secondary);
        p.activation_border = hex(secondary.darken(10.0));
        p.sequence_number = hex(line.invert());
        p.special_state = hex(line);
        p.scale = if v.color {
            COLOR_SCALE.iter().map(|s| c(s)).collect()
        } else {
            scale_from(primary, secondary, tertiary)
                .into_iter()
                .map(|x| hex(x.darken(25.0)))
                .collect()
        };
        p.pie_colors = if v.color {
            p.scale.clone()
        } else {
            pie_from(primary, secondary, tertiary)
        };
        p.pie_section_text = if v.color { c("#28253D") } else { text.clone() };
        p.color_slots = v.color.then(|| CategoricalPalette {
            border: BORDER_COLORS.iter().map(|s| c(s)).collect(),
            bkg: Vec::new(),
        });
        p.er_edge_label_background = v.color.then(|| c("#16141F"));
    }
    p.pie_stroke = c("black");
    p.pie_outer_stroke = c("black");
    p.pie_stroke_width = 2.0;
    p.pie_outer_stroke_width = 2.0;
    p.pie_opacity = 0.7;
    p.pie_title_text = p.text_color.clone();
    p.pie_legend_text = p.text_color.clone();

    // `insertLookDefs` (redux: `url(#drop-shadow)`, flood tinted by "dark" in the name)
    // and the neo themes' CSS `drop-shadow()` values.
    p.drop_shadow = Some(match (v.redux, v.dark) {
        (true, dark) => DropShadow {
            dx: 4.0,
            dy: 4.0,
            color: c(if dark { "#FFFFFF" } else { "#000000" }),
            opacity: 0.06,
        },
        (false, false) => DropShadow {
            dx: 0.0,
            dy: 1.0,
            color: c("#000000"),
            opacity: 0.25,
        },
        (false, true) => DropShadow {
            dx: 1.0,
            dy: 2.0,
            color: c("#b9b9b9"),
            opacity: 0.2,
        },
    });
    p
}

#[cfg(test)]
mod tests {
    use super::super::palette as build;
    use super::*;

    use crate::theme::ColorSlot;

    fn css(c: &Color) -> String {
        c.css().to_ascii_lowercase()
    }

    #[test]
    fn mk_border_matches_khroma() {
        // #ccc = hsl(0, 0%, 80%) -> s clamps at 0, l 70% -> 178.5 -> #b3b3b3.
        assert_eq!(mk_border(rgb("#cccccc")).to_hex(), "#b3b3b3");
        // #fff5ad = hsl(52.68, 100%, 83.92%) -> hsl(52.68, 60%, 73.92%):
        // q = .7392 + .6 - .7392*.6 = .8957, p = 2*.7392 - q = .5827,
        // r = q -> 228, g = p + (q-p)*6*.1463 -> 219, b = p -> 149.
        assert_eq!(mk_border(rgb("#fff5ad")).to_hex(), "#e4db95");
    }

    #[test]
    fn neo() {
        let p = build(ThemeName::Neo, &[]);
        assert_eq!(p.font_family, "arial, sans-serif");
        assert_eq!(p.font_size, 14.0);
        assert_eq!((p.stroke_width, p.radius), (Some(2.0), Some(3.0)));
        assert_eq!(css(&p.node_border), "#000000");
        assert_eq!(css(&p.node_text), "#333");
        assert_eq!(css(&p.line_color), "#000000"); // invert(#ffffff)
        assert_eq!(css(&p.actor_border), "#b3b3b3"); // mkBorder(#cccccc)
        assert_eq!(css(&p.cluster_border), "#e6e6e6"); // mkBorder(#fff): l 90% -> 229.5
        assert_eq!(css(&p.note_border), "#e4db95");
        assert_eq!(css(&p.edge_label_background), "#cccccc"); // adjust(#ccc, h -120)
        assert_eq!(css(&p.activation_border), "#b3b3b3"); // darken(#ccc, 10)
        // darken(#ECECFE, 25): hsl(240, 90%, 96.08%) -> l 71.08% -> rgb(115, 115, 248).
        assert_eq!(css(&p.scale[0]), "#7373f8");
        assert_eq!(css(&p.pie_colors[0]), "#ececfe");
        assert!(p.color_slots.is_none() && p.er_row_odd.is_none());
    }

    #[test]
    fn neo_dark() {
        let p = build(ThemeName::NeoDark, &[]);
        assert!(p.dark);
        assert_eq!(p.stroke_width, Some(1.0));
        assert_eq!(css(&p.main_bkg), "#2a2020");
        assert_eq!(css(&p.node_border), "#ccc"); // border1: nodeBorder is commented out
        assert_eq!(css(&p.line_color), "#cccccc"); // invert(#333)
        // lighten(#1f2020, 16) = hsl(180, 1.587%, 28.353%), theme-dark's clusterBkg.
        assert_eq!(css(&p.edge_label_background), "#474949");
        assert_eq!(css(&p.actor_text), "#e0dfdf"); // invert(#1f2020)
        assert_eq!(css(&p.sequence_number), "#333333"); // invert(lineColor)
        // darkMode is never set, so the scale is darkened by 25 points: black.
        assert_eq!(css(&p.scale[0]), "#000000");
    }

    #[test]
    fn redux() {
        let p = build(ThemeName::Redux, &[]);
        assert_eq!(p.font_family, "\"Recursive Variable\", arial, sans-serif");
        assert_eq!((p.stroke_width, p.radius), (Some(2.0), Some(12.0)));
        assert_eq!(css(&p.node_border), "#28253d");
        assert_eq!(css(&p.cluster_bkg), "#f9f9fb");
        assert_eq!(css(&p.note_border), "#facc15");
        // cScale = darken(mainBkg #fff, 25) = 191.25 -> #bfbfbf for every entry.
        assert!(p.scale.iter().all(|c| css(c) == "#bfbfbf"));
        assert!(p.color_slots.is_none());

        let d = build(ThemeName::ReduxDark, &[]);
        assert_eq!(css(&d.main_bkg), "#111113");
        assert_eq!(css(&d.actor_border), "#ffffff");
        assert_eq!(css(&d.label_box_border), "#bdbccc");
        assert_eq!(css(&d.note_bkg), "#fef9c3");
        assert!(d.color_slots.is_none());
    }

    #[test]
    fn redux_color_palettes() {
        let p = build(ThemeName::ReduxColor, &[]);
        let slots = p.color_slots.as_ref().unwrap();
        let s0 = slots.slot(ColorSlot(0));
        assert_eq!(
            (css(&s0.border), s0.fill.as_ref().map(css)),
            ("#e879f9".into(), Some("#fdf4ff".into()))
        );
        // colorIndex wraps at the palette length.
        assert_eq!(slots.slot(ColorSlot(13)), slots.slot(ColorSlot(1)));
        assert_eq!(css(&p.scale[0]), "#f4a8ff");
        assert_eq!(p.pie_colors, p.scale); // pieN = cScale(N-1)
        assert_eq!(
            p.er_edge_label_background.as_ref().map(css),
            Some("#ffffff".into())
        );

        let d = build(ThemeName::ReduxDarkColor, &[]);
        let ds = d.color_slots.as_ref().unwrap().slot(ColorSlot(2));
        assert_eq!((css(&ds.border), ds.fill), ("#fb923c".into(), None));
        assert_eq!(css(&d.pie_section_text), "#28253d");
        assert_eq!(
            d.er_edge_label_background.as_ref().map(css),
            Some("#16141f".into())
        );
    }

    #[test]
    fn overrides_still_apply() {
        let p = build(
            ThemeName::ReduxColor,
            &[
                ("strokeWidth".into(), "3".into()),
                ("mainBkg".into(), "#eee".into()),
            ],
        );
        assert_eq!(p.stroke_width, Some(3.0));
        assert_eq!(p.main_bkg.css(), "#eee");
    }
}
