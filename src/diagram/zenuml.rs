//! ZenUML diagrams, lowered onto the sequence-diagram IR and renderer.
//!
//! ZenUML's code-like syntax (`A.method(args) { ... }`, `x = B.call()`,
//! `if/else`, `while`, `par`, `try/catch/finally`, ...) is parsed into a small
//! statement tree, then lowered: nested sync calls become messages plus
//! activations, returns become dotted replies and control blocks become
//! sequence frames. Drawing is done by [`crate::diagram::sequence::render`].

use crate::config::MermaidConfig;
use crate::diagram::sequence::{
    self, BlockKind, BoxGroup, Event, Message, MsgHead, NotePlacement, Participant,
    ParticipantKind, SequenceDiagram,
};
use crate::error::{Error, Result};
use crate::scene::{DiagramKind, Scene};
use crate::theme::Palette;
use std::collections::HashMap;

/// One ZenUML statement with its optional `{ }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub line: usize,
    pub head: String,
    pub block: Option<Vec<Stmt>>,
}

/// Split a ZenUML body (after the `zenuml` keyword) into a statement tree.
pub fn parse_tree(body: &str) -> Result<Vec<Stmt>> {
    // (head, line, children) for each open block; index 0 is the root.
    let mut stack: Vec<(String, usize, Vec<Stmt>)> = vec![(String::new(), 0, Vec::new())];
    let mut cur = String::new();
    let mut cur_line = 0usize;
    let mut header_seen = false;
    for (i, raw) in body.lines().enumerate() {
        let line = i + 1;
        if !header_seen {
            let t = raw.trim();
            if t.is_empty() {
                continue;
            }
            let rest = t
                .strip_prefix("zenuml")
                .ok_or_else(|| Error::parse(line, "Expecting 'zenuml'"))?;
            header_seen = true;
            if !rest.trim().is_empty() {
                return Err(Error::parse(line, "Unexpected text after 'zenuml'"));
            }
            continue;
        }
        let chars: Vec<char> = raw.chars().collect();
        let mut depth = 0i32;
        let mut in_quote = false;
        let mut j = 0;
        let flush =
            |cur: &mut String, cur_line: usize, stack: &mut Vec<(String, usize, Vec<Stmt>)>| {
                let t = cur.trim();
                if !t.is_empty() {
                    let head = t.to_string();
                    if let Some(top) = stack.last_mut() {
                        top.2.push(Stmt {
                            line: cur_line,
                            head,
                            block: None,
                        });
                    }
                }
                cur.clear();
            };
        while j < chars.len() {
            let c = chars[j];
            if cur.trim().is_empty() {
                cur_line = line;
            }
            if in_quote {
                cur.push(c);
                if c == '"' {
                    in_quote = false;
                }
                j += 1;
                continue;
            }
            match c {
                '"' => {
                    in_quote = true;
                    cur.push(c);
                }
                '/' if chars.get(j + 1) == Some(&'/') => {
                    flush(&mut cur, cur_line, &mut stack);
                    let text: String = chars[j + 2..].iter().collect();
                    if let Some(top) = stack.last_mut() {
                        top.2.push(Stmt {
                            line,
                            head: format!("//{}", text.trim()),
                            block: None,
                        });
                    }
                    break;
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth -= 1;
                    cur.push(c);
                }
                ';' if depth == 0 => flush(&mut cur, cur_line, &mut stack),
                '{' if depth == 0 => {
                    let head = cur.trim().to_string();
                    cur.clear();
                    stack.push((head, cur_line, Vec::new()));
                }
                '}' if depth == 0 => {
                    flush(&mut cur, cur_line, &mut stack);
                    if stack.len() < 2 {
                        return Err(Error::parse(line, "Unmatched '}'"));
                    }
                    let (head, l, children) = stack.pop().unwrap_or_default();
                    if let Some(top) = stack.last_mut() {
                        top.2.push(Stmt {
                            line: l,
                            head,
                            block: Some(children),
                        });
                    }
                }
                _ => cur.push(c),
            }
            j += 1;
        }
        if in_quote {
            return Err(Error::parse(line, "Unterminated string"));
        }
        if depth != 0 {
            return Err(Error::parse(line, "Unbalanced parentheses"));
        }
        flush(&mut cur, cur_line, &mut stack);
    }
    if !header_seen {
        return Err(Error::parse(1, "Expecting 'zenuml'"));
    }
    if stack.len() > 1 {
        let l = stack.last().map_or(1, |s| s.1);
        return Err(Error::parse(l, "Missing '}' to close this block"));
    }
    Ok(stack.pop().map(|s| s.2).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Lowering
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Lower {
    title: Option<String>,
    participants: Vec<Participant>,
    index: HashMap<String, usize>,
    boxes: Vec<BoxGroup>,
    events: Vec<Event>,
    starter: Option<usize>,
    implicit_starter: Option<usize>,
    comment: Option<String>,
}

/// Where a statement list runs: the participant whose method body it is,
/// and who called it (target of `return`).
#[derive(Clone, Copy)]
struct Ctx {
    me: Option<usize>,
    caller: Option<usize>,
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '-')
}

/// Index of the first `needle` at parenthesis depth 0 and outside quotes.
fn find_top(s: &str, needle: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_quote = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            '(' if !in_quote => depth += 1,
            ')' if !in_quote => depth -= 1,
            _ if !in_quote && depth == 0 && s[i..].starts_with(needle) => return Some(i),
            _ => {}
        }
    }
    None
}

/// The `=` of an assignment (not `==`, `!=`, `<=`, `>=`).
fn find_assign(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_quote = false;
    let b = s.as_bytes();
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            '(' if !in_quote => depth += 1,
            ')' if !in_quote => depth -= 1,
            '=' if !in_quote && depth == 0 => {
                let prev = if i > 0 { b[i - 1] } else { b' ' };
                let next = b.get(i + 1).copied().unwrap_or(b' ');
                if !matches!(prev, b'=' | b'!' | b'<' | b'>') && next != b'=' {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `(cond)` → `cond`.
fn paren_label(s: &str) -> String {
    let s = s.trim();
    match (s.find('('), s.rfind(')')) {
        (Some(a), Some(b)) if b > a => s[a + 1..b].trim().to_string(),
        _ => s.to_string(),
    }
}

fn keyword(head: &str) -> &str {
    let end = head
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(head.len());
    &head[..end]
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

impl Lower {
    fn participant(&mut self, name: &str) -> usize {
        let id = unquote(name).to_string();
        if let Some(&i) = self.index.get(&id) {
            return i;
        }
        self.participants.push(Participant {
            id: id.clone(),
            label: id.clone(),
            kind: ParticipantKind::Participant,
        });
        let i = self.participants.len() - 1;
        self.index.insert(id, i);
        i
    }

    fn starter(&mut self) -> usize {
        if let Some(s) = self.starter.or(self.implicit_starter) {
            return s;
        }
        self.participants.push(Participant {
            id: "_starter".into(),
            label: String::new(),
            kind: ParticipantKind::Actor,
        });
        let i = self.participants.len() - 1;
        self.implicit_starter = Some(i);
        i
    }

    /// Who makes a call in this context: the current participant, else the starter.
    fn caller_of(&mut self, ctx: Ctx) -> usize {
        match ctx.me {
            Some(me) => me,
            None => self.starter(),
        }
    }

    fn note_for(&mut self, a: usize, b: usize) {
        if let Some(text) = self.comment.take() {
            self.events.push(Event::Note {
                placement: if a == b {
                    NotePlacement::Over(a, a)
                } else {
                    NotePlacement::Over(a.min(b), a.max(b))
                },
                text,
            });
        }
    }

    fn message(&mut self, from: usize, to: usize, text: &str, dotted: bool, head: MsgHead) {
        self.note_for(from, to);
        self.events.push(Event::Message(Message {
            from,
            to,
            text: text.trim().to_string(),
            dotted,
            head,
            bidirectional: false,
            activation: None,
        }));
    }

    /// Declaration: `[@Annot] [<<stereo>>] Name ["as" Label] [#color]`.
    fn declaration(&mut self, head: &str, line: usize) -> Result<Option<usize>> {
        let mut rest = head.trim();
        let mut kind = ParticipantKind::Participant;
        let mut stereo = None;
        if let Some(r) = rest.strip_prefix('@') {
            let end = r
                .find(|c: char| c.is_whitespace())
                .ok_or_else(|| Error::parse(line, format!("Expected a name after '@{r}'")))?;
            if r[..end].eq_ignore_ascii_case("actor") {
                kind = ParticipantKind::Actor;
            }
            rest = r[end..].trim();
        }
        if let Some(r) = rest.strip_prefix("<<") {
            let end = r
                .find(">>")
                .ok_or_else(|| Error::parse(line, "Unterminated '<<' stereotype"))?;
            stereo = Some(r[..end].trim().to_string());
            rest = r[end + 2..].trim();
        }
        if let Some(p) = rest.find(" #") {
            rest = rest[..p].trim();
        } else if rest.starts_with('#') {
            return Ok(None);
        }
        let (name, label) = match rest.find(" as ") {
            Some(p) => (rest[..p].trim(), Some(unquote(&rest[p + 4..]).to_string())),
            None => (rest, None),
        };
        let quoted = name.starts_with('"') && name.ends_with('"') && name.len() >= 2;
        if !(quoted || is_ident(name)) {
            return Ok(None);
        }
        let i = self.participant(name);
        let p = &mut self.participants[i];
        if let Some(l) = label {
            p.label = l;
        }
        if let Some(s) = stereo {
            p.label = format!("\u{ab}{s}\u{bb}<br>{}", p.label);
        }
        p.kind = kind;
        Ok(Some(i))
    }

    fn block_events(&mut self, block: &Option<Vec<Stmt>>, ctx: Ctx) -> Result<bool> {
        match block {
            Some(b) => self.stmts(b, ctx),
            None => Ok(false),
        }
    }

    /// Lower a statement list. Returns whether an explicit `return` occurred.
    fn stmts(&mut self, list: &[Stmt], ctx: Ctx) -> Result<bool> {
        let mut returned = false;
        let mut i = 0;
        while i < list.len() {
            let s = &list[i];
            let head = s.head.trim();
            let kw = keyword(head);
            i += 1;
            if let Some(c) = head.strip_prefix("//") {
                self.comment = Some(c.trim().to_string()).filter(|c| !c.is_empty());
                continue;
            }
            if kw == "title" && s.block.is_none() {
                self.title = Some(head[5..].trim().to_string());
                continue;
            }
            if let Some(r) = head.strip_prefix("@Starter") {
                let name = paren_label(r);
                if !is_ident(unquote(&name)) && !name.starts_with('"') {
                    return Err(Error::parse(s.line, "Expected '@Starter(Name)'"));
                }
                self.starter = Some(self.participant(&name));
                continue;
            }
            if let Some(r) = head
                .strip_prefix("@return")
                .or_else(|| head.strip_prefix("@reply"))
            {
                let (from, to, text) = self.parse_arrow(r, s.line)?;
                let text =
                    text.ok_or_else(|| Error::parse(s.line, "Expected '@return A->B: message'"))?;
                self.message(from, to, &text, true, MsgHead::Async);
                returned = true;
                continue;
            }
            match kw {
                "if" => {
                    self.comment = None;
                    self.events.push(Event::BlockStart {
                        kind: BlockKind::Alt,
                        label: paren_label(&head[2..]),
                    });
                    self.need_block(s)?;
                    returned |= self.block_events(&s.block, ctx)?;
                    while i < list.len() && keyword(list[i].head.trim()) == "else" {
                        let e = &list[i];
                        let rest = e.head.trim()[4..].trim();
                        let label = match rest.strip_prefix("if") {
                            Some(c) => paren_label(c),
                            None => "else".to_string(),
                        };
                        self.events.push(Event::BlockSection { label });
                        self.need_block(e)?;
                        returned |= self.block_events(&e.block, ctx)?;
                        i += 1;
                    }
                    self.events.push(Event::BlockEnd);
                    continue;
                }
                "else" => return Err(Error::parse(s.line, "'else' without a matching 'if'")),
                "catch" | "finally" => {
                    return Err(Error::parse(
                        s.line,
                        format!("'{kw}' without a matching 'try'"),
                    ));
                }
                "while" | "for" | "forEach" | "foreach" | "loop" => {
                    self.frame(s, BlockKind::Loop, paren_label(&head[kw.len()..]), ctx)?;
                    continue;
                }
                "opt" => {
                    self.frame(s, BlockKind::Opt, paren_label(&head[3..]), ctx)?;
                    continue;
                }
                "par" => {
                    self.frame(s, BlockKind::Par, paren_label(&head[3..]), ctx)?;
                    continue;
                }
                "critical" => {
                    self.frame(s, BlockKind::Critical, paren_label(&head[8..]), ctx)?;
                    continue;
                }
                "try" => {
                    self.comment = None;
                    self.need_block(s)?;
                    self.events.push(Event::BlockStart {
                        kind: BlockKind::Critical,
                        label: "try".into(),
                    });
                    returned |= self.block_events(&s.block, ctx)?;
                    while i < list.len()
                        && matches!(keyword(list[i].head.trim()), "catch" | "finally")
                    {
                        let e = &list[i];
                        self.need_block(e)?;
                        self.events.push(Event::BlockSection {
                            label: e.head.trim().to_string(),
                        });
                        returned |= self.block_events(&e.block, ctx)?;
                        i += 1;
                    }
                    self.events.push(Event::BlockEnd);
                    continue;
                }
                "group" => {
                    let label = unquote(head[5..].trim()).to_string();
                    let mut members = Vec::new();
                    for m in s.block.iter().flatten() {
                        if m.head.starts_with("//") {
                            continue;
                        }
                        match self.declaration(&m.head, m.line)? {
                            Some(p) => members.push(p),
                            None => {
                                return Err(Error::parse(
                                    m.line,
                                    "Only participants may be declared inside 'group'",
                                ));
                            }
                        }
                    }
                    self.boxes.push(BoxGroup {
                        label,
                        color: None,
                        members,
                    });
                    continue;
                }
                "return" if head.len() == 6 || head[6..].starts_with(char::is_whitespace) => {
                    if let (Some(me), Some(caller)) = (ctx.me, ctx.caller) {
                        self.message(me, caller, head[6..].trim(), true, MsgHead::Async);
                    }
                    returned = true;
                    continue;
                }
                _ => {}
            }
            // Messages.
            if find_top(head, "->").is_some() {
                let (from, to, text) = self.parse_arrow(head, s.line)?;
                match text {
                    Some(t) => {
                        if s.block.is_some() {
                            return Err(Error::parse(
                                s.line,
                                "An async message cannot have a block",
                            ));
                        }
                        self.message(from, to, &t, false, MsgHead::Async);
                    }
                    None => {
                        // `A->B.method()`: sync call with an explicit caller.
                        let arrow = find_top(head, "->").unwrap_or(0);
                        let call = head[arrow + 2..].trim();
                        let dot = find_top(call, ".").ok_or_else(|| {
                            Error::parse(s.line, "Expected 'A->B: message' or 'A->B.method()'")
                        })?;
                        self.call(from, to, &call[dot + 1..], None, s)?;
                    }
                }
                continue;
            }
            if head.starts_with('@') {
                if self.declaration(head, s.line)?.is_none() {
                    return Err(Error::parse(
                        s.line,
                        format!("Invalid participant '{head}'"),
                    ));
                }
                continue;
            }
            let (assign, rhs) = match find_assign(head) {
                Some(eq) => {
                    let lhs = head[..eq].trim();
                    let var = lhs.split_whitespace().last().unwrap_or("").to_string();
                    if var.is_empty() {
                        return Err(Error::parse(s.line, "Missing variable before '='"));
                    }
                    (Some(var), head[eq + 1..].trim())
                }
                None => (None, head),
            };
            if let Some(r) = rhs.strip_prefix("new ") {
                let r = r.trim();
                let name_end = r.find('(').unwrap_or(r.len());
                let name = r[..name_end].trim();
                if !is_ident(name) {
                    return Err(Error::parse(s.line, format!("Invalid class name '{name}'")));
                }
                let target = self.participant(name);
                let args = &r[name_end..];
                let text = if args.trim().is_empty() || args.trim() == "()" {
                    "\u{ab}create\u{bb}".to_string()
                } else {
                    format!("\u{ab}create\u{bb}{}", args.trim())
                };
                let caller = self.caller_of(ctx);
                self.message(caller, target, &text, false, MsgHead::Filled);
                if s.block.is_some() {
                    self.events.push(Event::Activate(target));
                    self.block_events(
                        &s.block,
                        Ctx {
                            me: Some(target),
                            caller: Some(caller),
                        },
                    )?;
                    self.events.push(Event::Deactivate(target));
                }
                continue;
            }
            let paren = rhs.find('(').unwrap_or(rhs.len());
            match find_top(rhs, ".").filter(|&d| d < paren) {
                Some(dot) => {
                    let target_name = rhs[..dot].trim();
                    let quoted = target_name.starts_with('"') && target_name.ends_with('"');
                    if !(quoted || is_ident(target_name)) {
                        return Err(Error::parse(
                            s.line,
                            format!("Invalid participant '{target_name}'"),
                        ));
                    }
                    let target = self.participant(target_name);
                    let caller = self.caller_of(ctx);
                    self.call(caller, target, &rhs[dot + 1..], assign, s)?;
                }
                None if paren < rhs.len() && is_ident(rhs[..paren].trim()) => {
                    // `method()` — a self call on the current participant.
                    let caller = self.caller_of(ctx);
                    self.call(caller, caller, rhs, assign, s)?;
                }
                None if assign.is_none() && s.block.is_none() => {
                    if self.declaration(head, s.line)?.is_none() {
                        return Err(Error::parse(
                            s.line,
                            format!("Unrecognised statement '{head}'"),
                        ));
                    }
                }
                None => {
                    return Err(Error::parse(
                        s.line,
                        format!("Unrecognised statement '{head}'"),
                    ));
                }
            }
        }
        Ok(returned)
    }

    fn need_block(&self, s: &Stmt) -> Result<()> {
        if s.block.is_none() {
            return Err(Error::parse(
                s.line,
                format!("Expected '{{' after '{}'", s.head.trim()),
            ));
        }
        Ok(())
    }

    fn frame(&mut self, s: &Stmt, kind: BlockKind, label: String, ctx: Ctx) -> Result<bool> {
        self.need_block(s)?;
        self.comment = None;
        self.events.push(Event::BlockStart { kind, label });
        let r = self.block_events(&s.block, ctx)?;
        self.events.push(Event::BlockEnd);
        Ok(r)
    }

    /// A sync call `from -> to.method` with optional block and assignment.
    fn call(
        &mut self,
        from: usize,
        to: usize,
        method: &str,
        assign: Option<String>,
        s: &Stmt,
    ) -> Result<()> {
        let method = method.trim();
        if method.is_empty() {
            return Err(Error::parse(s.line, "Missing method name"));
        }
        self.message(from, to, method, false, MsgHead::Filled);
        if s.block.is_none() && assign.is_none() {
            // A leaf call: no activation bar, like ZenUML's compact rendering.
            return Ok(());
        }
        self.events.push(Event::Activate(to));
        let returned = self.block_events(
            &s.block,
            Ctx {
                me: Some(to),
                caller: Some(from),
            },
        )?;
        if let (Some(var), false) = (assign, returned) {
            self.message(to, from, &var, true, MsgHead::Async);
        }
        self.events.push(Event::Deactivate(to));
        Ok(())
    }

    /// `A->B: text` → (A, B, Some(text)); `A->B.m()` → (A, B, None).
    fn parse_arrow(&mut self, s: &str, line: usize) -> Result<(usize, usize, Option<String>)> {
        let s = s.trim();
        let arrow = find_top(s, "->").ok_or_else(|| Error::parse(line, "Expected '->'"))?;
        let from = s[..arrow].trim();
        let rest = s[arrow + 2..].trim();
        let (to, text) = match rest.find(':') {
            Some(c) => (rest[..c].trim(), Some(rest[c + 1..].trim().to_string())),
            None => {
                let dot = find_top(rest, ".").unwrap_or(rest.len());
                (rest[..dot].trim(), None)
            }
        };
        for n in [from, to] {
            let quoted = n.starts_with('"') && n.ends_with('"') && n.len() >= 2;
            if !(quoted || is_ident(n)) {
                return Err(Error::parse(line, format!("Invalid participant '{n}'")));
            }
        }
        let a = self.participant(from);
        let b = self.participant(to);
        Ok((a, b, text))
    }

    /// Move the implicit starter (if any) to the front.
    fn finish(mut self) -> SequenceDiagram {
        if let Some(k) = self.implicit_starter.filter(|&k| k > 0) {
            let map = |i: usize| match i.cmp(&k) {
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Less => i + 1,
                std::cmp::Ordering::Greater => i,
            };
            let p = self.participants.remove(k);
            self.participants.insert(0, p);
            for ev in &mut self.events {
                match ev {
                    Event::Message(m) => {
                        m.from = map(m.from);
                        m.to = map(m.to);
                    }
                    Event::Note { placement, .. } => {
                        *placement = match *placement {
                            NotePlacement::LeftOf(a) => NotePlacement::LeftOf(map(a)),
                            NotePlacement::RightOf(a) => NotePlacement::RightOf(map(a)),
                            NotePlacement::Over(a, b) => NotePlacement::Over(map(a), map(b)),
                        };
                    }
                    Event::Activate(a) | Event::Deactivate(a) => *a = map(*a),
                    _ => {}
                }
            }
            for b in &mut self.boxes {
                for m in &mut b.members {
                    *m = map(*m);
                }
            }
        }
        SequenceDiagram {
            title: self.title,
            participants: self.participants,
            boxes: self.boxes,
            events: self.events,
        }
    }
}

/// Parse a ZenUML body and lower it to a sequence diagram.
pub fn parse(body: &str, title: Option<String>) -> Result<SequenceDiagram> {
    let tree = parse_tree(body)?;
    let mut l = Lower {
        title,
        ..Lower::default()
    };
    l.stmts(
        &tree,
        Ctx {
            me: None,
            caller: None,
        },
    )?;
    Ok(l.finish())
}

/// Parse and render a `zenuml` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    let mut scene = sequence::render(&d, &cfg.sequence, pal);
    scene.kind = DiagramKind::ZenUml;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs(d: &SequenceDiagram) -> Vec<(String, String, String, bool)> {
        d.events
            .iter()
            .filter_map(|e| match e {
                Event::Message(m) => Some((
                    d.participants[m.from].id.clone(),
                    d.participants[m.to].id.clone(),
                    m.text.clone(),
                    m.dotted,
                )),
                _ => None,
            })
            .collect()
    }

    fn err_line(src: &str) -> usize {
        match parse(src, None) {
            Err(Error::Parse { line, .. }) => line,
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn tree_nests_blocks() {
        let t =
            parse_tree("zenuml\nA.m() {\n  B.n(x, y) { return 1 }\n}\nC->D: hi // note\n").unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].head, "A.m()");
        let inner = t[0].block.as_ref().unwrap();
        assert_eq!(inner[0].head, "B.n(x, y)");
        assert_eq!(inner[0].block.as_ref().unwrap()[0].head, "return 1");
        assert_eq!(t[2].head, "//note");
    }

    #[test]
    fn async_messages_and_declarations() {
        let d = parse(
            "zenuml\n title Demo\n @Actor Alice\n @Database Bob\n J as John\n Alice->J: Hello\n J->Alice: Great!",
            None,
        )
        .unwrap();
        assert_eq!(d.title.as_deref(), Some("Demo"));
        assert_eq!(d.participants.len(), 3);
        assert_eq!(d.participants[0].kind, ParticipantKind::Actor);
        assert_eq!(d.participants[2].label, "John");
        assert_eq!(
            msgs(&d),
            vec![
                ("Alice".into(), "J".into(), "Hello".into(), false),
                ("J".into(), "Alice".into(), "Great!".into(), false),
            ]
        );
    }

    #[test]
    fn sync_calls_lower_to_activations_and_replies() {
        let d = parse(
            "zenuml\n@Starter(Client)\nresult = A.process(order) {\n  B.check()\n  x = C.load(id) {\n    return row\n  }\n  A.selfCall()\n}\n",
            None,
        )
        .unwrap();
        assert_eq!(
            msgs(&d),
            vec![
                ("Client".into(), "A".into(), "process(order)".into(), false),
                ("A".into(), "B".into(), "check()".into(), false),
                ("A".into(), "C".into(), "load(id)".into(), false),
                ("C".into(), "A".into(), "row".into(), true),
                ("A".into(), "A".into(), "selfCall()".into(), false),
                ("A".into(), "Client".into(), "result".into(), true),
            ]
        );
        let acts = d
            .events
            .iter()
            .filter(|e| matches!(e, Event::Activate(_)))
            .count();
        let deacts = d
            .events
            .iter()
            .filter(|e| matches!(e, Event::Deactivate(_)))
            .count();
        assert_eq!((acts, deacts), (2, 2));
    }

    #[test]
    fn implicit_starter_goes_first() {
        let d = parse("zenuml\nBob\nBob.hello()", None).unwrap();
        assert_eq!(d.participants[0].id, "_starter");
        assert_eq!(d.participants[1].id, "Bob");
        assert_eq!(msgs(&d)[0].0, "_starter");
    }

    #[test]
    fn control_blocks_map_to_frames() {
        let d = parse(
            "zenuml\nA.m() {\n if(x > 1) {\n  B.a()\n } else if (y) {\n  B.b()\n } else {\n  B.c()\n }\n while(more) { B.d() }\n opt { B.e() }\n par { B.f()\n C.g() }\n try { B.h() } catch(e) { B.i() } finally { B.j() }\n}",
            None,
        )
        .unwrap();
        let frames: Vec<String> = d
            .events
            .iter()
            .filter_map(|e| match e {
                Event::BlockStart { kind, label } => Some(format!("{kind:?}:{label}")),
                Event::BlockSection { label } => Some(format!("|{label}")),
                _ => None,
            })
            .collect();
        assert_eq!(
            frames,
            vec![
                "Alt:x > 1",
                "|y",
                "|else",
                "Loop:more",
                "Opt:",
                "Par:",
                "Critical:try",
                "|catch(e)",
                "|finally"
            ]
        );
        let ends = d
            .events
            .iter()
            .filter(|e| matches!(e, Event::BlockEnd))
            .count();
        assert_eq!(ends, 5);
    }

    #[test]
    fn groups_comments_creation_and_explicit_return() {
        let d = parse(
            "zenuml\ngroup Backend {\n  @Lambda Svc\n  Db\n}\n// kick off\nSvc.run() {\n  o = new Order(1)\n  @return Svc->Db: done\n}",
            None,
        )
        .unwrap();
        assert_eq!(d.boxes.len(), 1);
        assert_eq!(d.boxes[0].label, "Backend");
        assert_eq!(d.boxes[0].members.len(), 2);
        assert!(
            d.events
                .iter()
                .any(|e| matches!(e, Event::Note { text, .. } if text == "kick off"))
        );
        let m = msgs(&d);
        assert_eq!(m[1].1, "Order");
        assert!(m[1].2.contains("create"));
        assert_eq!(m[2], ("Svc".into(), "Db".into(), "done".into(), true));
    }

    #[test]
    fn errors_have_line_numbers() {
        assert_eq!(err_line("zenuml\nA.m() {\n  B.n()\n"), 2);
        assert_eq!(err_line("zenuml\nA->B: x\n}\n"), 3);
        assert_eq!(err_line("zenuml\n\nA.m(\n"), 3);
        assert_eq!(err_line("zenuml\nA->B: x\nelse { }\n"), 3);
        assert_eq!(err_line("zenuml\nA->B: x\n\nwhile(x)\n"), 4);
        assert_eq!(err_line("zenuml\nfoo bar baz\n"), 2);
        assert_eq!(err_line("sequenceDiagram\n"), 1);
    }

    #[test]
    fn renders_as_zenuml_scene() {
        let cfg = MermaidConfig::default();
        let s = render("zenuml\nA->B: hi\nB.x()", None, &cfg, &cfg.palette()).unwrap();
        assert_eq!(s.kind, DiagramKind::ZenUml);
        assert!(s.element_count() > 5);
    }
}
