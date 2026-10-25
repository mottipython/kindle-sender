//! Turns tag-soup HTML (a MOBI text blob, an .html file) into well-formed XHTML
//! body fragments, optionally split into several parts at page breaks.

use std::collections::HashMap;

use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use super::epub_writer::esc;

/// Marker the MOBI reader leaves where a page break was: html5ever would nest
/// content inside a self-closed unknown element, a void `<hr>` cannot nest.
pub const PAGEBREAK_CLASS: &str = "ks-pagebreak";

pub enum Hook {
    /// Apply the default attribute policy.
    Pass,
    Drop,
    Set(String, String),
}

pub trait Hooks {
    fn attr(&mut self, tag: &str, name: &str, value: &str) -> Hook;
}

pub struct Parsed {
    pub parts: Vec<String>,
    /// First heading text in each part.
    pub titles: Vec<Option<String>>,
    /// Element id -> index into `parts`.
    pub anchors: HashMap<String, usize>,
}

const ALLOWED_TAGS: &[&str] = &[
    "a", "abbr", "b", "bdo", "big", "blockquote", "br", "caption", "cite", "code", "col", "colgroup", "dd",
    "del", "dfn", "div", "dl", "dt", "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i", "img", "ins", "kbd",
    "li", "ol", "p", "pre", "q", "s", "samp", "small", "span", "strike", "strong", "sub", "sup", "table",
    "tbody", "td", "tfoot", "th", "thead", "tr", "tt", "u", "ul", "var",
];
const VOID_TAGS: &[&str] = &["br", "hr", "img", "col"];
const SKIP_TAGS: &[&str] = &["script", "style", "head", "meta", "link", "title", "noscript", "template"];

struct Ser<'a> {
    hooks: &'a mut dyn Hooks,
    parts: Vec<String>,
    titles: Vec<Option<String>>,
    cur: String,
    cur_title: Option<String>,
    stack: Vec<(String, String)>,
    anchors: HashMap<String, usize>,
    depth: usize,
}

/// Real books never nest this deep; past it, elements are flattened so hostile
/// input cannot overflow the stack.
const MAX_DEPTH: usize = 200;

pub fn tidy(html: &str, hooks: &mut dyn Hooks) -> Parsed {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let mut ser = Ser {
        hooks,
        parts: Vec::new(),
        titles: Vec::new(),
        cur: String::new(),
        cur_title: None,
        stack: Vec::new(),
        anchors: HashMap::new(),
        depth: 0,
    };
    ser.walk(&dom.document);
    ser.finish_part();

    // Drop parts with nothing readable in them and renumber the anchors.
    let mut remap = Vec::new();
    let mut parts = Vec::new();
    let mut titles = Vec::new();
    for (i, p) in ser.parts.into_iter().enumerate() {
        if has_content(&p) {
            remap.push(Some(parts.len()));
            parts.push(p);
            titles.push(ser.titles[i].clone());
        } else {
            remap.push(None);
        }
    }
    let last = parts.len().saturating_sub(1);
    let anchors = ser
        .anchors
        .into_iter()
        .map(|(k, v)| (k, remap.get(v).copied().flatten().unwrap_or(last.min(v))))
        .collect();
    Parsed { parts, titles, anchors }
}

fn has_content(part: &str) -> bool {
    if part.contains("<img") || part.contains("<hr") {
        return true;
    }
    let mut in_tag = false;
    for c in part.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !c.is_whitespace() => return true,
            _ => {}
        }
    }
    false
}

fn text_of(node: &Handle, out: &mut String) {
    // Iterative: the tree can be deeper than the stack allows.
    let mut todo = vec![node.clone()];
    while let Some(n) = todo.pop() {
        match &n.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            _ => todo.extend(n.children.borrow().iter().rev().cloned()),
        }
    }
}

impl Ser<'_> {
    fn finish_part(&mut self) {
        let mut part = std::mem::take(&mut self.cur);
        for (tag, _) in self.stack.iter().rev() {
            part.push_str(&format!("</{}>", tag));
        }
        self.parts.push(part);
        self.titles.push(self.cur_title.take());
    }

    fn page_break(&mut self) {
        self.finish_part();
        for (_, open) in &self.stack {
            self.cur.push_str(open);
        }
    }

    fn walk(&mut self, node: &Handle) {
        match &node.data {
            NodeData::Text { contents } => {
                self.cur.push_str(&esc(&contents.borrow()));
            }
            NodeData::Element { name, attrs, .. } => {
                let tag: &str = &name.local;
                if SKIP_TAGS.contains(&tag) {
                    return;
                }
                if tag == "mbp:pagebreak"
                    || (tag == "hr" && attrs.borrow().iter().any(|a| &*a.name.local == "class" && &*a.value == PAGEBREAK_CLASS))
                {
                    self.page_break();
                    return;
                }
                let tag = if tag == "center" { "div" } else { tag };
                if !ALLOWED_TAGS.contains(&tag) {
                    self.walk_children(node);
                    return;
                }

                let mut out_attrs: Vec<(String, String)> = Vec::new();
                for a in attrs.borrow().iter() {
                    let an: &str = &a.name.local;
                    let value: &str = &a.value;
                    let (n, v) = match self.hooks.attr(tag, an, value) {
                        Hook::Drop => continue,
                        Hook::Set(n, v) => (n, v),
                        Hook::Pass => match (tag, an) {
                            (_, "id") | (_, "class") | (_, "title") => (an.to_string(), value.to_string()),
                            ("a", "name") => ("id".to_string(), value.to_string()),
                            ("a", "href") | ("img", "src") | ("img", "alt") => (an.to_string(), value.to_string()),
                            ("td" | "th", "colspan" | "rowspan") | ("ol", "start") => (an.to_string(), value.to_string()),
                            _ => continue,
                        },
                    };
                    if !out_attrs.iter().any(|(k, _)| *k == n) {
                        out_attrs.push((n, v));
                    }
                }

                if tag == "img" {
                    if !out_attrs.iter().any(|(k, _)| k == "src") {
                        return;
                    }
                    if !out_attrs.iter().any(|(k, _)| k == "alt") {
                        out_attrs.push(("alt".to_string(), String::new()));
                    }
                }
                if tag == "a" && !out_attrs.iter().any(|(k, _)| k == "href" || k == "id") {
                    self.walk_children(node);
                    return;
                }

                for (k, v) in &out_attrs {
                    if k == "id" {
                        self.anchors.insert(v.clone(), self.parts.len());
                    }
                }

                let mut open = format!("<{}", tag);
                for (k, v) in &out_attrs {
                    open.push_str(&format!(" {}=\"{}\"", k, esc(v)));
                }

                if VOID_TAGS.contains(&tag) {
                    open.push_str("/>");
                    self.cur.push_str(&open);
                    return;
                }
                open.push('>');
                self.cur.push_str(&open);

                if self.cur_title.is_none() && matches!(tag, "h1" | "h2" | "h3") {
                    let mut t = String::new();
                    text_of(node, &mut t);
                    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !t.is_empty() {
                        self.cur_title = Some(t);
                    }
                }

                self.stack.push((tag.to_string(), open));
                self.walk_children(node);
                self.stack.pop();
                self.cur.push_str(&format!("</{}>", tag));
            }
            _ => self.walk_children(node),
        }
    }

    fn walk_children(&mut self, node: &Handle) {
        if self.depth >= MAX_DEPTH {
            // Too deep: keep the text, drop the structure.
            let mut t = String::new();
            text_of(node, &mut t);
            self.cur.push_str(&esc(&t));
            return;
        }
        self.depth += 1;
        let kids: Vec<Handle> = node.children.borrow().clone();
        for c in &kids {
            self.walk(c);
        }
        self.depth -= 1;
    }
}
