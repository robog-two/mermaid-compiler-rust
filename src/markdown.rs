//! Markdown mode: find mermaid code blocks and replace them with image links.
//!
//! Matches mermaid-cli's regex exactly (including its quirks):
//! `/^[^\S\n]*[`:]{3}(?:mermaid)([^\S\n]*\r?\n([\s\S]*?))[`:]{3}[^\S\n]*$/gm`

/// A mermaid block found in Markdown: byte range of the whole match and the definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chart<'a> {
    pub start: usize,
    pub end: usize,
    pub definition: &'a str,
}

fn is_hspace(b: u8) -> bool {
    b != b'\n' && (b == b' ' || b == b'\t' || b == b'\r' || b == 0x0b || b == 0x0c)
}

fn is_fence(b: u8) -> bool {
    b == b'`' || b == b':'
}

/// Find all charts, in order.
pub fn find_charts(md: &str) -> Vec<Chart<'_>> {
    let s = md.as_bytes();
    let mut out = Vec::new();
    let mut line_start = 0usize;
    while line_start <= s.len() {
        if let Some((end, def_start, def_end)) = match_at(s, line_start) {
            out.push(Chart {
                start: line_start,
                end,
                definition: &md[def_start..def_end],
            });
            // Resume at the next line start after the match.
            match s[end..].iter().position(|&b| b == b'\n') {
                Some(p) => line_start = end + p + 1,
                None => break,
            }
            continue;
        }
        match s[line_start..].iter().position(|&b| b == b'\n') {
            Some(p) => line_start += p + 1,
            None => break,
        }
    }
    out
}

fn match_at(s: &[u8], start: usize) -> Option<(usize, usize, usize)> {
    let mut i = start;
    while i < s.len() && is_hspace(s[i]) {
        i += 1;
    }
    if i + 3 > s.len() || !s[i..i + 3].iter().all(|&b| is_fence(b)) {
        return None;
    }
    i += 3;
    if !s[i..].starts_with(b"mermaid") {
        return None;
    }
    i += 7;
    while i < s.len() && is_hspace(s[i]) {
        i += 1;
    }
    if i >= s.len() || s[i] != b'\n' {
        return None;
    }
    let def_start = i + 1;
    // Lazy: first position q where three fence chars are followed by optional
    // horizontal space and then end-of-line or end-of-input.
    let mut q = def_start;
    while q + 3 <= s.len() {
        if s[q..q + 3].iter().all(|&b| is_fence(b)) {
            let mut e = q + 3;
            while e < s.len() && is_hspace(s[e]) {
                e += 1;
            }
            if e == s.len() || s[e] == b'\n' {
                return Some((e, def_start, q));
            }
        }
        q += 1;
    }
    None
}

/// Markdown image properties (`![alt](url "title")`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownImage {
    pub url: String,
    pub alt: String,
    pub title: Option<String>,
}

/// Same escaping as mermaid-cli's `markdownImage`.
pub fn markdown_image(img: &MarkdownImage) -> String {
    let alt: String = img
        .alt
        .chars()
        .flat_map(|c| {
            if matches!(c, '[' | ']' | '\\') {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect();
    match img.title.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => {
            let t: String = t
                .chars()
                .flat_map(|c| {
                    if matches!(c, '"' | '\\') {
                        vec!['\\', c]
                    } else {
                        vec![c]
                    }
                })
                .collect();
            format!("![{alt}]({} \"{t}\")", img.url)
        }
        None => format!("![{alt}]({})", img.url),
    }
}

/// Replace each chart with its image (`images.len()` must equal `charts.len()`).
pub fn replace_charts(md: &str, charts: &[Chart], images: &[MarkdownImage]) -> String {
    let mut out = String::with_capacity(md.len());
    let mut last = 0;
    for (c, img) in charts.iter().zip(images) {
        out.push_str(&md[last..c.start]);
        out.push_str(&markdown_image(img));
        last = c.end;
    }
    out.push_str(&md[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_backtick_and_colon_fences() {
        let md = "# T\n\n```mermaid\ngraph TD\nA-->B\n```\n\ntext\n\n:::mermaid\npie\n\"a\":1\n:::\n\n```js\nx\n```\n";
        let c = find_charts(md);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].definition, "graph TD\nA-->B\n");
        assert_eq!(c[1].definition, "pie\n\"a\":1\n");
    }

    #[test]
    fn indented_and_trailing_space() {
        let md = "  ```mermaid  \n  graph LR\n  A-->B\n  ```  \nafter";
        let c = find_charts(md);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].definition, "  graph LR\n  A-->B\n  ");
        let out = replace_charts(
            md,
            &c,
            &[MarkdownImage {
                url: "./x-1.svg".into(),
                alt: "diagram".into(),
                title: None,
            }],
        );
        assert_eq!(out, "![diagram](./x-1.svg)\nafter");
    }

    #[test]
    fn unterminated_block_is_ignored() {
        assert!(find_charts("```mermaid\ngraph TD\n").is_empty());
        assert!(find_charts("```mermaidx\ngraph\n```").is_empty());
    }

    #[test]
    fn image_escaping() {
        let img = MarkdownImage {
            url: "./a.svg".into(),
            alt: "a [b]".into(),
            title: Some("say \"hi\"".into()),
        };
        assert_eq!(
            markdown_image(&img),
            "![a \\[b\\]](./a.svg \"say \\\"hi\\\"\")"
        );
    }

    #[test]
    fn crlf_input() {
        let md = "```mermaid\r\ngraph TD\r\nA-->B\r\n```\r\n";
        assert_eq!(find_charts(md).len(), 1);
    }
}
