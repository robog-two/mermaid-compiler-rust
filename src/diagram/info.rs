//! `info` diagrams: a small card naming this implementation and its version.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{Point, Rect};
use crate::scene::{DiagramKind, Element, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::Palette;

/// Parsed `info` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub show_info: bool,
}

/// Parse `info [showInfo]`.
pub fn parse(body: &str) -> Result<Info> {
    let mut header = false;
    let mut show_info = false;
    for (i, raw) in body.lines().enumerate() {
        let line = i + 1;
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        let mut words = t.split_whitespace();
        if !header {
            if words.next() != Some("info") {
                return Err(Error::parse(line, "Expecting 'info'"));
            }
            header = true;
        }
        for w in words {
            match w {
                "showInfo" => show_info = true,
                other => {
                    return Err(Error::parse(
                        line,
                        format!("Unexpected '{other}': expecting 'showInfo'"),
                    ));
                }
            }
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'info'"));
    }
    Ok(Info { show_info })
}

/// Lines shown on the card.
pub fn lines(info: &Info) -> Vec<String> {
    let mut v = vec![format!("mmdc-rs v{}", crate::VERSION)];
    if info.show_info {
        v.push("Pure-Rust, browser-free mermaid renderer".into());
    }
    v
}

/// Parse and render an `info` diagram body (already preprocessed).
pub fn render(
    body: &str,
    _title: Option<String>,
    _cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let info = parse(body)?;
    let text = lines(&info);
    let head = TextBlock::new(vec![text[0].clone()], 20.0, true);
    let rest = TextBlock::new(text[1..].to_vec(), 14.0, false);
    let w = head.width.max(rest.width) + 48.0;
    let h = head.height + rest.height + 32.0;
    let card = Rect::new(0.0, 0.0, w, h);
    let mut els = vec![Element::rect(
        card,
        6.0,
        Paint::both(&pal.main_bkg, &pal.node_border, 1.0),
    )];
    let top = 16.0;
    els.push(Element::text(
        Text::centered(
            Point::new(w / 2.0, top + head.height / 2.0),
            &head.lines,
            20.0,
            head.line_height,
            &pal.node_text,
        )
        .bold(),
    ));
    if !rest.is_empty() {
        els.push(Element::text(Text::centered(
            Point::new(w / 2.0, top + head.height + rest.height / 2.0),
            &rest.lines,
            14.0,
            rest.line_height,
            &pal.node_text,
        )));
    }
    Ok(Scene {
        kind: DiagramKind::Info,
        view_box: card.pad(10.0),
        elements: vec![Element::group("info", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_info_and_show_info() {
        assert!(!parse("info").unwrap().show_info);
        assert!(parse("\n  info showInfo\n").unwrap().show_info);
        assert!(parse("info\nshowInfo").unwrap().show_info);
        assert!(matches!(
            parse("info\n\nbogus"),
            Err(Error::Parse { line: 3, .. })
        ));
    }

    #[test]
    fn renders_version() {
        let cfg = MermaidConfig::default();
        let s = render("info showInfo", None, &cfg, &cfg.palette()).unwrap();
        assert_eq!(s.kind, DiagramKind::Info);
        assert!(lines(&Info { show_info: false })[0].contains(crate::VERSION));
        assert!(s.view_box.w > 100.0);
    }
}
