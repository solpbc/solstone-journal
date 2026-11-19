// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Cursor;

use exif::{Exif, In, Reader, Tag, Value};
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const XMP_LIMIT: usize = 256 * 1024;
const XMP_EXIF: &[u8] = b"http://ns.adobe.com/exif/1.0/";
const RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";

pub(super) struct ImageMetadata {
    pub camera: bool,
    pub screenshot_comment: bool,
    pub android_software: bool,
    pub gnome_software: bool,
}

impl ImageMetadata {
    pub(super) fn read(bytes: &[u8]) -> Self {
        let exif = Reader::new()
            .read_from_container(&mut Cursor::new(bytes))
            .ok();
        let field = |tag| {
            exif.as_ref()
                .and_then(|data| data.get_field(tag, In::PRIMARY))
                .map(|field| &field.value)
        };
        let has_make = exif_text(exif.as_ref(), Tag::Make).any(nonblank);
        let camera = has_make
            && exif_text(exif.as_ref(), Tag::Model).any(nonblank)
            && [
                Tag::ExposureTime,
                Tag::FNumber,
                Tag::PhotographicSensitivity,
            ]
            .into_iter()
            .any(|tag| field(tag).is_some());
        let mut metadata = Self {
            camera,
            screenshot_comment: matches!(
                field(Tag::UserComment),
                Some(Value::Undefined(value, _))
                    if value.strip_prefix(b"ASCII\0\0\0")
                        .is_some_and(|text| trim_nul(text) == b"Screenshot")
            ) || exif_text(exif.as_ref(), Tag::UserComment)
                .any(|value| value == b"Screenshot"),
            android_software: !has_make
                && exif_text(exif.as_ref(), Tag::Software).any(android_build),
            gnome_software: false,
        };
        if camera {
            return metadata;
        }

        if let Some(mut data) = bytes.strip_prefix(PNG_MAGIC) {
            while let Some((kind, content, rest)) = png_chunk(data) {
                if kind == b"IEND" {
                    break;
                }
                if let Some((key, value)) = png_text(kind, content) {
                    match key {
                        b"Software" => {
                            metadata.gnome_software |= value == b"gnome-screenshot";
                            metadata.android_software |= !has_make && android_build(value);
                        }
                        b"XML:com.adobe.xmp" => {
                            metadata.screenshot_comment |= xmp_screenshot(value);
                        }
                        _ => {}
                    }
                }
                data = rest;
            }
        } else if let Some(xmp) = jpeg_xmp(bytes).or_else(|| webp_xmp(bytes)) {
            metadata.screenshot_comment |= xmp_screenshot(xmp);
        }
        metadata
    }
}

fn exif_text(exif: Option<&Exif>, tag: Tag) -> impl Iterator<Item = &[u8]> {
    exif.and_then(|data| data.get_field(tag, In::PRIMARY))
        .and_then(|field| match &field.value {
            Value::Ascii(values) => Some(values),
            _ => None,
        })
        .into_iter()
        .flatten()
        .map(Vec::as_slice)
}

fn trim_nul(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |i| i + 1);
    &bytes[..end]
}

fn nonblank(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|&byte| byte != 0 && !byte.is_ascii_whitespace())
}

fn android_build(value: &[u8]) -> bool {
    value.strip_prefix(b"Android ").is_some_and(nonblank)
}

fn png_chunk(data: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let (length, data) = data.split_first_chunk::<4>()?;
    let (kind, data) = data.split_first_chunk::<4>()?;
    let (content, rest) = data.split_at_checked(u32::from_be_bytes(*length) as usize)?;
    Some((kind, content, rest.get(4..)?))
}

fn png_text<'a>(kind: &[u8], data: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
    if !matches!(kind, b"tEXt" | b"iTXt") || data.len() > XMP_LIMIT {
        return None;
    }
    let (key, value) = split_nul(data)?;
    match kind {
        b"tEXt" => Some((key, value)),
        b"iTXt" => {
            let value = value.strip_prefix(&[0, 0])?;
            let (_, value) = split_nul(value)?;
            let (_, text) = split_nul(value)?;
            Some((key, text))
        }
        _ => None,
    }
}

fn split_nul(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let index = data.iter().position(|&byte| byte == 0)?;
    Some((&data[..index], &data[index + 1..]))
}

fn jpeg_xmp(bytes: &[u8]) -> Option<&[u8]> {
    let mut data = bytes.strip_prefix(&[0xff, 0xd8])?;
    while let Some(rest) = data.strip_prefix(&[0xff]) {
        let marker = rest.iter().position(|&byte| byte != 0xff)?;
        let (&marker, segment) = rest[marker..].split_first()?;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        let (length, _) = segment.split_first_chunk::<2>()?;
        let length = usize::from(u16::from_be_bytes(*length));
        let (body, rest) = segment.split_at_checked(length)?;
        if marker == 0xe1
            && let Some(xmp) = body
                .get(2..)?
                .strip_prefix(b"http://ns.adobe.com/xap/1.0/\0")
        {
            return Some(xmp);
        }
        if length < 2 {
            return None;
        }
        data = rest;
    }
    None
}

fn webp_xmp(bytes: &[u8]) -> Option<&[u8]> {
    let (size, data) = bytes.strip_prefix(b"RIFF")?.split_first_chunk::<4>()?;
    let mut data = data
        .get(..u32::from_le_bytes(*size) as usize)?
        .strip_prefix(b"WEBP")?;
    while let Some((kind, rest)) = data.split_first_chunk::<4>() {
        let (size, rest) = rest.split_first_chunk::<4>()?;
        let size = u32::from_le_bytes(*size) as usize;
        let (body, rest) = rest.split_at_checked(size)?;
        data = rest.get(size % 2..)?;
        if kind == b"XMP " {
            return Some(body);
        }
    }
    None
}

fn xmp_screenshot(bytes: &[u8]) -> bool {
    if bytes.len() > XMP_LIMIT {
        return false;
    }
    let mut xml = NsReader::from_reader(bytes);
    loop {
        match xml.read_resolved_event() {
            Ok((ResolveResult::Bound(ns), Event::Start(tag)))
                if ns.as_ref() == XMP_EXIF && tag.local_name().as_ref() == b"UserComment" =>
            {
                return xmp_user_comment(&mut xml);
            }
            Ok((_, Event::Eof)) | Err(_) => return false,
            _ => {}
        }
    }
}

fn xmp_user_comment(xml: &mut NsReader<&[u8]>) -> bool {
    match next_xml_event(xml) {
        Ok(Event::Text(value)) => {
            &value[..] == b"Screenshot" && matches!(next_xml_event(xml), Ok(Event::End(_)))
        }
        Ok(Event::Start(tag))
            if tag.local_name().as_ref() == b"Alt"
                && matches!(xml.resolver().resolve_element(tag.name()).0, ResolveResult::Bound(ns) if ns.as_ref() == RDF) =>
        {
            matches!(
                next_xml_event(xml),
                Ok(Event::Start(tag)) if tag.local_name().as_ref() == b"li"
                    && matches!(xml.resolver().resolve_element(tag.name()).0, ResolveResult::Bound(ns) if ns.as_ref() == RDF)
            ) && matches!(next_xml_event(xml), Ok(Event::Text(value)) if &value[..] == b"Screenshot")
                && (0..3).all(|_| matches!(next_xml_event(xml), Ok(Event::End(_))))
        }
        _ => false,
    }
}

fn next_xml_event<'a>(xml: &mut NsReader<&'a [u8]>) -> quick_xml::Result<Event<'a>> {
    loop {
        let event = xml.read_event()?;
        if !matches!(&event, Event::Text(value) if value.iter().all(u8::is_ascii_whitespace)) {
            return Ok(event);
        }
    }
}
