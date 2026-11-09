// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The dashboard shell's stylesheets and scripts, served as one file each.
// Included by the shell's build script, which writes the bundles, and by the
// shell's tests, which check them, so both read the shell page the same way.
//
// The shell page keeps listing every file it loads, in order; it stays the
// source. The build concatenates them unchanged, with a `/* file: … */`
// banner per segment. Nothing is minified or transpiled. Each script segment
// runs inside its own function, so one that throws doesn't stop the next, and
// every name a file declares at its top level stays a global, read and written
// through the same binding as when the file was its own script.

/// One stylesheet or script tag of the shell page that goes into a bundle.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundledTag {
    /// Byte range to replace in the page: the tag, with its line when the tag
    /// is alone on it.
    pub start: usize,
    pub end: usize,
    /// The served path the tag names.
    pub path: String,
    /// A stylesheet's `media` query, if any.
    pub media: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellPageTags {
    pub styles: Vec<BundledTag>,
    pub scripts: Vec<BundledTag>,
}

/// The served path of each bundle.
#[allow(dead_code)]
pub const SHELL_SCRIPT_BUNDLE: &str = "/static/shell.js";
#[allow(dead_code)]
pub const SHELL_STYLE_BUNDLE: &str = "/static/shell.css";
#[allow(dead_code)]
pub const SHELL_SCRIPT_MAP: &str = "/static/shell.js.map";

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        let before = lower[..at].chars().last();
        if !before.is_some_and(|c| c.is_ascii_whitespace()) {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            // A bare attribute (`defer`).
            let next = lower[from..].chars().next();
            if next.is_none_or(|c| c.is_ascii_whitespace() || c == '>' || c == '/') {
                return Some("");
            }
            continue;
        };
        let rest = rest.trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let value = &rest[1..];
        return value.find(quote).map(|end| &value[..end]);
    }
    None
}

/// Widen a tag's range to its whole line when nothing else is on it.
fn whole_line(page: &str, start: usize, end: usize) -> (usize, usize) {
    let line_start = page[..start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = page[end..].find('\n').map_or(page.len(), |at| end + at + 1);
    let before_blank = page[line_start..start].trim().is_empty();
    let after_blank = page[end..line_end].trim().is_empty();
    if before_blank && after_blank {
        (line_start, line_end)
    } else {
        (start, end)
    }
}

/// Every stylesheet and external script the shell page loads, in order. A
/// page that loads one the bundles can't keep in order is refused.
#[allow(dead_code)]
pub fn shell_page_tags(page: &str) -> Result<ShellPageTags, String> {
    let lower = page.to_ascii_lowercase();
    let mut tags = ShellPageTags::default();
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find('<') {
        let start = cursor + found;
        cursor = start + 1;
        let is_link = lower[start..].starts_with("<link");
        let is_script = lower[start..].starts_with("<script");
        if !is_link && !is_script {
            continue;
        }
        let close = lower[start..]
            .find('>')
            .map(|at| start + at + 1)
            .ok_or_else(|| "the shell page has an unclosed tag".to_owned())?;
        let tag = &page[start..close];
        if is_link {
            let rel = attribute(tag, "rel").unwrap_or_default().to_ascii_lowercase();
            if rel.split_whitespace().any(|word| word == "stylesheet") {
                let href = attribute(tag, "href")
                    .ok_or_else(|| format!("stylesheet tag {tag} has no href"))?;
                if !href.starts_with("/static/") || href.contains(['?', '#']) {
                    return Err(format!(
                        "the shell page loads stylesheet {href}, which isn't a plain shell \
                         file, so it can't go into the bundle in order"
                    ));
                }
                let (start, end) = whole_line(page, start, close);
                tags.styles.push(BundledTag {
                    start,
                    end,
                    path: href.to_owned(),
                    media: attribute(tag, "media").map(str::to_owned),
                });
            }
            cursor = close;
            continue;
        }
        let end_tag = lower[close..]
            .find("</script>")
            .map(|at| close + at)
            .ok_or_else(|| "the shell page has an unclosed script".to_owned())?;
        cursor = end_tag + "</script>".len();
        let Some(src) = attribute(tag, "src") else {
            continue; // inline scripts stay where they are
        };
        if !src.starts_with("/static/")
            || src.contains(['?', '#'])
            || attribute(tag, "defer").is_none()
            || attribute(tag, "async").is_some()
            || attribute(tag, "type").is_some()
            || !page[close..end_tag].trim().is_empty()
        {
            return Err(format!(
                "the shell page loads script {src} in a way the bundle can't keep in order; \
                 every shell script is a plain deferred /static file"
            ));
        }
        let (start, end) = whole_line(page, start, cursor);
        tags.scripts.push(BundledTag {
            start,
            end,
            path: src.to_owned(),
            media: None,
        });
    }
    if tags.styles.is_empty() || tags.scripts.is_empty() {
        return Err("the shell page has no stylesheets or no scripts to bundle".to_owned());
    }
    Ok(tags)
}

// ---------------------------------------------------------------------------
// A lexer for the shell's scripts: enough to know a file is lexically whole
// (every string, comment, template and regular expression ends; every bracket
// closes) and to find the names it declares at its top level. It is not a
// parser; a full parse of every segment runs under Node in the shell's tests.

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Punct(char),
    Value, // a string, number, template or regular expression
}

struct Lexed {
    tok: Tok,
    line: usize,
    /// A line break between the previous token and this one.
    newline_before: bool,
    depth: usize,
}

const REGEX_AFTER_WORDS: &[&str] = &[
    "return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case",
    "do", "else", "yield", "await",
];

/// Whether a `/` starts a regular expression or divides. The lexer fails
/// closed: where the tokens before it can't tell, the build stops and asks for
/// the code to say which (parentheses, or a regex assigned to a name first).
fn regex_allowed(out: &[Lexed], after_control_paren: bool) -> Result<bool, ()> {
    let previous = out.last().map(|l| &l.tok);
    match previous {
        None => Ok(true),
        Some(Tok::Value) => Ok(false),
        Some(Tok::Ident(word)) => Ok(REGEX_AFTER_WORDS.contains(&word.as_str())),
        Some(Tok::Punct(')')) => Ok(after_control_paren),
        Some(Tok::Punct(']')) => Ok(false),
        Some(Tok::Punct('}')) => Err(()),
        Some(Tok::Punct(c @ ('+' | '-')))
            if out.len() >= 2 && out[out.len() - 2].tok == Tok::Punct(*c) =>
        {
            Err(())
        }
        Some(Tok::Punct(_)) => Ok(true),
    }
}

/// Punctuation the shell's scripts use. Anything else stops the build rather
/// than being guessed at.
const KNOWN_PUNCT: &[u8] = b";,<>+-*/%&|^!~?:=.#";

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80
}

fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

fn lex_js(path: &str, source: &str) -> Result<Vec<Lexed>, String> {
    let bytes = source.as_bytes();
    let mut out: Vec<Lexed> = Vec::new();
    // '(' '[' '{' and 'T' for a template's `${`.
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut newline = false;
    // The last token closed the parentheses of an `if`, `while`, `for` or
    // `with`, after which a `/` starts a regular expression.
    let mut after_control_paren = false;
    let fail = |line: usize, what: &str| format!("{path}:{line}: {what}");

    // Scan a template literal's text from `i` (just after '`' or the '}' that
    // ends a substitution) to its end or its next `${`.
    fn template_text(bytes: &[u8], mut i: usize, line: &mut usize) -> Result<(usize, bool), ()> {
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'`' => return Ok((i + 1, false)),
                b'$' if bytes.get(i + 1) == Some(&b'{') => return Ok((i + 2, true)),
                b'\n' => {
                    *line += 1;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        Err(())
    }

    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            line += 1;
            newline = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let start_line = line;
            i += 2;
            loop {
                if i + 1 >= bytes.len() {
                    return Err(fail(start_line, "a comment never ends"));
                }
                if bytes[i] == b'\n' {
                    line += 1;
                    newline = true;
                }
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }
        let token_line = line;
        let push = |out: &mut Vec<Lexed>, tok: Tok, newline: &mut bool, depth: usize| {
            out.push(Lexed {
                tok,
                line: token_line,
                newline_before: *newline,
                depth,
            });
            *newline = false;
        };
        if c == b'"' || c == b'\'' {
            i += 1;
            loop {
                match bytes.get(i) {
                    None | Some(b'\n') => return Err(fail(token_line, "a string never ends")),
                    Some(b'\\') => {
                        if bytes.get(i + 1) == Some(&b'\n') {
                            line += 1;
                        }
                        i += 2;
                    }
                    Some(q) if *q == c => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            push(&mut out, Tok::Value, &mut newline, stack.len());
            continue;
        }
        if c == b'`' {
            let depth = stack.len();
            let (next, open) = template_text(bytes, i + 1, &mut line)
                .map_err(|()| fail(token_line, "a template literal never ends"))?;
            i = next;
            if open {
                stack.push((b'T', token_line));
            }
            push(&mut out, Tok::Value, &mut newline, depth);
            continue;
        }
        let regex = if c == b'/' {
            regex_allowed(&out, after_control_paren).map_err(|()| {
                fail(
                    token_line,
                    "can't tell whether this / divides or starts a regular expression; \
                     parenthesise the expression or name the regular expression first",
                )
            })?
        } else {
            false
        };
        after_control_paren = false;
        if regex {
            i += 1;
            let mut class = false;
            loop {
                match bytes.get(i) {
                    None | Some(b'\n') => {
                        return Err(fail(token_line, "a regular expression never ends"));
                    }
                    Some(b'\\') => i += 2,
                    Some(b'[') => {
                        class = true;
                        i += 1;
                    }
                    Some(b']') => {
                        class = false;
                        i += 1;
                    }
                    Some(b'/') if !class => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            while i < bytes.len() && is_ident_part(bytes[i]) {
                i += 1;
            }
            push(&mut out, Tok::Value, &mut newline, stack.len());
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            i += 1;
            while i < bytes.len() && (is_ident_part(bytes[i]) || bytes[i] == b'.') {
                i += 1;
            }
            push(&mut out, Tok::Value, &mut newline, stack.len());
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            while i < bytes.len() && is_ident_part(bytes[i]) {
                i += 1;
            }
            let word = String::from_utf8_lossy(&bytes[start..i]).into_owned();
            push(&mut out, Tok::Ident(word), &mut newline, stack.len());
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => {
                let control = c == b'('
                    && matches!(out.last().map(|l| &l.tok), Some(Tok::Ident(word)) if matches!(word.as_str(), "if" | "while" | "for" | "with"));
                push(&mut out, Tok::Punct(c as char), &mut newline, stack.len());
                stack.push((if control { b'C' } else { c }, token_line));
                i += 1;
            }
            b')' | b']' | b'}' => {
                let Some((open, open_line)) = stack.pop() else {
                    return Err(fail(token_line, &format!("'{}' closes nothing", c as char)));
                };
                if open == b'T' && c == b'}' {
                    let (next, again) = template_text(bytes, i + 1, &mut line)
                        .map_err(|()| fail(open_line, "a template literal never ends"))?;
                    i = next;
                    if again {
                        stack.push((b'T', open_line));
                    }
                    continue;
                }
                let expected = match open {
                    b'(' | b'C' => b')',
                    b'[' => b']',
                    _ => b'}',
                };
                if c != expected {
                    return Err(fail(
                        token_line,
                        &format!(
                            "'{}' closes the '{}' opened on line {open_line}",
                            c as char,
                            match open { b'T' => '$', b'C' => '(', other => other as char }
                        ),
                    ));
                }
                i += 1;
                push(&mut out, Tok::Punct(c as char), &mut newline, stack.len());
                after_control_paren = open == b'C';
            }
            _ if KNOWN_PUNCT.contains(&c) => {
                push(&mut out, Tok::Punct(c as char), &mut newline, stack.len());
                i += 1;
            }
            _ => {
                return Err(fail(
                    token_line,
                    &format!("a character the bundle's lexer doesn't know: {:?}", c as char),
                ));
            }
        }
    }
    if let Some((open, open_line)) = stack.pop() {
        let what = if open == b'T' { "a template substitution" } else { "a bracket" };
        return Err(fail(open_line, &format!("{what} opened here never closes")));
    }
    Ok(out)
}

/// What the build needs to know about one script.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptFacts {
    /// Names declared at the file's top level, in order.
    pub top_level: Vec<String>,
    /// The file's directive prologue makes it strict.
    pub strict: bool,
}

/// Whether a statement could end before `token`, so it starts a new one.
fn starts_statement(previous: Option<&Lexed>, token: &Lexed) -> bool {
    match previous {
        None => true,
        Some(prev) => match &prev.tok {
            Tok::Punct(';') | Tok::Punct('}') => true,
            Tok::Punct(')') | Tok::Punct(']') | Tok::Value | Tok::Ident(_) => token.newline_before,
            Tok::Punct(_) => false,
        },
    }
}

/// Whether an initializer ends at `token`: by automatic semicolon insertion, a
/// line that ends a complete expression followed by one starting a word.
fn ends_by_newline(previous: &Lexed, token: &Lexed) -> bool {
    token.newline_before
        && matches!(&token.tok, Tok::Ident(word) if word != "in" && word != "instanceof")
        && matches!(
            &previous.tok,
            Tok::Value | Tok::Ident(_) | Tok::Punct(')') | Tok::Punct(']') | Tok::Punct('}')
        )
}

/// Lex a script, check it is whole, and find what it declares at top level.
#[allow(dead_code)]
pub fn script_facts(path: &str, source: &str) -> Result<ScriptFacts, String> {
    let tokens = lex_js(path, source)?;
    let mut facts = ScriptFacts {
        strict: {
            let first = strip_leading_comments(source);
            first.starts_with("'use strict'") || first.starts_with("\"use strict\"")
        },
        ..ScriptFacts::default()
    };
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let previous = index.checked_sub(1).map(|at| &tokens[at]);
        if token.depth != 0 || !starts_statement(previous, token) {
            index += 1;
            continue;
        }
        let Tok::Ident(word) = &token.tok else {
            index += 1;
            continue;
        };
        let name_after = |at: usize| -> Result<String, String> {
            match tokens.get(at).map(|t| &t.tok) {
                Some(Tok::Ident(name)) => Ok(name.clone()),
                _ => Err(format!(
                    "{path}:{}: a top-level {word} without a plain name",
                    token.line
                )),
            }
        };
        match word.as_str() {
            "function" => {
                let mut at = index + 1;
                if tokens.get(at).map(|t| &t.tok) == Some(&Tok::Punct('*')) {
                    at += 1;
                }
                facts.top_level.push(name_after(at)?);
                index = at + 1;
            }
            "async"
                if matches!(tokens.get(index + 1), Some(Lexed { tok: Tok::Ident(next), newline_before: false, .. }) if next == "function") =>
            {
                let mut at = index + 2;
                if tokens.get(at).map(|t| &t.tok) == Some(&Tok::Punct('*')) {
                    at += 1;
                }
                facts.top_level.push(name_after(at)?);
                index = at + 1;
            }
            "class" => {
                facts.top_level.push(name_after(index + 1)?);
                index += 2;
            }
            "var" | "let" | "const" => {
                let mut at = index + 1;
                loop {
                    match tokens.get(at).map(|t| &t.tok) {
                        Some(Tok::Ident(name)) => facts.top_level.push(name.clone()),
                        Some(Tok::Punct('{' | '[')) => {
                            return Err(format!(
                                "{path}:{}: a top-level destructuring declaration; declare each \
                                 name the shell's other files read on its own",
                                tokens[at].line
                            ));
                        }
                        _ => {
                            return Err(format!(
                                "{path}:{}: a top-level {word} without a plain name",
                                token.line
                            ));
                        }
                    }
                    at += 1;
                    // Skip the initializer to the next declarator or the end.
                    let mut more = false;
                    while let Some(next) = tokens.get(at) {
                        if next.depth == 0 {
                            match next.tok {
                                Tok::Punct(',') => {
                                    more = true;
                                    at += 1;
                                    break;
                                }
                                Tok::Punct(';') => {
                                    at += 1;
                                    break;
                                }
                                _ => {}
                            }
                            if ends_by_newline(&tokens[at - 1], next) {
                                break;
                            }
                        }
                        at += 1;
                    }
                    if !more {
                        break;
                    }
                }
                index = at;
            }
            _ => index += 1,
        }
    }
    Ok(facts)
}

fn strip_leading_comments(source: &str) -> &str {
    let mut rest = source;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("//") {
            rest = after.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, tail)| tail);
        } else {
            return rest;
        }
    }
}

/// Names a file can't take over as globals, because the page can't redefine
/// them.
const RESERVED_GLOBALS: &[&str] = &["window", "document", "location", "top", "globalThis"];

/// Check a stylesheet is whole and can sit inside the bundle (and inside an
/// `@media` block): comments and strings end, braces balance, no rule that is
/// only valid at the very start of a sheet.
#[allow(dead_code)]
pub fn check_stylesheet(path: &str, source: &str) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut depth: Vec<(u8, usize)> = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => line += 1,
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = line;
                i += 2;
                loop {
                    if i + 1 >= bytes.len() {
                        return Err(format!("{path}:{start}: a comment never ends"));
                    }
                    if bytes[i] == b'\n' {
                        line += 1;
                    }
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            q @ (b'"' | b'\'') => {
                let start = line;
                i += 1;
                while bytes.get(i) != Some(&q) {
                    match bytes.get(i) {
                        None | Some(b'\n') => {
                            return Err(format!("{path}:{start}: a string never ends"));
                        }
                        Some(b'\\') => i += 2,
                        _ => i += 1,
                    }
                }
            }
            b'@' => {
                let rest = source[i + 1..].to_ascii_lowercase();
                for rule in ["import", "charset", "namespace"] {
                    if rest.starts_with(rule) {
                        return Err(format!(
                            "{path}:{line}: @{rule} is only valid at the start of a stylesheet, \
                             and this one is part of the shell's bundle"
                        ));
                    }
                }
            }
            c @ (b'{' | b'(' | b'[') => depth.push((c, line)),
            c @ (b'}' | b')' | b']') => {
                let expected = match c {
                    b'}' => b'{',
                    b')' => b'(',
                    _ => b'[',
                };
                match depth.pop() {
                    Some((open, _)) if open == expected => {}
                    Some((open, at)) => {
                        return Err(format!(
                            "{path}:{line}: '{}' closes the '{}' opened on line {at}",
                            c as char, open as char
                        ));
                    }
                    None => return Err(format!("{path}:{line}: '{}' closes nothing", c as char)),
                }
            }
            _ => {}
        }
        i += 1;
    }
    if let Some((open, at)) = depth.pop() {
        return Err(format!("{path}:{at}: '{}' never closes", open as char));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Assembly.

#[allow(dead_code)]
pub struct Bundle {
    pub text: String,
    /// Each segment's served path and the bundle line its first line is on.
    pub segments: Vec<(String, usize)>,
}

fn line_count(text: &str) -> usize {
    text.matches('\n').count()
}

fn with_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}

/// The shell's stylesheets in one sheet. A stylesheet the page loads with a
/// `media` query is wrapped in that query at its place, which is what the
/// attribute does, so the cascade is unchanged.
#[allow(dead_code)]
pub fn style_bundle(files: &[(BundledTag, String)]) -> Result<Bundle, String> {
    let mut text = String::from(
        "/* The journal dashboard's shell stylesheets in one file, in the order the shell page \
         lists them, each one unchanged under its own banner. Built with the journal; edit the \
         files, not this. */\n",
    );
    let mut segments = Vec::new();
    for (tag, source) in files {
        check_stylesheet(&tag.path, source)?;
        text.push_str(&format!("/* file: {} */\n", tag.path));
        if let Some(media) = &tag.media {
            text.push_str(&format!("@media {media} {{\n"));
        }
        segments.push((tag.path.clone(), line_count(&text) + 1));
        text.push_str(&with_newline(source));
        if tag.media.is_some() {
            text.push_str("}\n");
        }
    }
    Ok(bundle_end(text, segments))
}

fn bundle_end(text: String, segments: Vec<(String, usize)>) -> Bundle {
    Bundle { text, segments }
}

/// The shell's scripts in one script. `registry` is a script that runs first,
/// with data the shell reads (the app table); it is generated, not a file.
#[allow(dead_code)]
pub fn script_bundle(
    files: &[(BundledTag, String)],
    registry: &str,
    map_name: &str,
) -> Result<Bundle, String> {
    let mut facts = Vec::new();
    let mut owners: std::collections::BTreeMap<String, String> = Default::default();
    for (tag, source) in files {
        let found = script_facts(&tag.path, source)?;
        for name in &found.top_level {
            if RESERVED_GLOBALS.contains(&name.as_str()) {
                return Err(format!(
                    "{} declares {name} at its top level, which the page can't hand over",
                    tag.path
                ));
            }
            if let Some(other) = owners.insert(name.clone(), tag.path.clone()) {
                return Err(format!(
                    "{name} is declared at the top level of both {other} and {}; one global, \
                     one owner",
                    tag.path
                ));
            }
        }
        facts.push(found);
    }

    let mut text = String::from(
        "/* The journal dashboard's shell scripts in one file, in the order the shell page lists \
         them. Each segment is one file, unchanged, inside its own function: an error in one \
         stops only that one, and every name a file declares at its top level stays a global. \
         Built with the journal; edit the files, not this. */\n",
    );
    // The prologue: where each segment starts, so an error's position names
    // its file, and how a segment's error is reported.
    text.push_str("(function () { var src = document.currentScript && document.currentScript.src; var segments = [/*SEGMENTS*/];\n");
    text.push_str(concat!(
        "  function locate(file, line) { if (!src || file !== src) return null; ",
        "for (var i = segments.length - 1; i >= 0; i--) { if (line >= segments[i][0]) ",
        "return { file: '/' + segments[i][1], line: line - segments[i][0] + 1 }; } return null; }\n",
        "  function stack(text) { if (!src || typeof text !== 'string') return text; ",
        "return text.split(src + ':').map(function (part, i) { if (i === 0) return part; ",
        "var m = /^\\d+/.exec(part); var at = m && locate(src, Number(m[0])); ",
        "return at ? at.file + ':' + at.line + part.slice(m[0].length) : src + ':' + part; }).join(''); }\n",
        "  function fail(error) { if (typeof globalThis.reportError === 'function') { globalThis.reportError(error); } ",
        "else { setTimeout(function () { throw error; }); } }\n",
        "  Object.defineProperty(globalThis, '__solShell', { value: Object.freeze({ src: src, ",
        "segments: segments, locate: locate, stack: stack, fail: fail }) });\n",
        "})();\n",
    ));
    text.push_str("/* file: the app registry, generated with the journal */\n");
    text.push_str(&with_newline(registry));

    let mut segments = Vec::new();
    for ((tag, source), found) in files.iter().zip(&facts) {
        text.push_str(&format!("/* file: {} */\n", tag.path));
        let mut head = String::from("try { (function () {");
        if found.strict {
            head.push_str(" 'use strict';");
        }
        for name in &found.top_level {
            head.push_str(&format!(
                " Object.defineProperty(globalThis, '{name}', {{ configurable: true, enumerable: \
                 true, get: function () {{ return {name}; }}, set: function (value) {{ {name} = \
                 value; }} }});"
            ));
        }
        head.push('\n');
        text.push_str(&head);
        segments.push((tag.path.clone(), line_count(&text) + 1));
        text.push_str(&with_newline(source));
        text.push_str("}).call(globalThis); } catch (error) { __solShell.fail(error); }\n");
    }
    text.push_str(&format!("//# sourceMappingURL={map_name}\n"));

    // Names without their leading slash, so the table isn't read as a list
    // of references to load.
    let table = segments
        .iter()
        .map(|(path, line)| format!("[{line}, {:?}]", path.trim_start_matches('/')))
        .collect::<Vec<_>>()
        .join(", ");
    let text = text.replacen("/*SEGMENTS*/", &table, 1);
    Ok(bundle_end(text, segments))
}

fn vlq(value: i64, out: &mut String) {
    const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut v = if value < 0 { ((-value) << 1) | 1 } else { value << 1 };
    loop {
        let mut digit = (v & 31) as usize;
        v >>= 5;
        if v > 0 {
            digit |= 32;
        }
        out.push(DIGITS[digit] as char);
        if v == 0 {
            break;
        }
    }
}

/// A source map for a bundle: each segment's lines map, line for line, to its
/// file. Developer tools use it to name the file in a stack trace.
#[allow(dead_code)]
pub fn line_source_map(bundle: &Bundle, files: &[(BundledTag, String)], file: &str) -> String {
    let total = line_count(&bundle.text) + 1;
    let mut owner: Vec<Option<(usize, usize)>> = vec![None; total + 1];
    for (index, ((_, start), (_, source))) in bundle.segments.iter().zip(files).enumerate() {
        let lines = line_count(&with_newline(source));
        for offset in 0..lines {
            if let Some(slot) = owner.get_mut(start + offset) {
                *slot = Some((index, offset));
            }
        }
    }
    let mut mappings = String::new();
    let (mut last_source, mut last_line) = (0i64, 0i64);
    for line in 1..total {
        if line > 1 {
            mappings.push(';');
        }
        if let Some((source, offset)) = owner[line] {
            vlq(0, &mut mappings);
            vlq(source as i64 - last_source, &mut mappings);
            vlq(offset as i64 - last_line, &mut mappings);
            vlq(0, &mut mappings);
            last_source = source as i64;
            last_line = offset as i64;
        }
    }
    let sources = bundle
        .segments
        .iter()
        .map(|(path, _)| format!("{path:?}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"version\":3,\"file\":{file:?},\"sources\":[{sources}],\"names\":[],\"mappings\":\"{mappings}\"}}\n")
}
