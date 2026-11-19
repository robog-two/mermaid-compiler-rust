//! mermaid's own DOM for a gantt chart, as `ganttRenderer.ts` leaves it after
//! the stylesheet (and `DOMPurify` and the serialiser have had their say).

use super::{DAY_MS, Gantt, Task, auto_interval, civil_from_days, format_time, is_excluded, ticks};
use crate::config::MermaidConfig;
use crate::diagram::wardley::ID_MARK;
use crate::output::svg::{esc, esc_text, js_num as n};
use crate::text::svg_bbox_width;
use std::fmt::Write;

/// `Object.keys` order for string keys: the canonical array indices first,
/// ascending, then the others as they were first added.
fn js_key_order(keys: Vec<String>) -> Vec<String> {
    let index = |k: &str| {
        k.parse::<u32>()
            .ok()
            .filter(|n| n.to_string() == k && *n != u32::MAX)
    };
    let mut ints: Vec<(u32, String)> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    for k in keys {
        match index(&k) {
            Some(n) => ints.push((n, k)),
            None => rest.push(k),
        }
    }
    ints.sort_by_key(|(n, _)| *n);
    ints.into_iter().map(|(_, k)| k).chain(rest).collect()
}

/// A tag as the serialiser writes it: `<name a="v"/>` when it has no content.
fn tag(out: &mut String, name: &str, attrs: &[(&str, String)], text: &str) {
    out.push('<');
    out.push_str(name);
    for (k, v) in attrs {
        // The id placeholder's control characters are not for `esc` to drop.
        let v = v.split(ID_MARK).map(esc).collect::<Vec<_>>().join(ID_MARK);
        let _ = write!(out, " {k}=\"{v}\"");
    }
    if text.is_empty() {
        out.push_str("/>");
    } else {
        let _ = write!(out, ">{}</{name}>", esc_text(text));
    }
}

fn kv(k: &'static str, v: String) -> (&'static str, String) {
    (k, v)
}

/// What `getBBox().width` reports for a task label, as a 32-bit float.
fn bbox_width(label: &str, font: f64) -> f64 {
    f64::from(svg_bbox_width(label, font) as f32)
}

/// `lineBreakRegex` (`/<br\s*\/?>/gi`) split.
fn split_line_breaks(s: &str) -> Vec<&str> {
    let lower = s.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut parts = Vec::new();
    let (mut last, mut i) = (0, 0);
    while i < b.len() {
        if b[i] == b'<' && lower[i..].starts_with("<br") {
            let mut j = i + 3;
            while j < b.len() && (b[j] as char).is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'/' {
                j += 1;
            }
            if j < b.len() && b[j] == b'>' {
                parts.push(&s[last..i]);
                last = j + 1;
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    parts.push(&s[last..]);
    parts
}

/// The drawing as mermaid's `ganttRenderer` leaves it in the DOM, after the
/// stylesheet. The `id` placeholder is [`ID_MARK`].
pub(super) fn dom(g: &Gantt, cfg: &MermaidConfig, now: Option<i64>) -> String {
    let bar_h = cfg.section_f64("gantt", "barHeight", 20.0);
    let bar_gap = cfg.section_f64("gantt", "barGap", 4.0);
    let top_pad = cfg.section_f64("gantt", "topPadding", 50.0);
    let left_pad = cfg.section_f64("gantt", "leftPadding", 75.0);
    let right_pad = cfg.section_f64("gantt", "rightPadding", 75.0);
    let grid_pad = cfg.section_f64("gantt", "gridLineStartPadding", 35.0);
    let font = cfg.section_f64("gantt", "fontSize", 11.0);
    let sec_font = cfg.section_f64("gantt", "sectionFontSize", 11.0);
    let title_margin = cfg.section_f64("gantt", "titleTopMargin", 25.0);
    let n_styles = cfg.section_f64("gantt", "numberSectionStyles", 4.0);
    let w = cfg.section_f64("gantt", "useWidth", 784.0);
    let compact = cfg
        .section("gantt", "displayMode")
        .and_then(|j| j.as_str())
        .is_some_and(|m| m == "compact");
    let top_axis = g.top_axis || cfg.section_bool("gantt", "topAxis", false);
    let gap = bar_h + bar_gap;

    let ty = |t: &Task| g.section_text[t.section].as_str();
    let plain: Vec<usize> = (0..g.tasks.len()).filter(|&i| !g.tasks[i].vert).collect();
    let mut categories: Vec<&str> = Vec::new();
    for &i in &plain {
        if !categories.contains(&ty(&g.tasks[i])) {
            categories.push(ty(&g.tasks[i]));
        }
    }
    // `i % numberSectionStyles` for the category named `name`.
    let style_of = |name: &str| {
        categories
            .iter()
            .position(|c| *c == name)
            .map(|i| i as f64 % n_styles)
    };
    let sec_of = |t: &Task| style_of(ty(t)).unwrap_or(0.0);

    // Rows: `order` is the index among the plain tasks, or (compact) the lane
    // `getMaxIntersections` finds within the task's section.
    let mut order: Vec<i64> = vec![-1; g.tasks.len()];
    for (k, &i) in plain.iter().enumerate() {
        order[i] = k as i64;
    }
    let mut h = 2.0 * top_pad;
    let mut heights: Vec<(String, f64)> = Vec::new();
    if compact {
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for &i in &plain {
            let name = ty(&g.tasks[i]);
            match groups.iter_mut().find(|(n, _)| n == name) {
                Some((_, v)) => v.push(i),
                None => groups.push((name.to_string(), vec![i])),
            }
        }
        let names = js_key_order(groups.iter().map(|(n, _)| n.clone()).collect());
        let mut offset = 0i64;
        for name in names {
            let idxs = &groups.iter().find(|(n, _)| *n == name).unwrap().1;
            let mut timeline = vec![i64::MIN; idxs.len()];
            let mut by_start = idxs.clone();
            by_start.sort_by_key(|&i| (g.tasks[i].start, order[i]));
            let mut max_j = 0;
            for &i in &by_start {
                for (j, busy) in timeline.iter_mut().enumerate() {
                    if g.tasks[i].start >= *busy {
                        *busy = g.tasks[i].end;
                        order[i] = j as i64 + offset;
                        max_j = max_j.max(j);
                        break;
                    }
                }
            }
            let cat_h = (max_j + 1) as f64;
            offset += max_j as i64 + 1;
            h += cat_h * (bar_h + bar_gap);
            heights.push((name, cat_h));
        }
    } else {
        h += plain.len() as f64 * (bar_h + bar_gap);
        for c in js_key_order(categories.iter().map(|c| c.to_string()).collect()) {
            let count = plain.iter().filter(|&&i| ty(&g.tasks[i]) == c).count();
            heights.push((c, count as f64));
        }
    }

    // `scaleTime().domain([min start, max end]).rangeRound([0, w - paddings])`.
    let t0 = g.tasks.iter().map(|t| t.start).min().unwrap_or(0);
    let t1 = g.tasks.iter().map(|t| t.end).max().unwrap_or(0);
    let inner = w - left_pad - right_pad;
    let empty = g.tasks.is_empty();
    let scale = |t: i64| -> f64 {
        if empty {
            return f64::NAN;
        }
        let k = if t1 == t0 {
            0.5
        } else {
            (t - t0) as f64 / (t1 - t0) as f64
        };
        (0.0 * (1.0 - k) + inner * k + 0.5).floor()
    };

    // taskArray sorted by start (stable), then the vert tasks last.
    let mut sorted: Vec<usize> = (0..g.tasks.len()).collect();
    sorted.sort_by_key(|&i| g.tasks[i].start);
    sorted.sort_by_key(|&i| g.tasks[i].vert);

    let mut out = String::new();
    out.push_str("<g/>");

    // drawExcludeDays
    if !(g.excludes.is_empty() && g.includes.is_empty()) && !empty && t1 - t0 <= 5 * 366 * DAY_MS {
        let mut ranges: Vec<(i64, i64)> = Vec::new();
        let mut run: Option<(i64, i64)> = None;
        let mut d = t0;
        while d <= t1 {
            if is_excluded(g, d) {
                run = Some(run.map_or((d, d), |(a, _)| (a, d)));
            } else if let Some(r) = run.take() {
                ranges.push(r);
            }
            d += DAY_MS;
        }
        ranges.extend(run);
        out.push_str("<g>");
        for (i, (a, b)) in ranges.into_iter().enumerate() {
            let (y, m, dd) = civil_from_days(a.div_euclid(DAY_MS));
            let x0 = scale(a.div_euclid(DAY_MS) * DAY_MS);
            let x1 = scale(b.div_euclid(DAY_MS) * DAY_MS + DAY_MS - 1);
            tag(
                &mut out,
                "rect",
                &[
                    kv("id", format!("{ID_MARK}-exclude-{y:04}-{m:02}-{dd:02}")),
                    kv("x", n(x0 + left_pad)),
                    kv("y", n(grid_pad)),
                    kv("width", n(x1 - x0)),
                    kv("height", n(h - top_pad - grid_pad)),
                    kv(
                        "transform-origin",
                        format!(
                            "{}px {}px",
                            n(scale(a) + left_pad + 0.5 * (scale(b) - scale(a))),
                            n(i as f64 * gap + 0.5 * h)
                        ),
                    ),
                    kv("class", "exclude-range".into()),
                ],
                "",
            );
        }
        out.push_str("</g>");
    }

    // makeGrid
    let axis_format = g
        .axis_format
        .clone()
        .or_else(|| {
            cfg.section("gantt", "axisFormat")
                .and_then(|j| j.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "%Y-%m-%d".into());
    let tick_list: Vec<i64> = if empty || t1 == t0 {
        Vec::new()
    } else {
        let (step, unit) = g
            .tick_interval
            .unwrap_or_else(|| auto_interval(t0, t1, 10.0));
        let mut list = ticks(t0, t1, step, unit, g.week_start);
        if list.len() > 200 {
            let (s2, u2) = auto_interval(t0, t1, 10.0);
            list = ticks(t0, t1, s2, u2, g.week_start);
        }
        list
    };
    for top in [false, true] {
        if top && !top_axis {
            break;
        }
        // d3-axis with `tickSize(-h + topPadding + gridLineStartPadding)`; `k`
        // is -1 above the chart.
        let size = -h + top_pad + grid_pad;
        let k = if top { -1.0 } else { 1.0 };
        let y = if top { top_pad } else { h - 50.0 };
        let _ = write!(
            out,
            "<g class=\"grid\" transform=\"translate({}, {})\" fill=\"none\" font-size=\"10\" font-family=\"sans-serif\" text-anchor=\"middle\">",
            n(left_pad),
            n(y)
        );
        let outer = k * size;
        let d = if outer != 0.0 {
            format!("M0.5,{}V0.5H{}V{}", n(outer), n(inner + 0.5), n(outer))
        } else {
            format!("M0.5,0.5H{}", n(inner + 0.5))
        };
        tag(
            &mut out,
            "path",
            &[
                kv("class", "domain".into()),
                kv("stroke", "currentColor".into()),
                kv("d", d),
            ],
            "",
        );
        for &t in &tick_list {
            let _ = write!(
                out,
                "<g class=\"tick\" opacity=\"1\" transform=\"translate({},0)\">",
                n(scale(t) + 0.5)
            );
            tag(
                &mut out,
                "line",
                &[kv("stroke", "currentColor".into()), kv("y2", n(k * size))],
                "",
            );
            tag(
                &mut out,
                "text",
                &[
                    kv("fill", "#000".into()),
                    kv("y", n(k * 3.0)),
                    kv("dy", if top { "0em" } else { "1em" }.into()),
                    kv("stroke", "none".into()),
                    kv("font-size", "10".into()),
                    kv("style", "text-anchor: middle;".into()),
                ],
                &format_time(t, &axis_format),
            );
            out.push_str("</g>");
        }
        out.push_str("</g>");
    }

    // drawRects: the section bands, one per distinct row.
    out.push_str("<g>");
    let mut seen: Vec<i64> = Vec::new();
    for &i in sorted.iter().filter(|&&i| !g.tasks[i].vert) {
        if seen.contains(&order[i]) {
            continue;
        }
        seen.push(order[i]);
        let cls = match style_of(ty(&g.tasks[i])) {
            Some(k) => format!("section section{}", n(k)),
            None => "section section0".into(),
        };
        tag(
            &mut out,
            "rect",
            &[
                kv("x", "0".into()),
                kv("y", n(order[i] as f64 * gap + top_pad - 2.0)),
                kv("width", n(w - right_pad / 2.0)),
                kv("height", n(gap)),
                kv("class", cls),
            ],
            "",
        );
    }
    out.push_str("</g>");

    // The task bars, then their labels.
    out.push_str("<g>");
    let n_plain = plain.len() as f64;
    for &i in &sorted {
        let t = &g.tasks[i];
        let o = order[i] as f64;
        let (s, e) = (scale(t.start), scale(t.end));
        let x = if t.milestone {
            scale(t.start) + left_pad + 0.5 * (scale(t.end) - scale(t.start)) - 0.5 * bar_h
        } else {
            scale(t.start) + left_pad
        };
        let y = if t.vert { grid_pad } else { o * gap + top_pad };
        let width = if t.milestone {
            bar_h
        } else if t.vert {
            0.08 * bar_h
        } else {
            scale(t.render_end) - scale(t.start)
        };
        let height = if t.vert {
            n_plain * (bar_h + bar_gap) + bar_h * 2.0
        } else {
            bar_h
        };
        let mut kind = String::new();
        if t.active {
            kind = if t.crit { " activeCrit" } else { " active" }.into();
        } else if t.done {
            kind = if t.crit { " doneCrit" } else { " done" }.into();
        } else if t.crit {
            kind.push_str(" crit");
        }
        if kind.is_empty() {
            kind = " task".into();
        }
        if t.milestone {
            kind = format!(" milestone {kind}");
        }
        if t.vert {
            kind = format!(" vert {kind}");
        }
        let cls = format!("task{kind}{} ", n(sec_of(t)));
        tag(
            &mut out,
            "rect",
            &[
                kv("id", format!("{ID_MARK}-{}", t.id)),
                kv("rx", "3".into()),
                kv("ry", "3".into()),
                kv("x", n(x)),
                kv("y", n(y)),
                kv("width", n(width)),
                kv("height", n(height)),
                kv(
                    "transform-origin",
                    format!(
                        "{}px {}px",
                        n(s + left_pad + 0.5 * (e - s)),
                        n(o * gap + top_pad + 0.5 * bar_h)
                    ),
                ),
                kv("class", cls.trim().to_string()),
            ],
            "",
        );
    }
    for &i in &sorted {
        let t = &g.tasks[i];
        let o = order[i] as f64;
        let tw = bbox_width(&t.raw_label, font);
        let mut start_x = scale(t.start);
        let mut end_x = scale(t.render_end);
        if t.milestone {
            start_x += 0.5 * (scale(t.end) - scale(t.start)) - 0.5 * bar_h;
            end_x = start_x + bar_h;
        }
        let x = if t.vert {
            scale(t.start) + left_pad
        } else if tw > end_x - start_x {
            if end_x + tw + 1.5 * left_pad > w {
                start_x + left_pad - 5.0
            } else {
                end_x + left_pad + 5.0
            }
        } else {
            (end_x - start_x) / 2.0 + start_x + left_pad
        };
        let y = if t.vert {
            grid_pad + n_plain * (bar_h + bar_gap) + 60.0
        } else {
            o * gap + bar_h / 2.0 + (font / 2.0 - 2.0) + top_pad
        };
        let sx = scale(t.start);
        let mut ex = scale(t.end);
        if t.milestone {
            ex = sx + bar_h;
        }
        let sn = n(sec_of(t));
        let mut kind = String::new();
        if t.active {
            kind = if t.crit {
                format!("activeCritText{sn}")
            } else {
                format!("activeText{sn}")
            };
        }
        if t.done {
            kind = if t.crit {
                format!("{kind} doneCritText{sn}")
            } else {
                format!("{kind} doneText{sn}")
            };
        } else if t.crit {
            kind = format!("{kind} critText{sn}");
        }
        if t.milestone {
            kind.push_str(" milestoneText");
        }
        if t.vert {
            kind.push_str(" vertText");
        }
        let cls = if tw > ex - sx {
            if ex + tw + 1.5 * left_pad > w {
                format!(" taskTextOutsideLeft taskTextOutside{sn} {kind}")
            } else {
                format!(
                    " taskTextOutsideRight taskTextOutside{sn} {kind} width-{}",
                    n(tw)
                )
            }
        } else {
            format!(" taskText taskText{sn} {kind} width-{}", n(tw))
        };
        tag(
            &mut out,
            "text",
            &[
                kv("id", format!("{ID_MARK}-{}-text", t.id)),
                kv("font-size", n(font)),
                kv("x", n(x)),
                kv("y", n(y)),
                kv("class", cls.trim().to_string()),
            ],
            &t.raw_label,
        );
    }
    out.push_str("</g>");

    // vertLabels
    out.push_str("<g>");
    let mut prev_gap = 0.0;
    for (i, (name, count)) in heights.iter().enumerate() {
        let rows = split_line_breaks(name);
        let dy = -((rows.len() - 1) as f64) / 2.0;
        let y = if i > 0 {
            prev_gap += heights[i - 1].1;
            count * gap / 2.0 + prev_gap * gap + top_pad
        } else {
            count * gap / 2.0 + top_pad
        };
        let cls = match style_of(name) {
            Some(k) => format!("sectionTitle sectionTitle{}", n(k)),
            None => "sectionTitle".into(),
        };
        let _ = write!(
            out,
            "<text dy=\"{}em\" x=\"10\" y=\"{}\" font-size=\"{}\" class=\"{}\">",
            n(dy),
            n(y),
            n(sec_font),
            esc(&cls)
        );
        for (j, row) in rows.iter().enumerate() {
            let mut attrs = vec![
                kv("alignment-baseline", "central".into()),
                kv("x", "10".into()),
            ];
            if j > 0 {
                attrs.push(kv("dy", "1em".into()));
            }
            tag(&mut out, "tspan", &attrs, row);
        }
        out.push_str("</text>");
    }
    out.push_str("</g>");

    // drawToday
    if let Some(now) = now {
        let marker = g.today_marker.as_deref().unwrap_or("");
        if marker != "off" {
            out.push_str("<g class=\"today\">");
            let x = scale(now) + left_pad;
            let mut attrs = vec![
                kv("x1", n(x)),
                kv("x2", n(x)),
                kv("y1", n(title_margin)),
                kv("y2", n(h - title_margin)),
                kv("class", "today".into()),
            ];
            if !marker.is_empty() {
                attrs.push(kv("style", marker.replace(',', ";")));
            }
            tag(&mut out, "line", &attrs, "");
            out.push_str("</g>");
        }
    }

    // The title text, empty when there is none. `setDiagramTitle` sanitises it
    // as HTML, which `.text()` then keeps literally: its markup is escaped
    // twice over.
    let title = g.title_text.as_deref().unwrap_or("");
    tag(
        &mut out,
        "text",
        &[
            kv("x", n(w / 2.0)),
            kv("y", n(title_margin)),
            kv("class", "titleText".into()),
        ],
        &esc_text(title),
    );
    out
}
