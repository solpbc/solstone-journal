// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Violation {
    path: PathBuf,
    line: usize,
    rule: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileKind {
    Html,
    Css,
    Js,
}

#[derive(Clone, Debug)]
struct SourceFile {
    path: PathBuf,
    relative: PathBuf,
    kind: FileKind,
    source: String,
    clean: String,
}

#[derive(Clone, Debug, Default)]
struct Document {
    files: BTreeSet<PathBuf>,
    links_tokens: bool,
}

#[derive(Clone, Debug)]
struct Declaration {
    property: String,
    value: String,
    value_offset: usize,
}

#[derive(Clone, Debug)]
struct Attribute {
    name: String,
    value: String,
    value_offset: usize,
}

#[derive(Clone, Debug)]
struct Literal {
    start: usize,
    text: String,
}

#[derive(Clone, Debug)]
struct ReadInfo {
    valid_fallback: bool,
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn scan(root: &Path) -> Vec<Violation> {
    let mut files = Vec::new();
    let crates = root.join("core/crates");
    let Ok(entries) = fs::read_dir(&crates) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let assets = entry.path().join("assets");
        if assets.is_dir() {
            collect_assets(root, &assets, &mut files);
        }
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));

    let token_path = root.join("core/crates/solstone-core-convey-shell/assets/static/tokens.css");
    let tokens = fs::read_to_string(&token_path)
        .ok()
        .map(|source| parse_light_tokens(&source))
        .unwrap_or_default();
    let token_names = tokens.keys().cloned().collect::<BTreeSet<_>>();
    let by_path = files
        .iter()
        .map(|file| (file.path.clone(), file))
        .collect::<HashMap<_, _>>();
    let shell = files
        .iter()
        .find(|file| {
            file.relative
                .ends_with("solstone-core-convey-shell/assets/static/shell.html")
        })
        .map(|file| file.path.clone());
    let init = files
        .iter()
        .find(|file| {
            file.relative
                .ends_with("solstone-core-sol-link/assets/init.html")
        })
        .map(|file| file.path.clone());
    let shell_set = shell
        .as_ref()
        .map(|path| document_files(root, path, &by_path))
        .unwrap_or_default();

    let mut documents = Vec::new();
    for file in files.iter().filter(|file| file.kind == FileKind::Html) {
        let document_set = if Some(&file.path) == shell.as_ref() {
            shell_set.clone()
        } else if Some(&file.path) == init.as_ref() {
            document_files(root, &file.path, &by_path)
        } else {
            let mut set = document_files(root, &file.path, &by_path);
            set.extend(shell_set.iter().cloned());
            set
        };
        documents.push(Document {
            links_tokens: document_set.contains(&token_path),
            files: document_set,
        });
    }
    let mut file_documents: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (index, document) in documents.iter().enumerate() {
        for path in &document.files {
            file_documents.entry(path.clone()).or_default().push(index);
        }
    }

    let mut violations = Vec::new();
    for file in &files {
        if whole_file_allowlisted(&file.relative) {
            continue;
        }
        let allow_ranges = if file.kind == FileKind::Js {
            function_body_ranges(&file.clean, "_applyOverlays")
        } else {
            Vec::new()
        };
        match file.kind {
            FileKind::Css => scan_css(file, &token_names, &allow_ranges, &mut violations),
            FileKind::Html => {
                scan_html(file, &token_names, &tokens, &allow_ranges, &mut violations)
            }
            FileKind::Js => scan_js(file, &token_names, &tokens, &allow_ranges, &mut violations),
        }
        if file.kind == FileKind::Html {
            scan_dark_sheet_links(file, &shell, &init, &mut violations);
        } else {
            for (offset, _) in file.clean.match_indices("tokens-dark.css") {
                add_violation(&mut violations, file, offset, "dark_sheet_link_placement");
            }
        }
    }

    for file in &files {
        if whole_file_allowlisted(&file.relative) {
            continue;
        }
        let contexts = file_documents.get(&file.path).cloned().unwrap_or_default();
        let orphan = contexts.is_empty();
        for (name, offset) in find_var_uses(&file.clean) {
            let undeclared = if orphan {
                !token_names.contains(&name) && !declares_name_in_file(file, &name)
            } else {
                contexts.iter().any(|document_index| {
                    let document = &documents[*document_index];
                    let local = document.files.iter().any(|path| {
                        by_path
                            .get(path)
                            .is_some_and(|candidate| declares_name_in_file(candidate, &name))
                    });
                    !(document.links_tokens && token_names.contains(&name)) && !local
                })
            };
            if undeclared {
                add_violation(&mut violations, file, offset, "undeclared_custom_property");
            }
        }
    }
    violations.sort_by(|left, right| {
        (&left.path, left.line, left.rule).cmp(&(&right.path, right.line, right.rule))
    });
    violations.dedup();
    violations
}

fn collect_assets(root: &Path, directory: &Path, files: &mut Vec<SourceFile>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if is_excluded_target(&path) {
                continue;
            }
            collect_assets(root, &path, files);
            continue;
        }
        let Some(kind) = file_kind(&path) else {
            continue;
        };
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("tokens.css" | "tokens-dark.css")
        ) {
            continue;
        }
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        let clean = clean_source(&source, kind);
        files.push(SourceFile {
            relative: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
            path,
            kind,
            source,
            clean,
        });
    }
}

fn file_kind(path: &Path) -> Option<FileKind> {
    match path.extension()?.to_str()? {
        "html" => Some(FileKind::Html),
        "css" => Some(FileKind::Css),
        "js" => Some(FileKind::Js),
        _ => None,
    }
}

fn whole_file_allowlisted(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("pairing-qr.js" | "sunarc.js")
    )
}

fn is_excluded_target(path: &Path) -> bool {
    path.components()
        .any(|part| matches!(part, Component::Normal(name) if name == "vendor" || name == "tests"))
}

fn strip_comments(source: &str, kind: FileKind) -> String {
    if kind == FileKind::Html {
        return strip_html_comments(source);
    }
    let bytes = source.as_bytes();
    let mut output = bytes.to_vec();
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    while index < bytes.len() {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if bytes[index] == b'\\' {
                escaped = true;
            } else if bytes[index] == current {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') || bytes[index] == 96 {
            quote = Some(bytes[index]);
            index += 1;
            continue;
        }
        let comment = match kind {
            FileKind::Html if bytes[index..].starts_with(b"<!--") => Some((4, b"-->".as_slice())),
            FileKind::Css | FileKind::Js if bytes[index..].starts_with(b"/*") => {
                Some((2, b"*/".as_slice()))
            }
            FileKind::Js
                if bytes[index..].starts_with(b"//")
                    && (index == 0 || bytes[index - 1] != b':') =>
            {
                Some((2, b"\n".as_slice()))
            }
            _ => None,
        };
        if let Some((prefix, terminator)) = comment {
            let end = bytes[index + prefix..]
                .windows(terminator.len())
                .position(|window| window == terminator)
                .map(|offset| index + prefix + offset)
                .unwrap_or(bytes.len());
            let comment_end = (end + terminator.len()).min(bytes.len());
            for byte in &mut output[index..comment_end] {
                if *byte != b'\n' && *byte != b'\r' {
                    *byte = b' ';
                }
            }
            index = if end == bytes.len() {
                end
            } else {
                end + terminator.len()
            };
        } else {
            index += 1;
        }
    }
    String::from_utf8(output).expect("comment stripping preserves UTF-8")
}

fn strip_html_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut output = bytes.to_vec();
    let mut index = 0;
    let mut in_tag = false;
    let mut quote = None;
    while index < bytes.len() {
        if !in_tag && bytes[index..].starts_with(b"<!--") {
            let end = bytes[index + 4..]
                .windows(3)
                .position(|window| window == b"-->")
                .map(|offset| index + 4 + offset)
                .unwrap_or(bytes.len());
            let comment_end = if end == bytes.len() { end } else { end + 3 };
            for byte in &mut output[index..comment_end] {
                if *byte != b'\n' && *byte != b'\r' {
                    *byte = b' ';
                }
            }
            index = if end == bytes.len() { end } else { end + 3 };
            continue;
        }
        if let Some(current) = quote {
            if bytes[index] == current {
                quote = None;
            } else if bytes[index] == b'\\' {
                index = (index + 1).min(bytes.len() - 1);
            }
        } else if in_tag && matches!(bytes[index], b'\'' | b'"') {
            quote = Some(bytes[index]);
        } else if bytes[index] == b'<'
            && bytes.get(index + 1).is_some_and(|next| {
                next.is_ascii_alphabetic() || matches!(next, b'!' | b'/' | b'?')
            })
        {
            in_tag = true;
        } else if in_tag && bytes[index] == b'>' {
            in_tag = false;
        }
        index += 1;
    }
    String::from_utf8(output).expect("HTML comment stripping preserves UTF-8")
}

fn clean_source(source: &str, kind: FileKind) -> String {
    if kind != FileKind::Html {
        return strip_comments(source, kind);
    }
    let mut clean = strip_comments(source, FileKind::Html);
    let mut bytes = clean.as_bytes().to_vec();
    for tag in html_tags(&clean) {
        if tag.name == "style" || tag.name == "script" {
            let start = tag.start + clean[tag.start..].find('>').unwrap_or(0) + 1;
            if let Some(close_rel) = clean[start..].find("</") {
                let end = start + close_rel;
                let kind = if tag.name == "style" {
                    FileKind::Css
                } else {
                    FileKind::Js
                };
                let inner = strip_comments(&clean[start..end], kind);
                bytes[start..end].copy_from_slice(inner.as_bytes());
            }
        }
        for attribute in tag
            .attributes
            .iter()
            .filter(|attribute| attribute.name == "style")
        {
            let style = strip_comments(&attribute.value, FileKind::Css);
            let end = attribute.value_offset + attribute.value.len();
            bytes[attribute.value_offset..end].copy_from_slice(style.as_bytes());
        }
    }
    clean = String::from_utf8(bytes).expect("embedded comment stripping preserves UTF-8");
    clean
}

#[derive(Clone, Debug)]
struct HtmlTag {
    name: String,
    attributes: Vec<Attribute>,
    start: usize,
}

fn html_tags(source: &str) -> Vec<HtmlTag> {
    let bytes = source.as_bytes();
    let mut tags = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' || index + 1 >= bytes.len() || bytes[index + 1] == b'/' {
            index += 1;
            continue;
        }
        let start = index;
        let mut end = index + 1;
        let mut quote = None;
        while end < bytes.len() {
            if let Some(current) = quote {
                if bytes[end] == current {
                    quote = None;
                } else if bytes[end] == b'\\' {
                    end += 1;
                }
            } else if matches!(bytes[end], b'\'' | b'"') {
                quote = Some(bytes[end]);
            } else if bytes[end] == b'>' {
                break;
            }
            end += 1;
        }
        if end >= bytes.len() {
            break;
        }
        let inside = &source[index + 1..end];
        let name_end = inside
            .find(|character: char| character.is_ascii_whitespace() || character == '/')
            .unwrap_or(inside.len());
        let name = inside[..name_end].to_ascii_lowercase();
        let attributes = parse_html_attributes(source, index + 1 + name_end, end);
        tags.push(HtmlTag {
            name,
            attributes,
            start,
        });
        index = end + 1;
    }
    tags
}

fn parse_html_attributes(source: &str, mut index: usize, end: usize) -> Vec<Attribute> {
    let bytes = source.as_bytes();
    let mut attributes = Vec::new();
    while index < end {
        while index < end && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        let name_start = index;
        while index < end
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b'=' | b'/' | b'>')
        {
            index += 1;
        }
        if index == name_start {
            index += 1;
            continue;
        }
        let name = source[name_start..index].to_ascii_lowercase();
        while index < end && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= end || bytes[index] != b'=' {
            continue;
        }
        index += 1;
        while index < end && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= end {
            break;
        }
        let (value_start, value_end) = if matches!(bytes[index], b'\'' | b'"') {
            let quote = bytes[index];
            index += 1;
            let start = index;
            while index < end && bytes[index] != quote {
                index += 1;
            }
            let finish = index;
            if index < end {
                index += 1;
            }
            (start, finish)
        } else {
            let start = index;
            while index < end && !bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            (start, index)
        };
        attributes.push(Attribute {
            name,
            value: source[value_start..value_end].to_string(),
            value_offset: value_start,
        });
    }
    attributes
}

fn document_files(
    root: &Path,
    html_path: &Path,
    known_files: &HashMap<PathBuf, &SourceFile>,
) -> BTreeSet<PathBuf> {
    let mut result = BTreeSet::from([html_path.to_path_buf()]);
    let Some(file) = known_files.get(html_path) else {
        return result;
    };
    for (tag_name, attribute_name) in [("link", "href"), ("script", "src")] {
        for tag in html_tags(&file.clean) {
            if tag.name != tag_name {
                continue;
            }
            if tag_name == "link"
                && tag.attributes.iter().any(|attribute| {
                    attribute.name == "rel"
                        && !attribute
                            .value
                            .split_ascii_whitespace()
                            .any(|v| v == "stylesheet")
                })
            {
                continue;
            }
            let Some(attribute) = tag
                .attributes
                .iter()
                .find(|attribute| attribute.name == attribute_name)
            else {
                continue;
            };
            if let Some(path) = resolve_asset(root, html_path, &attribute.value, known_files) {
                if !is_excluded_target(&path) {
                    result.insert(path);
                }
            }
        }
    }
    result
}

fn resolve_asset(
    root: &Path,
    html_path: &Path,
    url: &str,
    known_files: &HashMap<PathBuf, &SourceFile>,
) -> Option<PathBuf> {
    let url = url.split(['?', '#']).next()?.trim();
    if url.is_empty()
        || url.starts_with("http:")
        || url.starts_with("https:")
        || url.starts_with("data:")
        || url.starts_with("//")
    {
        return None;
    }
    if let Some(suffix) = url.strip_prefix("/static/") {
        return Some(normalize_path(
            root.join("core/crates/solstone-core-convey-shell/assets/static")
                .join(suffix),
        ));
    }
    if let Some(route) = url.strip_prefix("/app/") {
        let mut segments = route.split('/');
        let app = segments.next()?;
        if segments.next()? != "static" {
            return None;
        }
        let asset = segments.collect::<PathBuf>();
        let assets_root = html_path
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "assets"))?;
        let app_asset = normalize_path(assets_root.join(app).join(&asset));
        if known_files.contains_key(&app_asset) {
            return Some(app_asset);
        }
        let flat_asset = normalize_path(assets_root.join(asset));
        return known_files.contains_key(&flat_asset).then_some(flat_asset);
    }
    if url.starts_with('/') {
        return None;
    }
    Some(normalize_path(html_path.parent()?.join(url)))
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn parse_light_tokens(source: &str) -> BTreeMap<String, String> {
    let clean = strip_comments(source, FileKind::Css);
    let mut tokens = BTreeMap::new();
    let bytes = clean.as_bytes();
    let mut index = 0;
    let mut depth = 0usize;
    while index < bytes.len() {
        if !bytes[index].is_ascii() {
            index += 1;
            continue;
        }
        if clean[index..].starts_with(":root")
            && (index == 0 || !is_ident(bytes[index - 1]))
            && (index + 5 == bytes.len() || !is_ident(bytes[index + 5]))
        {
            let mut open = index + 5;
            while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                open += 1;
            }
            if depth == 0 && bytes.get(open) == Some(&b'{') {
                if let Some(close) = matching_brace(&clean, open) {
                    for declaration in css_declarations(&clean[open + 1..close], open + 1) {
                        if declaration.property.starts_with("--") {
                            tokens
                                .entry(declaration.property)
                                .or_insert_with(|| normalize_color_value(&declaration.value));
                        }
                    }
                }
            }
        }
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        index += 1;
    }
    tokens
}

fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate().skip(open) {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"') || byte == 96 {
            quote = Some(byte);
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn css_declarations(source: &str, base: usize) -> Vec<Declaration> {
    let bytes = source.as_bytes();
    let mut declarations = Vec::new();
    let bare_list = !source.contains('{') && !source.contains('}');
    let mut depth = 0usize;
    let mut segment_start = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut parentheses = 0usize;
    for index in 0..bytes.len() {
        let byte = bytes[index];
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"') || byte == 96 {
            quote = Some(byte);
            continue;
        }
        match byte {
            b'(' => parentheses += 1,
            b')' => parentheses = parentheses.saturating_sub(1),
            b'{' if parentheses == 0 => {
                depth += 1;
                segment_start = index + 1;
            }
            b'}' if parentheses == 0 => {
                if depth > 0 {
                    if let Some(declaration) = parse_declaration_segment(
                        &source[segment_start..index],
                        base + segment_start,
                    ) {
                        declarations.push(declaration);
                    }
                    depth -= 1;
                }
                segment_start = index + 1;
            }
            b';' if parentheses == 0 && (depth > 0 || bare_list) => {
                if let Some(declaration) =
                    parse_declaration_segment(&source[segment_start..index], base + segment_start)
                {
                    declarations.push(declaration);
                }
                segment_start = index + 1;
            }
            _ => {}
        }
    }
    if bare_list {
        if let Some(declaration) =
            parse_declaration_segment(&source[segment_start..], base + segment_start)
        {
            declarations.push(declaration);
        }
    }
    declarations
}

fn parse_declaration_segment(segment: &str, base: usize) -> Option<Declaration> {
    let bytes = segment.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut parentheses = 0usize;
    let mut colon = None;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"') || byte == 96 {
            quote = Some(byte);
        } else {
            match byte {
                b'(' => parentheses += 1,
                b')' => parentheses = parentheses.saturating_sub(1),
                b':' if parentheses == 0 => {
                    colon = Some(index);
                    break;
                }
                _ => {}
            }
        }
    }
    let colon = colon?;
    let property = segment[..colon].trim();
    if property.is_empty()
        || !property
            .bytes()
            .all(|byte| byte == b'-' || byte.is_ascii_alphanumeric() || byte == b'_')
        || property.starts_with('@')
    {
        return None;
    }
    let value_start = colon + 1;
    let value = segment[value_start..].trim().to_string();
    let leading = segment[value_start..].len() - segment[value_start..].trim_start().len();
    Some(Declaration {
        property: property.to_ascii_lowercase(),
        value,
        value_offset: base + value_start + leading,
    })
}

fn scan_css(
    file: &SourceFile,
    token_names: &BTreeSet<String>,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    for declaration in css_declarations(&file.clean, 0) {
        scan_declaration(file, &declaration, token_names, allow_ranges, violations);
    }
}

fn scan_html(
    file: &SourceFile,
    token_names: &BTreeSet<String>,
    tokens: &BTreeMap<String, String>,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    let tags = html_tags(&file.clean);
    for tag in &tags {
        for attribute in &tag.attributes {
            match attribute.name.as_str() {
                "style" => {
                    let style = strip_comments(&attribute.value, FileKind::Css);
                    for declaration in css_declarations(&style, attribute.value_offset) {
                        scan_declaration(file, &declaration, token_names, allow_ranges, violations);
                    }
                }
                "fill" | "stroke" | "stop-color" | "color" => {
                    scan_value_literals(
                        file,
                        &attribute.value,
                        attribute.value_offset,
                        &attribute.name,
                        false,
                        allow_ranges,
                        violations,
                    );
                    scan_color_fallbacks(
                        file,
                        &attribute.value,
                        attribute.value_offset,
                        allow_ranges,
                        violations,
                    );
                }
                _ => {}
            }
        }
    }
    for tag in &tags {
        let after_tag = tag.start + file.clean[tag.start..].find('>').unwrap_or(0) + 1;
        let Some(close_rel) = file.clean[after_tag..].find("</") else {
            continue;
        };
        let content_start = after_tag;
        let content_end = after_tag + close_rel;
        let body = &file.clean[content_start..content_end];
        match tag.name.as_str() {
            "style" => {
                let style = strip_comments(body, FileKind::Css);
                for declaration in css_declarations(&style, content_start) {
                    scan_declaration(file, &declaration, token_names, allow_ranges, violations);
                }
            }
            "script" => {
                let script = strip_comments(body, FileKind::Js);
                let inline_ranges = function_body_ranges(&script, "_applyOverlays")
                    .into_iter()
                    .map(|(start, end)| (content_start + start, content_start + end))
                    .collect::<Vec<_>>();
                scan_js_source(
                    file,
                    &script,
                    content_start,
                    token_names,
                    tokens,
                    &inline_ranges,
                    violations,
                );
            }
            _ => {}
        }
    }
}

fn scan_declaration(
    file: &SourceFile,
    declaration: &Declaration,
    token_names: &BTreeSet<String>,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    if declaration.property.starts_with("--") && token_names.contains(&declaration.property) {
        add_violation(
            violations,
            file,
            declaration.value_offset,
            "token_redeclaration",
        );
    }
    if declaration.property == "color-scheme" {
        add_violation(
            violations,
            file,
            declaration
                .value_offset
                .saturating_sub("color-scheme:".len()),
            "color_scheme_declaration",
        );
    }
    scan_value_literals(
        file,
        &declaration.value,
        declaration.value_offset,
        &declaration.property,
        false,
        allow_ranges,
        violations,
    );
    scan_color_fallbacks(
        file,
        &declaration.value,
        declaration.value_offset,
        allow_ranges,
        violations,
    );
}

fn scan_js(
    file: &SourceFile,
    token_names: &BTreeSet<String>,
    tokens: &BTreeMap<String, String>,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    scan_js_source(
        file,
        &file.clean,
        0,
        token_names,
        tokens,
        allow_ranges,
        violations,
    );
}

fn scan_js_source(
    file: &SourceFile,
    source: &str,
    base: usize,
    token_names: &BTreeSet<String>,
    tokens: &BTreeMap<String, String>,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    let literals = js_strings(source, base);
    let const_colors = color_literal_bindings(source);
    let approved_fallbacks = approved_canvas_fallbacks(source, base, tokens, &literals);
    scan_interpolated_channel_bindings(file, source, base, &literals, allow_ranges, violations);
    for literal in &literals {
        let content_base = literal.start + 1;
        let local_start = literal.start.saturating_sub(base);
        let template = source
            .as_bytes()
            .get(local_start)
            .is_some_and(|byte| *byte == 96);
        let shadow =
            js_shadow_context(source, local_start) || literal.text.contains("drop-shadow(");
        for color in color_literals(&literal.text, true) {
            let absolute = content_base + color.start;
            if inside_ranges(absolute, allow_ranges)
                || approved_fallbacks.contains(&absolute)
                || (shadow && is_black_alpha(&color.text))
            {
                continue;
            }
            add_violation(
                violations,
                file,
                absolute,
                if shadow {
                    "shadow_literal"
                } else {
                    "color_literal"
                },
            );
        }
        scan_color_fallbacks(file, &literal.text, content_base, allow_ranges, violations);
        if template {
            scan_channel_triplets_in_template(
                file,
                &literal.text,
                content_base,
                allow_ranges,
                violations,
            );
        }
    }
    scan_js_set_properties(
        file,
        source,
        base,
        token_names,
        &const_colors,
        allow_ranges,
        violations,
    );
    scan_js_style_assignments(file, source, base, &const_colors, allow_ranges, violations);
    scan_js_style_objects(file, source, base, &const_colors, allow_ranges, violations);
    scan_js_computed_reads(file, source, base, tokens, &literals, violations);

    for literal in literals {
        for declaration in css_declarations(&literal.text, literal.start + 1) {
            if declaration.property == "color-scheme" {
                add_violation(
                    violations,
                    file,
                    declaration.value_offset,
                    "color_scheme_declaration",
                );
            }
            scan_var_redeclarations_in_value(
                file,
                &declaration.value,
                declaration.value_offset,
                token_names,
                violations,
            );
        }
    }
}

fn js_strings(source: &str, base: usize) -> Vec<Literal> {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !matches!(bytes[index], b'\'' | b'"') && bytes[index] != 96 {
            index += 1;
            continue;
        }
        let quote = bytes[index];
        let start = index;
        index += 1;
        let content_start = index;
        let mut escaped = false;
        while index < bytes.len() {
            if escaped {
                escaped = false;
            } else if bytes[index] == b'\\' {
                escaped = true;
            } else if bytes[index] == quote {
                break;
            }
            index += 1;
        }
        let content_end = index.min(bytes.len());
        result.push(Literal {
            start: base + start,
            text: source[content_start..content_end].to_string(),
        });
        index = (index + 1).min(bytes.len());
    }
    result
}

fn scan_js_set_properties(
    file: &SourceFile,
    source: &str,
    base: usize,
    token_names: &BTreeSet<String>,
    const_colors: &BTreeSet<String>,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find("setProperty") {
        let start = cursor + relative;
        cursor = start + "setProperty".len();
        if (start > 0 && is_ident(source.as_bytes()[start - 1]))
            || (cursor < source.len() && is_ident(source.as_bytes()[cursor]))
        {
            continue;
        }
        let Some(open_rel) = source[cursor..].find('(') else {
            continue;
        };
        let open = cursor + open_rel;
        let Some(close) = matching_paren(source, open) else {
            continue;
        };
        let Some((property_arg, value_arg, property_offset, value_offset)) =
            first_two_arguments(&source[open + 1..close], base + open + 1)
        else {
            continue;
        };
        let property = unquote(property_arg).trim();
        if property == "color-scheme" {
            add_violation(
                violations,
                file,
                property_offset,
                "color_scheme_declaration",
            );
        }
        if property.starts_with("--") && token_names.contains(property) {
            add_violation(violations, file, property_offset, "token_redeclaration");
        }
        let value = value_arg.trim();
        if (is_color_value(value, true) || identifier_is_color(value, const_colors))
            && !inside_ranges(value_offset, allow_ranges)
        {
            add_violation(violations, file, value_offset, "dom_style_literal");
        }
        cursor = close + 1;
    }
}

fn scan_js_style_assignments(
    file: &SourceFile,
    source: &str,
    base: usize,
    const_colors: &BTreeSet<String>,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    for sink in [".style.", ".fillStyle", ".strokeStyle", ".shadowColor"] {
        let mut cursor = 0;
        while let Some(relative) = source[cursor..].find(sink) {
            let start = cursor + relative;
            cursor = start + sink.len();
            if sink == ".style." {
                let property_start = cursor;
                cursor = source[cursor..]
                    .find(|character: char| {
                        character.is_ascii_whitespace() || matches!(character, '=' | '(' | ';')
                    })
                    .map(|offset| cursor + offset)
                    .unwrap_or(source.len());
                let style_property = source[property_start..cursor].trim();
                if style_property.eq_ignore_ascii_case("colorScheme")
                    || style_property == "color-scheme"
                {
                    add_violation(
                        violations,
                        file,
                        base + property_start,
                        "color_scheme_declaration",
                    );
                }
            }
            let Some(equal_rel) = source[cursor..].find('=') else {
                continue;
            };
            let equal = cursor + equal_rel;
            if source.as_bytes().get(equal + 1) == Some(&b'=') {
                continue;
            }
            let end = source[equal + 1..]
                .find(';')
                .map(|offset| equal + 1 + offset)
                .unwrap_or(source.len());
            let raw_value = &source[equal + 1..end];
            let value = raw_value.trim();
            let value_offset = base + equal + 1 + raw_value.len() - raw_value.trim_start().len();
            if sink == ".style."
                && (is_color_value(value, true) || identifier_is_color(value, const_colors))
                && !inside_ranges(value_offset, allow_ranges)
            {
                add_violation(violations, file, value_offset, "dom_style_literal");
            }
            if matches!(sink, ".fillStyle" | ".strokeStyle" | ".shadowColor")
                && value.contains("var(")
            {
                add_violation(violations, file, value_offset, "canvas_var_string");
            }
        }
    }
}

fn scan_js_style_objects(
    file: &SourceFile,
    source: &str,
    base: usize,
    const_colors: &BTreeSet<String>,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find("style") {
        let start = cursor + relative;
        cursor = start + "style".len();
        if (start > 0 && is_ident(source.as_bytes()[start - 1]))
            || (cursor < source.len() && is_ident(source.as_bytes()[cursor]))
        {
            continue;
        }
        let Some(colon_rel) = source[cursor..].find(':') else {
            continue;
        };
        let colon = cursor + colon_rel;
        let Some(open_rel) = source[colon + 1..].find('{') else {
            continue;
        };
        let open = colon + 1 + open_rel;
        if let Some(close) = matching_brace(source, open) {
            for declaration in css_declarations(&source[open..=close], base + open) {
                if declaration.property == "colorscheme" || declaration.property == "color-scheme" {
                    add_violation(
                        violations,
                        file,
                        declaration.value_offset,
                        "color_scheme_declaration",
                    );
                }
                scan_value_literals(
                    file,
                    &declaration.value,
                    declaration.value_offset,
                    &declaration.property,
                    true,
                    allow_ranges,
                    violations,
                );
                if is_color_value(&declaration.value, true)
                    || identifier_is_color(&declaration.value, const_colors)
                {
                    if !inside_ranges(declaration.value_offset, allow_ranges) {
                        add_violation(
                            violations,
                            file,
                            declaration.value_offset,
                            "dom_style_literal",
                        );
                    }
                }
            }
            cursor = close + 1;
        }
    }
}

fn scan_js_computed_reads(
    file: &SourceFile,
    source: &str,
    base: usize,
    tokens: &BTreeMap<String, String>,
    literals: &[Literal],
    violations: &mut Vec<Violation>,
) {
    let mut reads = HashMap::new();
    for (statement_start, statement_end, statement) in statements(source) {
        if !statement.contains("getPropertyValue") && !statement.contains("getComputedStyle") {
            continue;
        }
        let info = computed_read_info(statement, tokens, literals, base + statement_start);
        if let Some((binding, _)) = assigned_identifier(statement) {
            if let Some(info) = info.clone() {
                reads.insert(binding, info);
            }
        }
        if contains_style_sink(statement) {
            let allowed =
                is_canvas_sink(statement) && info.as_ref().is_some_and(|read| read.valid_fallback);
            if !allowed {
                add_violation(
                    violations,
                    file,
                    base + statement_start,
                    "computed_color_read",
                );
            }
        }
        let _ = statement_end;
    }
    for (start, _, statement) in statements(source) {
        if !contains_style_sink(statement) {
            continue;
        }
        let rhs = statement.split_once('=').map(|(_, rhs)| rhs).unwrap_or("");
        for (binding, read) in &reads {
            if contains_identifier(rhs, binding) {
                let allowed = is_canvas_sink(statement) && read.valid_fallback;
                if !allowed {
                    add_violation(violations, file, base + start, "computed_color_read");
                }
            }
        }
    }
}

fn approved_canvas_fallbacks(
    source: &str,
    base: usize,
    tokens: &BTreeMap<String, String>,
    literals: &[Literal],
) -> BTreeSet<usize> {
    let mut approved = BTreeSet::new();
    for (start, _, statement) in statements(source) {
        if !is_canvas_sink(statement) {
            continue;
        }
        let Some(info) = computed_read_info(statement, tokens, literals, base + start) else {
            continue;
        };
        if !info.valid_fallback {
            continue;
        }
        if let Some((_, rhs)) = statement.split_once('=') {
            for literal in literals {
                let local_start = literal.start.saturating_sub(base);
                if local_start < source.len() && rhs.contains(&literal.text) {
                    approved.insert(literal.start + 1);
                }
            }
        }
    }
    approved
}

fn computed_read_info(
    statement: &str,
    tokens: &BTreeMap<String, String>,
    literals: &[Literal],
    global_base: usize,
) -> Option<ReadInfo> {
    let Some(read_pos) = statement.find("getPropertyValue") else {
        return statement.contains("getComputedStyle").then_some(ReadInfo {
            valid_fallback: false,
        });
    };
    let open = statement[read_pos..].find('(')? + read_pos;
    let close = matching_paren(statement, open)?;
    let property = unquote(statement[open + 1..close].trim()).trim();
    let full_name = if property.starts_with("--") {
        property.to_string()
    } else {
        return None;
    };
    let rest = statement[close + 1..].trim();
    let rest = rest.strip_prefix(".trim()").unwrap_or(rest).trim_start();
    let fallback = if let Some((_, fallback)) = rest.split_once("||") {
        Some(fallback.trim().trim_end_matches(';').trim())
    } else if let Some((condition, branches)) = rest.split_once('?') {
        branches
            .split_once(':')
            .and_then(|(when_true, when_false)| {
                let when_true = when_true.trim();
                let when_false = when_false.trim().trim_end_matches(';').trim();
                let condition = condition.trim();
                if condition.contains("=== ''")
                    || condition.contains("=== \"\"")
                    || condition.contains("== null")
                    || condition.contains("=== null")
                    || condition.contains("=== undefined")
                    || condition.starts_with('!')
                {
                    Some(when_true)
                } else if when_true == condition || when_true.contains("getPropertyValue") {
                    Some(when_false)
                } else if when_false.contains("getPropertyValue") {
                    Some(when_true)
                } else {
                    None
                }
            })
    } else {
        None
    };
    let expected = tokens.get(&full_name);
    let actual = fallback.and_then(|fallback| {
        literals.iter().find(|literal| {
            literal.start >= global_base
                && literal.start < global_base + statement.len()
                && fallback.contains(&literal.text)
        })
    });
    let valid_fallback = match (expected, actual) {
        (Some(expected), Some(actual)) => {
            normalize_color_value(&actual.text) == normalize_color_value(expected)
        }
        _ => false,
    };
    Some(ReadInfo { valid_fallback })
}

fn scan_channel_triplets_in_template(
    file: &SourceFile,
    value: &str,
    base: usize,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() || (index > 0 && bytes[index - 1].is_ascii_digit()) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_digit()
                || bytes[index].is_ascii_whitespace()
                || bytes[index] == b',')
        {
            index += 1;
        }
        let parts = value[start..index]
            .trim()
            .split(',')
            .map(str::trim)
            .collect::<Vec<_>>();
        if parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.parse::<f32>().is_ok())
        {
            let before = &value[..start];
            if ["rgba(", "rgb(", "hsla(", "hsl("]
                .iter()
                .any(|name| before.rfind(name).is_some())
            {
                let absolute = base + start;
                if !inside_ranges(absolute, allow_ranges) {
                    add_violation(violations, file, absolute, "channel_triplet");
                }
            }
        }
    }
}

fn scan_interpolated_channel_bindings(
    file: &SourceFile,
    source: &str,
    base: usize,
    literals: &[Literal],
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    let mut bindings = Vec::new();
    for (statement_start, _, statement) in statements(source) {
        let Some((name, _value)) = assigned_identifier(statement) else {
            continue;
        };
        let Some(literal) = js_strings(statement, base + statement_start)
            .into_iter()
            .find(|literal| is_channel_triplet(&literal.text))
        else {
            continue;
        };
        bindings.push((name, literal.start + 1));
    }
    for (name, literal_offset) in bindings {
        let placeholder = format!("{}{{{name}}}", '$');
        for literal in literals {
            let local_start = literal.start.saturating_sub(base);
            if source.as_bytes().get(local_start) != Some(&96) {
                continue;
            }
            let Some(position) = literal.text.find(&placeholder) else {
                continue;
            };
            let before = &literal.text[..position];
            let inside_function = ["rgba(", "rgb(", "hsla(", "hsl("]
                .iter()
                .filter_map(|function| before.rfind(function))
                .any(|start| !before[start..].contains(')'));
            if inside_function && !inside_ranges(literal_offset, allow_ranges) {
                add_violation(violations, file, literal_offset, "channel_triplet");
                break;
            }
        }
    }
}

fn is_channel_triplet(value: &str) -> bool {
    let channels = value.trim().split(',').map(str::trim).collect::<Vec<_>>();
    channels.len() == 3
        && channels
            .iter()
            .all(|channel| !channel.is_empty() && channel.parse::<f32>().is_ok())
}

fn scan_value_literals(
    file: &SourceFile,
    value: &str,
    base: usize,
    property: &str,
    js_context: bool,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    let shadow = property.contains("shadow") || value.contains("drop-shadow(");
    for literal in color_literals(value, js_context) {
        let absolute = base + literal.start;
        if inside_ranges(absolute, allow_ranges) || (shadow && is_black_alpha(&literal.text)) {
            continue;
        }
        add_violation(
            violations,
            file,
            absolute,
            if shadow {
                "shadow_literal"
            } else {
                "color_literal"
            },
        );
    }
}

fn scan_color_fallbacks(
    file: &SourceFile,
    value: &str,
    base: usize,
    allow_ranges: &[(usize, usize)],
    violations: &mut Vec<Violation>,
) {
    let mut cursor = 0;
    while let Some(relative) = value[cursor..].find("var(") {
        let start = cursor + relative;
        let open = start + 3;
        let Some(close) = matching_paren(value, open) else {
            break;
        };
        let args = &value[open + 1..close];
        if let Some(comma) = top_level_comma(args) {
            for literal in color_literals(&args[comma + 1..], false) {
                let absolute = base + open + 1 + comma + 1 + literal.start;
                if !inside_ranges(absolute, allow_ranges) {
                    add_violation(violations, file, absolute, "color_fallback");
                }
            }
        }
        cursor = close + 1;
    }
}

fn scan_var_redeclarations_in_value(
    file: &SourceFile,
    value: &str,
    base: usize,
    token_names: &BTreeSet<String>,
    violations: &mut Vec<Violation>,
) {
    let mut cursor = 0;
    while let Some(relative) = value[cursor..].find("setProperty") {
        let start = cursor + relative;
        cursor = start + "setProperty".len();
        let Some(open_rel) = value[cursor..].find('(') else {
            continue;
        };
        let open = cursor + open_rel;
        let Some(close) = matching_paren(value, open) else {
            continue;
        };
        let first = value[open + 1..close]
            .split(',')
            .next()
            .unwrap_or("")
            .trim();
        if token_names.contains(unquote(first).trim()) {
            add_violation(violations, file, base + open + 1, "token_redeclaration");
        }
        cursor = close + 1;
    }
}

fn find_var_uses(source: &str) -> Vec<(String, usize)> {
    let mut uses = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find("var(") {
        let start = cursor + relative;
        let name_start = start + 4;
        if !source[name_start..].starts_with("--") {
            cursor = name_start;
            continue;
        }
        let name_end = source[name_start..]
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
            .map(|offset| name_start + offset)
            .unwrap_or(source.len());
        uses.push((source[name_start..name_end].to_string(), start));
        cursor = name_end.max(name_start + 2);
    }
    uses
}

fn declares_name_in_file(file: &SourceFile, name: &str) -> bool {
    match file.kind {
        FileKind::Css => css_declarations(&file.clean, 0)
            .iter()
            .any(|declaration| declaration.property == name),
        FileKind::Html => {
            html_tags(&file.clean).iter().any(|tag| {
                tag.attributes.iter().any(|attribute| {
                    attribute.name == "style"
                        && css_declarations(&attribute.value, 0)
                            .iter()
                            .any(|declaration| declaration.property == name)
                })
            }) || html_style_declarations(&file.clean)
                .iter()
                .any(|declaration| declaration.property == name)
                || html_script_sources(&file.clean)
                    .iter()
                    .any(|script| js_declares_property(script, name))
        }
        FileKind::Js => js_declares_property(&file.clean, name),
    }
}

fn html_style_declarations(source: &str) -> Vec<Declaration> {
    let mut declarations = Vec::new();
    for tag in html_tags(source) {
        if tag.name != "style" {
            continue;
        }
        let after_tag = tag.start + source[tag.start..].find('>').unwrap_or(0) + 1;
        if let Some(close_rel) = source[after_tag..].find("</") {
            let start = after_tag;
            declarations.extend(css_declarations(&source[start..start + close_rel], start));
        }
    }
    declarations
}

fn html_script_sources(source: &str) -> Vec<&str> {
    let mut scripts = Vec::new();
    for tag in html_tags(source) {
        if tag.name != "script" {
            continue;
        }
        let start = tag.start + source[tag.start..].find('>').unwrap_or(0) + 1;
        if let Some(close_rel) = source[start..].find("</") {
            scripts.push(&source[start..start + close_rel]);
        }
    }
    scripts
}

fn js_declares_property(source: &str, name: &str) -> bool {
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find("setProperty") {
        let after_name = cursor + relative + "setProperty".len();
        let Some(open_rel) = source[after_name..].find('(') else {
            return false;
        };
        let open = after_name + open_rel;
        let Some(close) = matching_paren(source, open) else {
            return false;
        };
        if top_level_comma(&source[open + 1..close])
            .is_some_and(|comma| unquote(&source[open + 1..open + 1 + comma]).trim() == name)
        {
            return true;
        }
        cursor = close + 1;
    }
    false
}

fn color_literals(value: &str, js_context: bool) -> Vec<Literal> {
    let bytes = value.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii() {
            index += 1;
            continue;
        }
        if bytes[index] == b'#' {
            let mut end = index + 1;
            while end < bytes.len() && bytes[end].is_ascii_hexdigit() && end - index <= 8 {
                end += 1;
            }
            let digits = end - index - 1;
            if matches!(digits, 3 | 4 | 6 | 8) && (end == bytes.len() || !is_ident(bytes[end])) {
                result.push(Literal {
                    start: index,
                    text: value[index..end].to_string(),
                });
                index = end;
                continue;
            }
        }
        let mut function_end = None;
        for name in ["rgba", "rgb", "hsla", "hsl"] {
            if value[index..].starts_with(name) && (index == 0 || !is_ident(bytes[index - 1])) {
                let open = index + name.len();
                if bytes.get(open) != Some(&b'(') {
                    continue;
                }
                if let Some(close) = matching_paren(value, open) {
                    let channels = &value[open + 1..close];
                    if channels.bytes().any(|byte| byte.is_ascii_digit())
                        && !channels.trim_start().starts_with("var(")
                    {
                        function_end = Some(close + 1);
                        break;
                    }
                }
            }
        }
        if let Some(end) = function_end {
            result.push(Literal {
                start: index,
                text: value[index..end].to_string(),
            });
            index = end;
            continue;
        }
        if bytes[index].is_ascii_alphabetic() {
            let end = value[index..]
                .find(|character: char| !character.is_ascii_alphabetic())
                .map(|offset| index + offset)
                .unwrap_or(value.len());
            let word = &value[index..end];
            if is_named_color(word)
                && !matches!(
                    word.to_ascii_lowercase().as_str(),
                    "transparent" | "inherit" | "currentcolor"
                )
                && (index == 0 || !is_ident(bytes[index - 1]))
                && (end == bytes.len() || !is_ident(bytes[end]))
                && (!js_context || js_named_color_context(value, index, end))
            {
                result.push(Literal {
                    start: index,
                    text: word.to_string(),
                });
            }
            index = end.max(index + 1);
            continue;
        }
        index += 1;
    }
    result
}

fn js_named_color_context(source: &str, start: usize, end: usize) -> bool {
    if source.trim().eq_ignore_ascii_case(&source[start..end]) {
        return true;
    }
    let before = &source[..start];
    let Some(colon) = before.rfind(':') else {
        return false;
    };
    let property = before[..colon]
        .rsplit([';', '{', '}'])
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        property.as_str(),
        "color"
            | "background"
            | "background-color"
            | "border"
            | "border-color"
            | "fill"
            | "stroke"
            | "outline"
            | "box-shadow"
            | "text-shadow"
    )
}

fn is_named_color(word: &str) -> bool {
    const COLORS: &[&str] = &[
        "aliceblue",
        "antiquewhite",
        "aqua",
        "aquamarine",
        "azure",
        "beige",
        "bisque",
        "black",
        "blanchedalmond",
        "blue",
        "blueviolet",
        "brown",
        "burlywood",
        "cadetblue",
        "chartreuse",
        "chocolate",
        "coral",
        "cornflowerblue",
        "cornsilk",
        "crimson",
        "cyan",
        "darkblue",
        "darkcyan",
        "darkgoldenrod",
        "darkgray",
        "darkgreen",
        "darkgrey",
        "darkkhaki",
        "darkmagenta",
        "darkolivegreen",
        "darkorange",
        "darkorchid",
        "darkred",
        "darksalmon",
        "darkseagreen",
        "darkslateblue",
        "darkslategray",
        "darkslategrey",
        "darkturquoise",
        "darkviolet",
        "deeppink",
        "deepskyblue",
        "dimgray",
        "dimgrey",
        "dodgerblue",
        "firebrick",
        "floralwhite",
        "forestgreen",
        "fuchsia",
        "gainsboro",
        "ghostwhite",
        "gold",
        "goldenrod",
        "gray",
        "green",
        "greenyellow",
        "grey",
        "honeydew",
        "hotpink",
        "indianred",
        "indigo",
        "ivory",
        "khaki",
        "lavender",
        "lavenderblush",
        "lawngreen",
        "lemonchiffon",
        "lightblue",
        "lightcoral",
        "lightcyan",
        "lightgoldenrodyellow",
        "lightgray",
        "lightgreen",
        "lightgrey",
        "lightpink",
        "lightsalmon",
        "lightseagreen",
        "lightskyblue",
        "lightslategray",
        "lightslategrey",
        "lightsteelblue",
        "lightyellow",
        "lime",
        "limegreen",
        "linen",
        "magenta",
        "maroon",
        "mediumaquamarine",
        "mediumblue",
        "mediumorchid",
        "mediumpurple",
        "mediumseagreen",
        "mediumslateblue",
        "mediumspringgreen",
        "mediumturquoise",
        "mediumvioletred",
        "midnightblue",
        "mintcream",
        "mistyrose",
        "moccasin",
        "navajowhite",
        "navy",
        "oldlace",
        "olive",
        "olivedrab",
        "orange",
        "orangered",
        "orchid",
        "palegoldenrod",
        "palegreen",
        "paleturquoise",
        "palevioletred",
        "papayawhip",
        "peachpuff",
        "peru",
        "pink",
        "plum",
        "powderblue",
        "purple",
        "rebeccapurple",
        "red",
        "rosybrown",
        "royalblue",
        "saddlebrown",
        "salmon",
        "sandybrown",
        "seagreen",
        "seashell",
        "sienna",
        "silver",
        "skyblue",
        "slateblue",
        "slategray",
        "slategrey",
        "snow",
        "springgreen",
        "steelblue",
        "tan",
        "teal",
        "thistle",
        "tomato",
        "turquoise",
        "violet",
        "wheat",
        "white",
        "whitesmoke",
        "yellow",
        "yellowgreen",
    ];
    COLORS.contains(&word.to_ascii_lowercase().as_str())
}

fn is_black_alpha(color: &str) -> bool {
    let color = color.trim().to_ascii_lowercase();
    if let Some(hex) = color.strip_prefix('#') {
        return match hex.len() {
            4 => hex[..3] == *"000" && hex.as_bytes()[3] != b'f',
            8 => hex[..6] == *"000000" && hex.as_bytes()[6..8] != *b"ff",
            _ => false,
        };
    }
    let Some(open) = color.find('(') else {
        return false;
    };
    let Some(close) = color.rfind(')') else {
        return false;
    };
    let channels = color[open + 1..close].replace('/', ",");
    let parts = channels
        .split([',', ' '])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 4
        || parts[..3].iter().any(|channel| {
            channel
                .trim_end_matches('%')
                .parse::<f32>()
                .map_or(true, |number| number != 0.0)
        })
    {
        return false;
    }
    parts[3].parse::<f32>().is_ok_and(|alpha| alpha < 1.0)
}

fn is_color_value(value: &str, js_context: bool) -> bool {
    !color_literals(value, js_context).is_empty()
}

fn color_literal_bindings(source: &str) -> BTreeSet<String> {
    let mut bindings = BTreeSet::new();
    for (_, _, statement) in statements(source) {
        if let Some((name, value)) = assigned_identifier(statement) {
            if is_color_value(value.trim(), true) {
                bindings.insert(name);
            }
        }
    }
    bindings
}

fn assigned_identifier(statement: &str) -> Option<(String, &str)> {
    let trimmed = statement.trim();
    let body = trimmed
        .strip_prefix("const ")
        .or_else(|| trimmed.strip_prefix("let "))
        .or_else(|| trimmed.strip_prefix("var "))
        .unwrap_or(trimmed);
    let equal = body.find('=')?;
    let name = body[..equal].trim();
    if !is_identifier(name) {
        return None;
    }
    Some((
        name.to_string(),
        body[equal + 1..].trim().trim_end_matches(';'),
    ))
}

fn identifier_is_color(value: &str, const_colors: &BTreeSet<String>) -> bool {
    let value = value.trim().trim_end_matches(';');
    is_identifier(value) && const_colors.contains(value)
}

fn statements(source: &str) -> Vec<(usize, usize, &str)> {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    while index < bytes.len() {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if bytes[index] == b'\\' {
                escaped = true;
            } else if bytes[index] == current {
                quote = None;
            }
        } else if matches!(bytes[index], b'\'' | b'"') || bytes[index] == 96 {
            quote = Some(bytes[index]);
        } else if bytes[index] == b';' || bytes[index] == b'\n' {
            if !source[start..index].trim().is_empty() {
                result.push((start, index, &source[start..index]));
            }
            start = index + 1;
        }
        index += 1;
    }
    if !source[start..].trim().is_empty() {
        result.push((start, source.len(), &source[start..]));
    }
    result
}

fn contains_style_sink(statement: &str) -> bool {
    statement.contains(".style.")
        || statement.contains("style:")
        || statement.contains(".fillStyle")
        || statement.contains(".strokeStyle")
        || statement.contains(".shadowColor")
        || statement.contains("setProperty(")
}

fn is_canvas_sink(statement: &str) -> bool {
    statement.contains(".fillStyle")
        || statement.contains(".strokeStyle")
        || statement.contains(".shadowColor")
}

fn contains_identifier(source: &str, identifier: &str) -> bool {
    source.match_indices(identifier).any(|(index, _)| {
        (index == 0 || !is_ident(source.as_bytes()[index - 1]))
            && (index + identifier.len() == source.len()
                || !is_ident(source.as_bytes()[index + identifier.len()]))
    })
}

fn matching_paren(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate().skip(open) {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"') || byte == 96 {
            quote = Some(byte);
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn first_two_arguments(args: &str, base: usize) -> Option<(&str, &str, usize, usize)> {
    let comma = top_level_comma(args)?;
    let after = &args[comma + 1..];
    let value_offset = base + comma + 1 + after.len() - after.trim_start().len();
    Some((args[..comma].trim(), after.trim(), base, value_offset))
}

fn top_level_comma(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') || byte == 96 {
            quote = Some(byte);
        } else if matches!(byte, b'(' | b'{' | b'[') {
            depth += 1;
        } else if matches!(byte, b')' | b'}' | b']') {
            depth = depth.saturating_sub(1);
        } else if byte == b',' && depth == 0 {
            return Some(index);
        }
    }
    None
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (matches!(bytes[0], b'\'' | b'"') || bytes[0] == 96)
            && bytes[0] == *bytes.last().unwrap_or(&0)
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphabetic() || first == b'_' || first == b'$')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn normalize_color_value(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    let Some(hex) = value.strip_prefix('#') else {
        return value;
    };
    match hex.len() {
        3 => format!(
            "#{}{}{}{}{}{}",
            &hex[0..1],
            &hex[0..1],
            &hex[1..2],
            &hex[1..2],
            &hex[2..3],
            &hex[2..3]
        ),
        6 => value,
        _ => value,
    }
}

fn function_body_ranges(source: &str, name: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for start in code_identifier_positions(source, name) {
        let cursor = start + name.len();
        if (start > 0 && is_ident(source.as_bytes()[start - 1]))
            || (cursor < source.len() && is_ident(source.as_bytes()[cursor]))
        {
            continue;
        }
        let Some(open) = function_body_open(source, cursor) else {
            continue;
        };
        if let Some(close) = matching_brace(source, open) {
            ranges.push((open, close + 1));
        }
    }
    ranges
}

fn code_identifier_positions(source: &str, name: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    let name = name.as_bytes();
    let mut positions = Vec::new();
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    while index < bytes.len() {
        if !bytes[index].is_ascii() {
            index += 1;
            continue;
        }
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if bytes[index] == b'\\' {
                escaped = true;
            } else if bytes[index] == current {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') || bytes[index] == 96 {
            quote = Some(bytes[index]);
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(name) {
            positions.push(index);
            index += name.len();
        } else {
            index += 1;
        }
    }
    positions
}

fn function_body_open(source: &str, mut cursor: usize) -> Option<usize> {
    cursor = skip_ascii_whitespace(source, cursor);
    if source.as_bytes().get(cursor) == Some(&b'=') {
        cursor = skip_ascii_whitespace(source, cursor + 1);
        if source[cursor..].starts_with("function") {
            cursor += "function".len();
            cursor = source[cursor..].find('(')? + cursor;
        } else {
            let arrow = find_code_substring(source, cursor, "=>")?;
            let body = skip_ascii_whitespace(source, arrow + 2);
            return (source.as_bytes().get(body) == Some(&b'{')).then_some(body);
        }
    }
    if source.as_bytes().get(cursor) != Some(&b'(') {
        return None;
    }
    let close = matching_paren(source, cursor)?;
    let mut body = skip_ascii_whitespace(source, close + 1);
    if source[body..].starts_with("=>") {
        body = skip_ascii_whitespace(source, body + 2);
    }
    (source.as_bytes().get(body) == Some(&b'{')).then_some(body)
}

fn skip_ascii_whitespace(source: &str, mut index: usize) -> usize {
    let bytes = source.as_bytes();
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn find_code_substring(source: &str, start: usize, needle: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let needle = needle.as_bytes();
    let mut index = start;
    let mut quote = None;
    let mut escaped = false;
    while index < bytes.len() {
        if !bytes[index].is_ascii() {
            index += 1;
            continue;
        }
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if bytes[index] == b'\\' {
                escaped = true;
            } else if bytes[index] == current {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') || bytes[index] == 96 {
            quote = Some(bytes[index]);
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(needle) {
            return Some(index);
        }
        if bytes[index] == b';' {
            return None;
        }
        index += 1;
    }
    None
}

fn js_shadow_context(source: &str, literal_start: usize) -> bool {
    let before = &source[..literal_start.min(source.len())];
    let boundary = before
        .rfind([';', '\n', '{', '}'])
        .map_or(0, |index| index + 1);
    let context = &before[boundary..];
    [
        "boxShadow",
        "box-shadow",
        "textShadow",
        "text-shadow",
        "drop-shadow",
    ]
    .iter()
    .any(|name| {
        context.rfind(name).is_some_and(|index| {
            context[index + name.len()..].contains(':')
                || context[index + name.len()..].contains('=')
        })
    })
}

fn scan_dark_sheet_links(
    file: &SourceFile,
    shell_path: &Option<PathBuf>,
    init_path: &Option<PathBuf>,
    violations: &mut Vec<Violation>,
) {
    let is_shell = Some(&file.path) == shell_path.as_ref();
    let is_init = Some(&file.path) == init_path.as_ref();
    if !is_shell && !is_init {
        for (offset, _) in file.clean.match_indices("tokens-dark.css") {
            add_violation(violations, file, offset, "dark_sheet_link_placement");
        }
        return;
    }
    let tags = html_tags(&file.clean)
        .into_iter()
        .filter(|tag| tag.name == "link")
        .filter(|tag| {
            tag.attributes.iter().any(|attribute| {
                attribute.name == "rel"
                    && attribute
                        .value
                        .split_ascii_whitespace()
                        .any(|value| value == "stylesheet")
            })
        })
        .collect::<Vec<_>>();
    let tokens_index = tags
        .iter()
        .position(|tag| html_href(tag) == Some("/static/tokens.css"));
    let dark_indices = tags
        .iter()
        .enumerate()
        .filter_map(|(index, tag)| {
            (html_href(tag) == Some("/static/tokens-dark.css")).then_some(index)
        })
        .collect::<Vec<_>>();
    for index in &dark_indices {
        let is_stylesheet = tags[*index].attributes.iter().any(|attribute| {
            attribute.name == "rel"
                && attribute
                    .value
                    .split_ascii_whitespace()
                    .any(|value| value == "stylesheet")
        });
        if !is_stylesheet || tokens_index.map(|tokens| tokens + 1) != Some(*index) {
            add_violation(
                violations,
                file,
                tags[*index].start,
                "dark_sheet_link_placement",
            );
        }
    }
    if tokens_index
        .and_then(|index| tags.get(index + 1))
        .and_then(html_href)
        != Some("/static/tokens-dark.css")
    {
        add_violation(violations, file, 0, "dark_sheet_missing");
    }
}

fn html_href(tag: &HtmlTag) -> Option<&str> {
    tag.attributes
        .iter()
        .find(|attribute| attribute.name == "href")
        .map(|attribute| attribute.value.as_str())
}

fn add_violation(
    violations: &mut Vec<Violation>,
    file: &SourceFile,
    offset: usize,
    rule: &'static str,
) {
    let line = file.source[..offset.min(file.source.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    violations.push(Violation {
        path: file.relative.clone(),
        line,
        rule,
    });
}

fn inside_ranges(offset: usize, ranges: &[(usize, usize)]) -> bool {
    ranges
        .iter()
        .any(|(start, end)| offset >= *start && offset < *end)
}

fn normalize_violation_dump(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|violation| {
            format!(
                "{}:{} {}",
                violation.path.display(),
                violation.line,
                violation.rule
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn real_tree_is_clean() {
    let violations = scan(&repository_root());
    assert!(
        violations.is_empty(),
        "convey colour violations:\n{}",
        normalize_violation_dump(&violations)
    );
}

#[cfg(test)]
mod mutation_tests {
    use super::*;
    use tempfile::TempDir;

    struct Fixture {
        _temp: TempDir,
        root: PathBuf,
        workspace: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("temporary fixture directory");
            let root = temp.path().to_path_buf();
            let shell_dir = root.join("core/crates/solstone-core-convey-shell/assets/static");
            let init_dir = root.join("core/crates/solstone-core-sol-link/assets");
            let workspace_dir = root.join("core/crates/example-web/assets");
            fs::create_dir_all(&shell_dir).unwrap();
            fs::create_dir_all(&init_dir).unwrap();
            fs::create_dir_all(&workspace_dir).unwrap();
            fs::write(
                shell_dir.join("tokens.css"),
                ":root { --ink: #1A1A1A; --paper: #FFFFFF; --orange: #E8913A; --cream-bright: #FEFCF8; }",
            )
            .unwrap();
            fs::write(
                shell_dir.join("shell.html"),
                "<link rel=\"stylesheet\" href=\"/static/tokens.css\">\n<link rel=\"stylesheet\" href=\"/static/tokens-dark.css\">\n<link rel=\"stylesheet\" href=\"/static/fixture.css\">\n",
            )
            .unwrap();
            fs::write(shell_dir.join("fixture.css"), "").unwrap();
            fs::write(
                init_dir.join("init.html"),
                "<link rel=\"stylesheet\" href=\"/static/tokens.css\">\n<link rel=\"stylesheet\" href=\"/static/tokens-dark.css\">\n",
            )
            .unwrap();
            let workspace = workspace_dir.join("workspace.html");
            fs::write(&workspace, "<main></main>").unwrap();
            Self {
                _temp: temp,
                root,
                workspace,
            }
        }

        fn set_workspace(&self, source: &str) {
            fs::write(&self.workspace, source).unwrap();
        }

        fn set_css(&self, css: &str) {
            self.set_workspace(&format!("<style>\n{css}\n</style>"));
        }

        fn set_js(&self, js: &str) {
            let path = self.workspace.parent().unwrap().join("app.js");
            fs::write(&path, js).unwrap();
            self.set_workspace("<script src=\"app.js\"></script>");
        }

        fn violations(&self) -> Vec<Violation> {
            scan(&self.root)
        }

        fn has_rule(&self, rule: &str) -> bool {
            self.violations()
                .iter()
                .any(|violation| violation.rule == rule)
        }
    }

    #[test]
    fn mutation_color_literal_fails() {
        let fixture = Fixture::new();
        fixture.set_css("body { background: #fff; }");
        assert!(fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_color_literal_passes() {
        let fixture = Fixture::new();
        fixture.set_css("/* #fff */\n#main-content { white-space: nowrap; color: transparent; }");
        assert!(!fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_inline_style_literal_fails() {
        let fixture = Fixture::new();
        fixture.set_workspace("<div style=\"background: #fff\"></div>");
        assert!(fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_svg_presentation_literal_fails() {
        let fixture = Fixture::new();
        fixture.set_workspace("<svg><path fill=\"#fff\"></path></svg>");
        assert!(fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_javascript_comment_passes() {
        let fixture = Fixture::new();
        fixture.set_js("// #fff and rgba(0,0,0,.2)\nnode.style.color = 'var(--ink)';");
        assert!(!fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_html_comment_passes() {
        let fixture = Fixture::new();
        fixture.set_workspace("don't <!-- tokens-dark.css --> <main></main>");
        assert!(!fixture.has_rule("dark_sheet_link_placement"));
    }

    #[test]
    fn mutation_named_color_fails() {
        let fixture = Fixture::new();
        fixture.set_css("body { background: white; color: red; }");
        assert!(fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_color_fallback_fails() {
        let fixture = Fixture::new();
        fixture.set_css(".x { color: var(--ink, #111); }");
        assert!(fixture.has_rule("color_fallback"));
    }

    #[test]
    fn mutation_color_fallback_passes() {
        let fixture = Fixture::new();
        fixture.set_css(".x { color: var(--ink, var(--paper)); padding: var(--touch-min, 44px); }");
        assert!(!fixture.has_rule("color_fallback"));
    }

    #[test]
    fn mutation_undeclared_custom_property_fails() {
        let fixture = Fixture::new();
        fixture.set_css(".x { color: var(--no-such); }");
        assert!(fixture.has_rule("undeclared_custom_property"));
    }

    #[test]
    fn mutation_undeclared_custom_property_passes() {
        let fixture = Fixture::new();
        fixture.set_css(".x { color: var(--ink); }");
        assert!(!fixture.has_rule("undeclared_custom_property"));
    }

    #[test]
    fn mutation_app_static_stylesheet_declaration_passes_when_linked() {
        let fixture = Fixture::new();
        let stylesheet = fixture.workspace.parent().unwrap().join("app.css");
        fs::write(&stylesheet, ":root { --local-width: 72px; }").unwrap();
        fixture.set_workspace(
            "<link rel=\"stylesheet\" href=\"/app/example/static/app.css\"><style>.x { width: var(--local-width); }</style>",
        );
        assert!(!fixture.has_rule("undeclared_custom_property"));
    }

    #[test]
    fn mutation_app_static_stylesheet_declaration_fails_when_unlinked() {
        let fixture = Fixture::new();
        let stylesheet = fixture.workspace.parent().unwrap().join("app.css");
        fs::write(&stylesheet, ":root { --local-width: 72px; }").unwrap();
        fixture.set_workspace("<style>.x { width: var(--local-width); }</style>");
        assert!(fixture.has_rule("undeclared_custom_property"));
    }

    #[test]
    fn mutation_token_redeclaration_fails() {
        let fixture = Fixture::new();
        fixture.set_workspace("<style>:root { --ink: #111111; }</style>");
        assert!(fixture.has_rule("token_redeclaration"));
    }

    #[test]
    fn mutation_token_redeclaration_passes() {
        let fixture = Fixture::new();
        let app = fixture.workspace.parent().unwrap().join("app.js");
        fs::write(app, "container.style.setProperty('--facet-color', color);").unwrap();
        fixture.set_workspace(
            "<style>:root { --muted: var(--ink); }</style><script src=\"app.js\"></script>",
        );
        assert!(!fixture.has_rule("token_redeclaration"));
    }

    #[test]
    fn mutation_inline_token_redeclaration_fails() {
        let fixture = Fixture::new();
        fixture.set_workspace("<div style=\"--ink: #111111\"></div>");
        assert!(fixture.has_rule("token_redeclaration"));
    }

    #[test]
    fn mutation_shadow_literal_fails() {
        let fixture = Fixture::new();
        fixture.set_css(".x { box-shadow: 0 0 2px rgba(15, 23, 42, 0.22); }");
        assert!(fixture.has_rule("shadow_literal"));
    }

    #[test]
    fn mutation_shadow_literal_passes() {
        let fixture = Fixture::new();
        fixture.set_css(".x { box-shadow: 0 1px 2px rgba(0,0,0,.06); text-shadow: 0 1px #000a; }");
        assert!(!fixture.has_rule("shadow_literal"));
    }

    #[test]
    fn mutation_shadow_hex_fails() {
        let fixture = Fixture::new();
        fixture.set_css(".x { box-shadow: 0 1px 2px #403e3410; }");
        assert!(fixture.has_rule("shadow_literal"));
    }

    #[test]
    fn mutation_dom_style_literal_fails() {
        let fixture = Fixture::new();
        fixture.set_js(
            "const c = '#fff'; node.style.color = '#fff'; el({style: {background: '#fff'}}); node.style.setProperty('color', '#fff'); node.style.color = c;",
        );
        assert!(fixture.has_rule("dom_style_literal"));
    }

    #[test]
    fn mutation_dom_style_literal_passes() {
        let fixture = Fixture::new();
        fixture.set_js(
            "node.style.color = 'var(--ink)'; el({style: {background: 'var(--gold)'}}); region.style.setProperty('--entities-facet-color', facet.color); swatch.style.backgroundColor = color;",
        );
        assert!(!fixture.has_rule("dom_style_literal"));
    }

    #[test]
    fn mutation_channel_triplet_fails() {
        let fixture = Fixture::new();
        let tick = char::from(96);
        let script = format!(
            "{tick}rgba({}{}'232,145,58'{}{}, 0.5){tick}",
            '$', '{', '}', ""
        );
        fixture.set_js(&script);
        assert!(fixture.has_rule("channel_triplet"));
    }

    #[test]
    fn mutation_channel_triplet_constant_fails() {
        let fixture = Fixture::new();
        let tick = char::from(96);
        let interpolation = format!("{}{}", '$', '{');
        fixture.set_js(&format!(
            "const WARM_HEATMAP_RGB = '232,145,58'; const css = {tick}rgba({interpolation}WARM_HEATMAP_RGB}}, 0.5){tick};"
        ));
        assert!(fixture.has_rule("channel_triplet"));
    }

    #[test]
    fn mutation_channel_triplet_passes() {
        let fixture = Fixture::new();
        let tick = char::from(96);
        let script = format!(
            "{tick}color-mix(in srgb, var(--orange) {}{{n}}%, transparent){tick}",
            '$'
        );
        fixture.set_js(&script);
        assert!(!fixture.has_rule("channel_triplet"));
        assert!(!fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_canvas_var_string_fails() {
        let fixture = Fixture::new();
        fixture.set_js("ctx.fillStyle = 'var(--ink)';");
        assert!(fixture.has_rule("canvas_var_string"));
    }

    #[test]
    fn mutation_computed_color_read_passes() {
        let fixture = Fixture::new();
        fixture.set_js(
            "ctx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue('--ink').trim() || '#1a1a1a'; ctx.strokeStyle = getComputedStyle(document.documentElement).getPropertyValue('--paper').trim() || '#fff';",
        );
        assert!(!fixture.has_rule("computed_color_read"));
        assert!(!fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_computed_color_read_wrong_fallback_fails() {
        let fixture = Fixture::new();
        fixture.set_js(
            "ctx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue('--ink').trim() || '#ff00ff';",
        );
        assert!(fixture.has_rule("computed_color_read"));
    }

    #[test]
    fn mutation_computed_color_read_to_dom_fails() {
        let fixture = Fixture::new();
        fixture.set_js(
            "node.style.color = getComputedStyle(document.documentElement).getPropertyValue('--ink').trim() || '#1a1a1a';",
        );
        assert!(fixture.has_rule("computed_color_read"));
    }

    #[test]
    fn mutation_canvas_direct_literal_fails() {
        let fixture = Fixture::new();
        fixture.set_js("ctx.fillStyle = '#1a1a1a';");
        assert!(fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_function_allowlist_passes_inside() {
        let fixture = Fixture::new();
        fixture.set_workspace(
            "<script>class FrameCapture { _applyOverlays() { ctx.fillStyle = '#fff'; } }</script>",
        );
        assert!(!fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_function_allowlist_fails_after() {
        let fixture = Fixture::new();
        fixture.set_workspace(
            "<script>class FrameCapture { _applyOverlays() {} }\nctx.fillStyle = '#fff';</script>",
        );
        assert!(fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_pairing_file_allowlist_passes() {
        let fixture = Fixture::new();
        let path = fixture.workspace.parent().unwrap().join("pairing-qr.js");
        fs::write(&path, "ctx.fillStyle = '#fff';").unwrap();
        assert!(!fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_pairing_file_allowlist_fails_elsewhere() {
        let fixture = Fixture::new();
        fixture.set_js("ctx.fillStyle = '#fff';");
        assert!(fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_sunarc_file_allowlist_passes() {
        let fixture = Fixture::new();
        let path = fixture.workspace.parent().unwrap().join("sunarc.js");
        fs::write(&path, "ctx.fillStyle = '#fff';").unwrap();
        assert!(!fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_sunarc_file_allowlist_fails_elsewhere() {
        let fixture = Fixture::new();
        fixture.set_js("ctx.fillStyle = '#fff';");
        assert!(fixture.has_rule("color_literal"));
    }

    #[test]
    fn mutation_color_scheme_declaration_fails() {
        let fixture = Fixture::new();
        fixture.set_css(":root { color-scheme: light; }");
        assert!(fixture.has_rule("color_scheme_declaration"));
    }

    #[test]
    fn mutation_color_scheme_declaration_passes() {
        let fixture = Fixture::new();
        fixture.set_css(
            "/* color-scheme: light */\n@media (prefers-color-scheme: dark) { body { color: var(--ink); } }",
        );
        assert!(!fixture.has_rule("color_scheme_declaration"));
    }

    #[test]
    fn mutation_dark_sheet_link_placement_fails_in_workspace() {
        let fixture = Fixture::new();
        fixture.set_workspace(
            "<link rel=\"stylesheet\" href=\"/static/tokens-dark.css\"><main></main>",
        );
        assert!(fixture.has_rule("dark_sheet_link_placement"));
    }

    #[test]
    fn mutation_dark_sheet_link_placement_fails_when_not_next() {
        let fixture = Fixture::new();
        let shell = fixture
            .root
            .join("core/crates/solstone-core-convey-shell/assets/static/shell.html");
        fs::write(
            shell,
            "<link rel=\"stylesheet\" href=\"/static/tokens.css\"><link rel=\"stylesheet\" href=\"/static/fixture.css\"><link rel=\"stylesheet\" href=\"/static/tokens-dark.css\">",
        )
        .unwrap();
        assert!(fixture.has_rule("dark_sheet_link_placement"));
    }

    #[test]
    fn mutation_dark_sheet_reference_in_javascript_fails() {
        let fixture = Fixture::new();
        fixture.set_js("const sheet = 'tokens-dark.css';");
        assert!(fixture.has_rule("dark_sheet_link_placement"));
    }

    #[test]
    fn mutation_dark_sheet_init_missing_fails() {
        let fixture = Fixture::new();
        let init = fixture
            .root
            .join("core/crates/solstone-core-sol-link/assets/init.html");
        fs::write(
            init,
            "<link rel=\"stylesheet\" href=\"/static/tokens.css\">",
        )
        .unwrap();
        assert!(fixture.has_rule("dark_sheet_missing"));
    }

    #[test]
    fn mutation_dark_sheet_missing_fails() {
        let fixture = Fixture::new();
        let shell = fixture
            .root
            .join("core/crates/solstone-core-convey-shell/assets/static/shell.html");
        fs::write(
            shell,
            "<link rel=\"stylesheet\" href=\"/static/tokens.css\">",
        )
        .unwrap();
        assert!(fixture.has_rule("dark_sheet_missing"));
    }

    #[test]
    fn mutation_dark_sheet_link_placement_passes_when_next() {
        let fixture = Fixture::new();
        assert!(!fixture.has_rule("dark_sheet_link_placement"));
    }

    #[test]
    fn mutation_set_property_token_redeclaration_fails() {
        let fixture = Fixture::new();
        fixture.set_js("node.style.setProperty('--ink', 'var(--ink)');");
        assert!(fixture.has_rule("token_redeclaration"));
    }

    #[test]
    fn mutation_set_property_local_numeric_passes() {
        let fixture = Fixture::new();
        fixture.set_js(
            "node.style.setProperty('--entities-facet-color', facet.color); node.style.setProperty('--bar-fill', '40%');",
        );
        assert!(!fixture.has_rule("token_redeclaration"));
        assert!(!fixture.has_rule("dom_style_literal"));
    }
}
