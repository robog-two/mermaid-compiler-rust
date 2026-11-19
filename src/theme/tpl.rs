//! Expansion of transcribed JavaScript template literals (`${expr}`).

use super::styles::Opts;

/// Expands every `${expr}` of `t` with `extra(expr)` or, failing that, with the
/// simple expressions `options.name`, string and number literals joined with
/// `||` or `??`. An undefined option prints `undefined`, as JavaScript does.
pub fn tpl(o: &Opts, t: &str, extra: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let body = &rest[at + 2..];
        let mut depth = 1usize;
        let mut end = body.len();
        for (i, ch) in body.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let expr = body[..end].trim();
        let v = extra(expr).unwrap_or_else(|| simple(o, expr));
        out.push_str(&v);
        rest = body.get(end + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

fn operand(o: &Opts, e: &str) -> Option<String> {
    let e = e.trim();
    if let Some(name) = e.strip_prefix("options.") {
        let v = o.get(name).map(str::to_string);
        if v.is_none() && std::env::var_os("MMDC_STYLE_DEBUG").is_some() {
            eprintln!("undefined option {name} ({:?})", o.theme);
        }
        return v;
    }
    let quoted = |q: char| e.strip_prefix(q).and_then(|x| x.strip_suffix(q));
    if let Some(l) = quoted('\'').or_else(|| quoted('"')) {
        return Some(l.to_string());
    }
    if e.parse::<f64>().is_ok() {
        return Some(e.to_string());
    }
    None
}

fn simple(o: &Opts, e: &str) -> String {
    let e = e.trim().trim_start_matches('(').trim_end_matches(')');
    let nullish = e.contains("??");
    let parts: Vec<&str> = if nullish {
        e.split("??").collect()
    } else {
        e.split("||").collect()
    };
    let mut last = None;
    for p in &parts {
        last = operand(o, p);
        if let Some(v) = &last {
            if nullish || (!v.is_empty() && v != "false") {
                return v.clone();
            }
        }
    }
    last.unwrap_or_else(|| "undefined".to_string())
}
