// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! HTML bodies: identify mail-client quote, forward, and signature containers,
//! apply the two-signal removal / hold rules, and render the remainder cleanly.

use std::cell::RefCell;
use std::collections::BTreeSet;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

use crate::extract::{ForwardedBlock, Held, Removal, parse_forwarded_headers_and_body};
use crate::text::{coverage, is_attribution};

/// Replaces a removed container in rendered HTML text; dropped afterwards.
pub const SENTINEL: &str = "QQSOLSTONEEMAILREMOVEDQQ";

#[derive(Clone, Debug)]
enum Node {
    El(El),
    Text(String),
}

#[derive(Clone, Debug, Default)]
struct El {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    raw: bool,
}

impl El {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }

    fn has_class(&self, c: &str) -> bool {
        self.attr("class")
            .is_some_and(|v| v.split_whitespace().any(|x| x.eq_ignore_ascii_case(c)))
    }

    fn class_contains(&self, c: &str) -> bool {
        self.attr("class")
            .is_some_and(|v| v.to_lowercase().contains(c))
    }
}

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];
const BLOCK: &[&str] = &[
    "div",
    "p",
    "br",
    "li",
    "tr",
    "blockquote",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "table",
    "ul",
    "ol",
    "pre",
    "hr",
];

#[derive(Default)]
struct Sink {
    toks: RefCell<Vec<Token>>,
}

impl TokenSink for Sink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        use html5ever::tokenizer::states::RawKind;
        let raw = match &token {
            Token::TagToken(t) if t.kind == TagKind::StartTag => match &*t.name {
                "style" => Some(RawKind::Rawtext),
                "script" => Some(RawKind::ScriptData),
                "title" | "textarea" => Some(RawKind::Rcdata),
                _ => None,
            },
            _ => None,
        };
        self.toks.borrow_mut().push(token);
        match raw {
            Some(k) => TokenSinkResult::RawData(k),
            None => TokenSinkResult::Continue,
        }
    }
}

fn parse(html: &str) -> El {
    let tok = Tokenizer::new(Sink::default(), TokenizerOpts::default());
    let q = BufferQueue::default();
    q.push_back(StrTendril::from_slice(html));
    let _ = tok.feed(&q);
    tok.end();
    let toks = tok.sink.toks.take();
    let mut stack = vec![El {
        name: "#root".into(),
        ..El::default()
    }];
    for t in toks {
        match t {
            Token::TagToken(tag) => {
                let name = tag.name.to_string().to_ascii_lowercase();
                if tag.kind == TagKind::StartTag {
                    let el = El {
                        raw: matches!(name.as_str(), "style" | "script" | "title"),
                        attrs: tag
                            .attrs
                            .iter()
                            .map(|a| {
                                (
                                    a.name.local.to_string().to_ascii_lowercase(),
                                    a.value.to_string(),
                                )
                            })
                            .collect(),
                        name: name.clone(),
                        children: Vec::new(),
                    };
                    if VOID.contains(&name.as_str()) || tag.self_closing {
                        stack.last_mut().unwrap().children.push(Node::El(el));
                    } else {
                        stack.push(el);
                    }
                } else if let Some(pos) = stack.iter().rposition(|e| e.name == name) {
                    if pos == 0 {
                        continue;
                    }
                    while stack.len() > pos {
                        let done = stack.pop().unwrap();
                        stack.last_mut().unwrap().children.push(Node::El(done));
                    }
                }
            }
            Token::CharacterTokens(s) => {
                stack
                    .last_mut()
                    .unwrap()
                    .children
                    .push(Node::Text(s.to_string()));
            }
            _ => {}
        }
    }
    while stack.len() > 1 {
        let done = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(Node::El(done));
    }
    stack.pop().unwrap()
}

fn text_of(node: &Node, out: &mut String) {
    match node {
        Node::Text(t) => out.push_str(t),
        Node::El(e) => {
            if matches!(e.name.as_str(), "style" | "script" | "title" | "head") {
                return;
            }
            let block = BLOCK.contains(&e.name.as_str());
            if block && !out.ends_with('\n') {
                out.push('\n');
            }
            for c in &e.children {
                text_of(c, out);
            }
            if block && !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
}

fn el_text(e: &El) -> String {
    let mut s = String::new();
    text_of(&Node::El(e.clone()), &mut s);
    s
}

fn last_line(nodes: &[Node]) -> Option<String> {
    let mut s = String::new();
    for n in nodes {
        text_of(n, &mut s);
    }
    s.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_owned)
}

fn first_line(e: &El) -> Option<String> {
    el_text(e)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

fn is_quote(e: &El, in_gmail_quote: bool) -> bool {
    (e.name == "blockquote"
        && (e.class_contains("gmail_quote")
            || e.attr("type")
                .is_some_and(|t| t.eq_ignore_ascii_case("cite"))
            || e.class_contains("protonmail_quote")
            || in_gmail_quote))
        || (e.name == "div"
            && (e.has_class("yahoo_quoted") || e.class_contains("protonmail_quote")))
}

fn is_attr_box(e: &El) -> bool {
    e.has_class("gmail_attr") || e.has_class("moz-cite-prefix")
}

fn is_signature(e: &El) -> bool {
    e.class_contains("gmail_signature")
        || e.attr("data-smartmail") == Some("gmail_signature")
        || e.has_class("moz-signature")
}

fn is_outlook_tail(e: &El) -> bool {
    matches!(e.attr("id"), Some("divRplyFwdMsg" | "appendonsend"))
}

fn is_gmail_forward(e: &El) -> bool {
    e.name == "div"
        && e.has_class("gmail_quote")
        && e.children.iter().any(|c| match c {
            Node::El(a) => {
                is_attr_box(a) && el_text(a).to_lowercase().contains("forwarded message")
            }
            _ => false,
        })
}

#[derive(Default)]
pub(crate) struct ImageRegions {
    pub authored: BTreeSet<String>,
    pub omitted: BTreeSet<String>,
    pub uncertain: BTreeSet<String>,
}

fn image_id(e: &El) -> Option<String> {
    (e.name == "img")
        .then(|| e.attr("src").and_then(cid_url_id))
        .flatten()
}

fn record_omitted(node: &Node, ids: &mut BTreeSet<String>) {
    if let Node::El(e) = node {
        if let Some(id) = image_id(e) {
            ids.insert(id);
        }
        for n in &e.children {
            record_omitted(n, ids);
        }
    }
}

struct Ctx<'a> {
    parent: Option<&'a str>,
    removed: Vec<Removal>,
    held: Vec<Held>,
    quote_text: String,
    sig_text: String,
    novel_text: String,
    forwarded: Vec<ForwardedBlock>,
    image_regions: ImageRegions,
    in_container: usize,
    cut: bool,
}

fn sentinel() -> Node {
    Node::El(El {
        name: "p".into(),
        children: vec![Node::Text(SENTINEL.into())],
        ..El::default()
    })
}

fn transform(nodes: &[Node], ctx: &mut Ctx<'_>, in_gq: bool) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for n in nodes {
        if ctx.cut {
            record_omitted(n, &mut ctx.image_regions.omitted);
            continue;
        }
        let Node::El(e) = n else {
            if let (Node::Text(t), 0) = (n, ctx.in_container) {
                ctx.novel_text.push_str(t);
                ctx.novel_text.push('\n');
            }
            out.push(n.clone());
            continue;
        };
        if matches!(e.name.as_str(), "script" | "title") {
            continue;
        }
        if is_outlook_tail(e) {
            let mut text = el_text(e);
            text.push('\n');
            ctx.quote_text.push_str(&text);
            ctx.removed.push(Removal {
                kind: "quote".into(),
                signals: vec!["outlook_reply_box".into(), "header_block".into()],
                chars: text.chars().count(),
            });
            out.push(sentinel());
            ctx.cut = true;
            record_omitted(n, &mut ctx.image_regions.omitted);
            continue;
        }
        if is_signature(e) {
            record_omitted(n, &mut ctx.image_regions.omitted);
            let text = el_text(e);
            ctx.sig_text.push_str(&text);
            ctx.sig_text.push('\n');
            ctx.removed.push(Removal {
                kind: "signature".into(),
                signals: vec!["signature_container".into()],
                chars: text.chars().count(),
            });
            out.push(sentinel());
            continue;
        }
        if is_gmail_forward(e) {
            record_omitted(n, &mut ctx.image_regions.omitted);
            let mut raw_html = String::new();
            serialize(&e.children, false, &mut raw_html);
            let rendered_fwd = render(&raw_html);
            ctx.forwarded
                .push(parse_forwarded_headers_and_body(&rendered_fwd));
            ctx.quote_text.push_str(&rendered_fwd);
            ctx.removed.push(Removal {
                kind: "forward".into(),
                signals: vec!["forward_marker".into(), "gmail_attr".into()],
                chars: rendered_fwd.chars().count(),
            });
            out.push(sentinel());
            continue;
        }
        if is_quote(e, in_gq) {
            let text = el_text(e);
            ctx.quote_text.push_str(&text);
            ctx.quote_text.push('\n');
            let inside = first_line(e).is_some_and(|l| is_attribution(&l))
                || e.children
                    .iter()
                    .any(|c| matches!(c, Node::El(a) if is_attr_box(a)));
            let before = last_line(&out).is_some_and(|l| is_attribution(&l));
            let parent = ctx
                .parent
                .is_some_and(|p| coverage(&text, p) >= crate::extract::PARENT_MATCH);
            let disagrees = ctx.parent.is_some() && !parent;
            let mut signals = vec!["quote_container".to_owned()];
            if inside || before {
                signals.push("attribution".into());
            }
            if parent {
                signals.push("parent_match".into());
            }
            if signals.len() >= 2 && !disagrees {
                record_omitted(n, &mut ctx.image_regions.omitted);
                ctx.removed.push(Removal {
                    kind: "quote".into(),
                    signals,
                    chars: text.chars().count(),
                });
                out.push(sentinel());
            } else {
                ctx.held.push(Held {
                    reason: if disagrees && signals.len() >= 2 {
                        "quote_disagrees_with_parent"
                    } else {
                        "quote_container_only"
                    }
                    .into(),
                    chars: text.chars().count(),
                    text: text.clone(),
                });
                let mut kept = e.clone();
                ctx.in_container += 1;
                kept.children = transform(&e.children, ctx, false);
                ctx.in_container -= 1;
                out.push(Node::El(kept));
            }
            continue;
        }
        if let Some(id) = image_id(e) {
            if ctx.in_container == 0 {
                ctx.image_regions.authored.insert(id);
            } else {
                ctx.image_regions.uncertain.insert(id);
            }
        }
        let mut kept = e.clone();
        let gq = in_gq || (e.name == "div" && e.has_class("gmail_quote"));
        kept.children = transform(&e.children, ctx, gq);
        out.push(Node::El(kept));
    }
    out
}

fn escape(s: &str, attr: bool) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' if attr => o.push_str("&quot;"),
            '\u{a0}' => o.push_str("&nbsp;"),
            _ => o.push(c),
        }
    }
    o
}

fn serialize(nodes: &[Node], raw: bool, out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => {
                if raw {
                    out.push_str(t);
                } else {
                    out.push_str(&escape(t, false));
                }
            }
            Node::El(e) => {
                out.push('<');
                out.push_str(&e.name);
                for (k, v) in &e.attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(&escape(v, true));
                    out.push('"');
                }
                out.push('>');
                if VOID.contains(&e.name.as_str()) {
                    continue;
                }
                serialize(&e.children, e.raw, out);
                out.push_str("</");
                out.push_str(&e.name);
                out.push('>');
            }
        }
    }
}

pub struct HtmlRead {
    pub text: String,
    pub quote_text: String,
    pub sig_text: String,
    pub novel_text: String,
    pub forwarded: Vec<ForwardedBlock>,
    pub removed: Vec<Removal>,
    pub held: Vec<Held>,
}

/// Render HTML to plain text with 3-tier fallback (CSS layout -> raw mode ->
/// html5ever tokenizer text nodes) so malformed tables never produce silent empty output.
#[must_use]
pub fn render(html: &str) -> String {
    let attempt = |raw: bool| {
        std::panic::catch_unwind(|| {
            let c = html2text::config::plain_no_decorate();
            let c = if raw {
                c.raw_mode(true)
            } else {
                c.use_doc_css()
            };
            c.string_from_read(html.as_bytes(), 10_000)
        })
    };
    if let Ok(Ok(s)) = attempt(false)
        && !s.trim().is_empty()
    {
        return s;
    }
    if let Ok(Ok(s)) = attempt(true)
        && !s.trim().is_empty()
    {
        return s;
    }
    let mut s = String::new();
    text_of(&Node::El(parse(html)), &mut s);
    let lines: Vec<String> = s
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let mut out = String::new();
    for l in lines {
        if l.is_empty() && (out.is_empty() || out.ends_with("\n\n")) {
            continue;
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// RFC 2392 uses URL escaping for the message-local Content-ID. Other URLs are
/// never resolved. The ID itself can be case sensitive.
fn cid_url_id(url: &str) -> Option<String> {
    let (scheme, value) = url.trim().split_once(':')?;
    if !scheme.eq_ignore_ascii_case("cid") || value.is_empty() {
        return None;
    }
    String::from_utf8(crate::headers::percent_decoded(value)?).ok()
}

#[must_use]
pub(crate) fn image_regions(html: &str) -> ImageRegions {
    transform_body(html, None).1.image_regions
}

fn transform_body<'a>(html: &str, parent: Option<&'a str>) -> (Vec<Node>, Ctx<'a>) {
    let root = parse(html);
    let mut ctx = Ctx {
        parent,
        removed: Vec::new(),
        held: Vec::new(),
        quote_text: String::new(),
        sig_text: String::new(),
        novel_text: String::new(),
        forwarded: Vec::new(),
        image_regions: ImageRegions::default(),
        in_container: 0,
        cut: false,
    };
    let kept = transform(&root.children, &mut ctx, false);
    (kept, ctx)
}

#[must_use]
pub fn read(html: &str, parent: Option<&str>) -> HtmlRead {
    let (kept, ctx) = transform_body(html, parent);
    let mut out = String::new();
    serialize(&kept, false, &mut out);
    let rendered = render(&out);
    let mut lines: Vec<&str> = Vec::new();
    for line in rendered.lines() {
        if line.trim() == SENTINEL {
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            if lines.last().is_some_and(|l| is_attribution(l)) {
                lines.pop();
            }
            continue;
        }
        lines.push(line.trim_end());
    }
    HtmlRead {
        text: lines.join("\n"),
        quote_text: ctx.quote_text,
        sig_text: ctx.sig_text,
        novel_text: ctx.novel_text,
        forwarded: ctx.forwarded,
        removed: ctx.removed,
        held: ctx.held,
    }
}
