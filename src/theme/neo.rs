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

use super::classic::{self, K, LIMIT, Vars, git_inverses, scale_invert_peer, scale_labels};
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

/// `journeyFills` of `redux-dark-color` (Tailwind 900 shades).
const DARK_JOURNEY_FILLS: [&str; 8] = [
    "#701a75", "#134e4a", "#7c2d12", "#581c87", "#14532d", "#4c1d95", "#7f1d1d", "#713f12",
];

/// `neo-dark`'s fixed `git0..7` (lightened by 25: it sets `gitDarkMode = true`).
const NEO_DARK_GIT: [&str; 8] = [
    "#0b0000", "#4d1037", "#3f5258", "#4f2f1b", "#6e0a0a", "#3b0048", "#995a01", "#154706",
];

/// The theme object's constructor values that `updateColors()` reads for the section
/// colour sets (`cScale*`, `fillType*`, `git*`, `pie*`).
pub(super) fn section_init(theme: ThemeName, v: &mut Vars) {
    if Variant::of(theme).dark {
        let p = hexk("#1f2020");
        v.set("primaryColor", p);
        v.set("secondaryColor", p.lighten(16.0));
        v.set("tertiaryColor", p.hue(-160.0));
        v.set("primaryTextColor", p.invert());
    } else {
        v.set("primaryColor", hexk("#cccccc"));
        v.set("mainBkg", hexk("#ffffff"));
        v.set("tertiaryColor", hexk("#ffffff"));
    }
}

/// The light themes' gantt-block locals (`primaryColor`, `secondaryColor`,
/// `tertiaryColor` consts) or the dark themes' own `this.*` triple.
fn triple(v: &Vars, dark: bool) -> (K, K, K) {
    if dark {
        (
            v.c("primaryColor"),
            v.c("secondaryColor"),
            v.c("tertiaryColor"),
        )
    } else {
        let p = hexk("#ECECFE");
        (p, hexk("#E9E9F1"), p.adjust(180.0, 0.0, 5.0))
    }
}

/// `updateColors()` of one family member, restricted to the section colour sets.
pub(super) fn section_update(theme: ThemeName, v: &mut Vars) {
    let var = Variant::of(theme);
    let dm = v.dark_mode;
    let text = if dm {
        "#eee"
    } else if var.redux {
        if var.dark { "#FFFFFF" } else { "#28253D" }
    } else {
        "#333"
    };
    v.or("primaryTextColor", |_| hexk(text));
    v.or("actorTextColor", |v| v.c("primaryTextColor"));
    v.or("labelTextColor", |v| v.c("actorTextColor"));
    let (p, s, t) = triple(v, var.dark);

    // cScale0..11.
    if var.color {
        for (i, c) in COLOR_SCALE.iter().enumerate() {
            v.or(format!("cScale{i}"), |_| hexk(c));
        }
    } else {
        if var.redux && !var.dark {
            // theme-redux.js: plain assignment, every slot is `mainBkg`.
            let m = v.c("mainBkg");
            for i in 0..LIMIT {
                v.set(format!("cScale{i}"), m);
            }
        } else {
            v.or("cScale0", |_| p);
            v.or("cScale1", |_| s);
            v.or("cScale2", |_| t);
            for (i, h) in [30.0, 60.0, 90.0, 120.0, 150.0, 210.0, 270.0, 300.0, 330.0]
                .iter()
                .enumerate()
            {
                let l = if i + 3 == 8 { 150.0 } else { 0.0 };
                v.or(format!("cScale{}", i + 3), |_| p.adjust(*h, 0.0, l));
            }
        }
        for i in 0..LIMIT {
            let c = v.n("cScale", i).darken(if dm { 75.0 } else { 25.0 });
            v.set(format!("cScale{i}"), c);
        }
    }
    scale_invert_peer(v, dm);
    if var.color && var.dark {
        // theme-redux-dark-color.js: unguarded `darken(cScaleN, 75)`.
        for i in 0..LIMIT {
            let c = v.n("cScale", i).darken(75.0);
            v.set(format!("cScaleLabel{i}"), c);
        }
    } else {
        let label = v
            .get("scaleLabelColor")
            .unwrap_or_else(|| v.c("labelTextColor"));
        scale_labels(v, label);
    }

    // fillType0..7.
    for i in 0..8 {
        let c = match (var.color, var.dark) {
            (true, false) => hexk(BKG_COLORS[i]),
            (true, true) => hexk(DARK_JOURNEY_FILLS[i]),
            _ => {
                let (fp, fs, _) = triple(v, var.dark);
                let base = if i % 2 == 0 { fp } else { fs };
                base.hue([0.0, 0.0, 64.0, 64.0, -64.0, -64.0, 128.0, 128.0][i])
            }
        };
        v.or(format!("fillType{i}"), |_| c);
    }

    // git0..7.
    let (gp, gs, gt) = if var.dark && !var.redux {
        (
            hexk(NEO_DARK_GIT[0]),
            hexk(NEO_DARK_GIT[1]),
            hexk(NEO_DARK_GIT[2]),
        )
    } else {
        (p, s, t)
    };
    v.or("git0", |_| gp);
    v.or("git1", |_| gs);
    v.or("git2", |_| gt);
    for (i, h) in [-30.0, -60.0, -90.0, 60.0, 120.0].iter().enumerate() {
        let c = if var.dark && !var.redux {
            hexk(NEO_DARK_GIT[i + 3])
        } else {
            p.hue(*h)
        };
        v.or(format!("git{}", i + 3), |_| c);
    }
    let git_lighten = dm || (var.dark && !var.redux);
    for i in 0..8 {
        let g = v.n("git", i);
        let g = if git_lighten {
            g.lighten(25.0)
        } else {
            g.darken(25.0)
        };
        v.set(format!("git{i}"), g);
    }
    git_inverses(v);
    let branch = v.get("branchLabelColor").unwrap_or_else(|| {
        if dm {
            hexk("black")
        } else {
            v.c("labelTextColor")
        }
    });
    for i in 0..8 {
        v.or(format!("gitBranchLabel{i}"), |_| branch);
    }

    // pie1..12 (kept under the engine's `pieN` keys).
    if var.color {
        for i in 0..LIMIT {
            v.or(format!("pie{}", i + 1), |v| v.n("cScale", i));
        }
    } else {
        let list = [
            p,
            s,
            t,
            p.adjust(0.0, 0.0, -10.0),
            s.adjust(0.0, 0.0, -10.0),
            t.adjust(0.0, 0.0, -10.0),
            p.adjust(60.0, 0.0, -10.0),
            p.adjust(-60.0, 0.0, -10.0),
            p.adjust(120.0, 0.0, 0.0),
            p.adjust(60.0, 0.0, -20.0),
            p.adjust(-60.0, 0.0, -20.0),
            p.adjust(120.0, 0.0, -10.0),
        ];
        for (i, c) in list.into_iter().enumerate() {
            v.or(format!("pie{}", i + 1), |_| c);
        }
    }
}

fn hexk(s: &str) -> K {
    classic::hex(s)
}

/// Build the palette for one of the six family members. `base` supplies the fields this
/// family does not set (inherited from the default palette as mermaid inherits nothing
/// diagram-visible there).
pub(super) fn palette(theme: ThemeName, base: Palette) -> Palette {
    let v = Variant::of(theme);
    let mut p = base;

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
        p.quadrant_fill = c("#ECECFE");
        p.primary_text = c(primary_text);
        p.primary_border = hex(primary_border);
        p.node_text = c(primary_text);
        p.line_color = hex(line);
        p.text_color = c(primary_text);
        p.title_color = hex(tertiary.invert());
        if v.redux {
            p.cluster_bkg = c("#F9F9FB");
            p.cluster_border = c("#BDBCCC");
            p.composite_title_background = Some(c("#F9F9FB"));
            p.alt_background = Some(c("#F9F9FB"));
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
        // cScale*, pie*, ...: `section_update`.
        p.pie_section_text = c(primary_text);
        p.color_slots = v.color.then(|| CategoricalPalette {
            border: BORDER_COLORS.iter().map(|s| c(s)).collect(),
            bkg: BKG_COLORS.iter().map(|s| c(s)).collect(),
        });
        p.er_edge_label_background = v.color.then(|| c("#FFFFFF"));
        p.requirement_edge_label_background = v.redux.then(|| c("#FFFFFF"));
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
        p.quadrant_fill = hex(primary);
        p.primary_text = hex(primary_text);
        p.primary_border = hex(primary_border);
        p.node_text = hex(primary_text);
        p.line_color = hex(line);
        p.text_color = text.clone();
        p.title_color = hex(tertiary.invert());
        if v.redux {
            p.cluster_bkg = c("#1E1A2E");
            p.cluster_border = c("#BDBCCC");
            p.composite_title_background = Some(c("#16141F"));
            p.composite_background = Some(c("#16141F"));
            p.alt_background = Some(c("#16141F"));
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
        // cScale*, pie*, ...: `section_update`.
        p.pie_section_text = if v.color { c("#28253D") } else { text.clone() };
        p.color_slots = v.color.then(|| CategoricalPalette {
            border: BORDER_COLORS.iter().map(|s| c(s)).collect(),
            bkg: Vec::new(),
        });
        p.er_edge_label_background = v.color.then(|| c("#16141F"));
        p.requirement_edge_label_background = v.redux.then(|| c("#16141F"));
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

    /// Section colour sets checked against mermaid 12.3.0's resolved `themeVariables`
    /// (rendered per theme in the reference renderer).
    #[test]
    fn section_colors_match_upstream() {
        let keys = [
            "cScale1",
            "cScale8",
            "cScalePeer1",
            "cScaleInv2",
            "cScaleLabel0",
            "fillType2",
            "fillType7",
            "git0",
            "git3",
            "gitInv1",
            "gitBranchLabel0",
        ];
        let expect: [(ThemeName, [&str; 11]); 6] = [
            (
                ThemeName::Neo,
                [
                    "#9b9bbf", "#bff986", "#7c7cac", "#060679", "#333333", "#feecfd", "#f1eae9",
                    "#7373f8", "#73b5f8", "#646440", "#333333",
                ],
            ),
            (
                ThemeName::NeoDark,
                [
                    "#080909", "#c0bfbe", "#000000", "#ffffff", "#e0dfdf", "#1f1f20", "#494749",
                    "#8b0000", "#ae683b", "#48d97d", "#e0dfdf",
                ],
            ),
            (
                ThemeName::Redux,
                [
                    "#bfbfbf", "#bfbfbf", "#a6a6a6", "#404040", "#28253d", "#feecfd", "#f1eae9",
                    "#7373f8", "#73b5f8", "#646440", "#28253d",
                ],
            ),
            (
                ThemeName::ReduxDark,
                [
                    "#080909", "#c0bfbe", "#000000", "#ffffff", "#e0dfdf", "#1f1f20", "#494749",
                    "#000000", "#000000", "#f7f6f6", "#e0dfdf",
                ],
            ),
            (
                ThemeName::ReduxColor,
                [
                    "#46ecd5", "#a3b3ff", "#18e7cb", "#004795", "#28253d", "#fff7ed", "#fefce8",
                    "#7373f8", "#73b5f8", "#646440", "#28253d",
                ],
            ),
            (
                ThemeName::ReduxDarkColor,
                [
                    "#46ecd5", "#a3b3ff", "#18e7cb", "#004795", "#230028", "#7c2d12", "#713f12",
                    "#000000", "#000000", "#f7f6f6", "#e0dfdf",
                ],
            ),
        ];
        for (theme, want) in expect {
            let p = build(theme, &[]);
            for (key, want) in keys.iter().zip(want) {
                let (prefix, i) = key.split_at(key.find(|c: char| c.is_ascii_digit()).unwrap());
                let i: usize = i.parse().unwrap();
                let set = match prefix {
                    "cScale" => &p.scale,
                    "cScalePeer" => &p.scale_peer,
                    "cScaleInv" => &p.scale_inv,
                    "cScaleLabel" => &p.scale_text,
                    "fillType" => &p.fill_type,
                    "git" => &p.git,
                    "gitInv" => &p.git_inv,
                    "gitBranchLabel" => &p.git_branch_label,
                    _ => unreachable!(),
                };
                assert_eq!(css(&set[i]), want, "{theme:?} {key}");
            }
        }
        // pieN: the neo light themes use their gantt-block locals, the colour themes
        // reuse cScale.
        assert_eq!(css(&build(ThemeName::Neo, &[]).pie_colors[2]), "#ffffff");
        assert_eq!(
            css(&build(ThemeName::ReduxDarkColor, &[]).pie_colors[1]),
            "#46ecd5"
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
