//! The part of `stylis` (v4) that mermaid's `compileCSS` relies on: nested rules
//! are flattened and namespaced with `#svgId`, whitespace is collapsed the way
//! stylis's tokenizer collapses it, comments are dropped and every declaration is
//! terminated with `;`.
//!
//! The text mermaid puts in the first `<style>` of every diagram is this
//! function's output, so the details that look arbitrary (no space after a comma
//! outside parentheses, `9,5!important`, parenthesised values copied verbatim,
//! empty rules dropped) are stylis's.

/// One flattened output item, in stylis's emission order.
enum Item {
    /// `selectors{decls}` (selectors already resolved against the parents).
    Rule { selectors: Vec<String>, decls: Vec<String> },
    /// `@media ...{` / `@keyframes ...{` and its closing brace.
    Open(String),
    Close,
    /// A rule inside `@keyframes`: its selectors are not namespaced.
    Frame { selector: String, decls: Vec<String> },
}

/// `compile("#id", css)`: `serialize(compile("#id{css}"))` with mermaid's
/// namespacing middleware.
pub fn compile(namespace: &str, css: &str) -> String {
    let wrapped = format!("{namespace}{{{css}}}");
    let chars: Vec<char> = wrapped.chars().collect();
    let mut p = Parser {
        c: &chars,
        i: 0,
        items: Vec::new(),
    };
    p.block(&[namespace.to_string()], false, true);
    let mut out = String::new();
    for it in &p.items {
        match it {
            Item::Rule { selectors, decls } => {
                if decls.is_empty() {
                    continue;
                }
                let props: Vec<String> = selectors
                    .iter()
                    .map(|s| namespaced(namespace, s, decls))
                    .collect();
                out.push_str(&props.join(","));
                out.push('{');
                for d in decls {
                    out.push_str(d);
                }
                out.push('}');
            }
            Item::Frame { selector, decls } => {
                if decls.is_empty() {
                    continue;
                }
                out.push_str(selector);
                out.push('{');
                for d in decls {
                    out.push_str(d);
                }
                out.push('}');
            }
            Item::Open(h) => {
                out.push_str(h);
                out.push('{');
            }
            Item::Close => out.push('}'),
        }
    }
    out
}

/// mermaid's `addNamespace` middleware.
fn namespaced(ns: &str, prop: &str, decls: &[String]) -> String {
    if prop == ns {
        let allowed = decls.iter().all(|d| {
            let name = d.split(':').next().unwrap_or("");
            matches!(name, "font-family" | "font-size" | "fill")
        });
        if allowed {
            return prop.to_string();
        }
    }
    let already = (prop.starts_with(&format!("{ns} ")) || prop.starts_with(&format!("{ns}>")))
        && !prop.starts_with(&format!("{ns} ||"));
    if already {
        prop.to_string()
    } else {
        format!("{ns} {prop}")
    }
}

struct Parser<'a> {
    c: &'a [char],
    i: usize,
    items: Vec<Item>,
}

/// stylis's `token()`: 4 isolates (`! + , / > @ ~` and `; { }`), 3 is `:`.
fn token(c: char) -> u8 {
    match c {
        '\0' | '\t' | '\n' | '\r' | ' ' => 5,
        '!' | '+' | ',' | '/' | '>' | '@' | '~' | ';' | '{' | '}' => 4,
        ':' => 3,
        '"' | '\'' | '(' | '[' => 2,
        ')' | ']' => 1,
        _ => 0,
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    /// Skips a comment at the cursor (`/* */` or `// ...`); true when it did.
    fn comment(&mut self) -> bool {
        if self.peek() != Some('/') {
            return false;
        }
        match self.c.get(self.i + 1) {
            Some('*') => {
                self.i += 2;
                while self.i < self.c.len() {
                    if self.c[self.i] == '*' && self.c.get(self.i + 1) == Some(&'/') {
                        self.i += 2;
                        return true;
                    }
                    self.i += 1;
                }
                true
            }
            Some('/') => {
                while self.i < self.c.len() && self.c[self.i] != '\n' {
                    self.i += 1;
                }
                true
            }
            _ => false,
        }
    }

    /// Copies a delimited run (`"..."`, `(...)`, `[...]`) verbatim.
    fn delimited(&mut self, buf: &mut String) {
        let open = self.c[self.i];
        let close = match open {
            '(' => ')',
            '[' => ']',
            q => q,
        };
        buf.push(open);
        self.i += 1;
        while let Some(ch) = self.peek() {
            buf.push(ch);
            self.i += 1;
            if ch == '\\' {
                if let Some(n) = self.peek() {
                    buf.push(n);
                    self.i += 1;
                }
            } else if ch == close {
                return;
            } else if (ch == '"' || ch == '\'') && open != '"' && open != '\'' {
                // A quoted run inside brackets or parentheses.
                let q = ch;
                while let Some(n) = self.peek() {
                    buf.push(n);
                    self.i += 1;
                    if n == '\\' {
                        if let Some(m) = self.peek() {
                            buf.push(m);
                            self.i += 1;
                        }
                    } else if n == q {
                        break;
                    }
                }
            }
        }
    }

    /// Reads up to (not including) the next `{`, `;` or `}`, with stylis's
    /// whitespace rules applied. Returns the text and the terminator.
    fn chunk(&mut self) -> (String, Option<char>) {
        let mut buf = String::new();
        loop {
            let Some(ch) = self.peek() else {
                return (buf, None);
            };
            match ch {
                '{' | ';' | '}' => {
                    self.i += 1;
                    return (buf, Some(ch));
                }
                '/' if self.comment() => {}
                '"' | '\'' | '(' | '[' => self.delimited(&mut buf),
                ' ' | '\t' | '\n' | '\r' => {
                    while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
                        self.i += 1;
                    }
                    // A comment after the whitespace is skipped before looking
                    // at what follows.
                    while self.comment() {
                        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
                            self.i += 1;
                        }
                    }
                    let prev = buf.chars().last().unwrap_or('\0');
                    let next = self.peek().unwrap_or('\0');
                    if !(token(prev) > 2 || token(next) > 3) {
                        buf.push(' ');
                    }
                }
                _ => {
                    buf.push(ch);
                    self.i += 1;
                }
            }
        }
    }

    /// Parses rules and declarations until the closing brace of the current
    /// block. `parents` are the resolved selectors of the enclosing rule.
    fn block(&mut self, parents: &[String], frames: bool, top: bool) {
        // The wrapper `ns{...}` of `compile`: the first chunk is the namespace
        // itself, already known as the parent.
        if top {
            let _ = self.chunk();
        }
        let mut own: Option<usize> = None;
        if !frames {
            self.items.push(Item::Rule {
                selectors: parents.to_vec(),
                decls: Vec::new(),
            });
            own = Some(self.items.len() - 1);
        }
        loop {
            let (text, term) = self.chunk();
            let text = text.trim().to_string();
            match term {
                Some('{') => {
                    if text.starts_with('@') {
                        let keyframes = text.contains("keyframes");
                        if keyframes {
                            self.items.push(Item::Open(text));
                            self.block(parents, true, false);
                            self.items.push(Item::Close);
                        } else {
                            self.items.push(Item::Open(text));
                            self.block(parents, false, false);
                            self.items.push(Item::Close);
                        }
                    } else if frames {
                        self.items.push(Item::Frame {
                            selector: text,
                            decls: Vec::new(),
                        });
                        let at = self.items.len() - 1;
                        self.frame_decls(at);
                    } else {
                        let sels = resolve(parents, &text);
                        self.block(&sels, false, false);
                    }
                }
                Some(';') | Some('}') | None => {
                    if let (Some(at), Some(d)) = (own, declaration(&text)) {
                        if let Item::Rule { decls, .. } = &mut self.items[at] {
                            decls.push(d);
                        }
                    }
                    if term != Some(';') {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    fn frame_decls(&mut self, at: usize) {
        loop {
            let (text, term) = self.chunk();
            if let Some(d) = declaration(text.trim()) {
                if let Item::Frame { decls, .. } = &mut self.items[at] {
                    decls.push(d);
                }
            }
            if term != Some(';') {
                return;
            }
        }
    }
}

/// `prop:value;` as stylis writes a declaration.
fn declaration(text: &str) -> Option<String> {
    let text = text.trim();
    let colon = text.find(':')?;
    // A declaration without a property or a value is dropped.
    if colon == 0 || text[colon + 1..].trim().is_empty() {
        return None;
    }
    let mut s = text.to_string();
    // `prop : value`: stylis removes the (first) space of the declaration.
    if s[..colon].ends_with(' ') {
        s.remove(s.find(' ').unwrap_or(0));
    }
    s.push(';');
    Some(s)
}

/// Resolves the comma separated selector list `text` against the parents.
fn resolve(parents: &[String], text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, None::<char>, 0usize);
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    for &(i, ch) in &bytes {
        match (ch, quote) {
            (q, Some(open)) if q == open => quote = None,
            (_, Some(_)) => {}
            ('"' | '\'', None) => quote = Some(ch),
            ('(' | '[', None) => depth += 1,
            (')' | ']', None) => depth -= 1,
            (',', None) if depth == 0 => {
                parts.push(text[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim().to_string());
    let mut out = Vec::new();
    for sel in parts.iter().filter(|s| !s.is_empty()) {
        for parent in parents {
            if sel.contains('&') {
                out.push(sel.replace('&', parent));
            } else {
                out.push(format!("{parent} {sel}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::compile;

    #[test]
    fn flattens_namespaces_and_minifies() {
        let css = "& { font-family: \"a b\", c; fill: #333 }\n\
            @keyframes dash { to { stroke-dashoffset: 0; } }\n\
            & .slow { stroke-dasharray: 9,5 !important; /* c */ x : 1 }\n\
            // gone\n\
            .a, .b > .c { stroke-width : 2px; filter: drop-shadow(1px 2px  rgba(1, 2, 3, 1)) }\n\
            .empty { }\n";
        assert_eq!(
            compile("#i", css),
            "#i{font-family:\"a b\",c;fill:#333;}@keyframes dash{to{stroke-dashoffset:0;}}\
             #i .slow{stroke-dasharray:9,5!important;x:1;}\
             #i .a,#i .b>.c{stroke-width:2px;filter:drop-shadow(1px 2px  rgba(1, 2, 3, 1));}"
        );
    }

    #[test]
    fn root_rule_with_other_declarations_is_namespaced() {
        assert_eq!(compile("#i", "& { stroke: red }"), "#i #i{stroke:red;}");
    }
}
