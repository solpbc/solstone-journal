// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{
    fonts::{Face, advance_width, encode_winansi, winansi_glyph_name},
    markdown::Block,
};

const PAGE_WIDTH: f32 = 612.0;
const PAGE_HEIGHT: f32 = 792.0;
const MARGIN: f32 = 54.0;

pub(crate) fn render(blocks: &[Block]) -> Vec<u8> {
    let stream = content_stream(blocks);
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R /F2 5 0 R /F3 6 0 R /F4 7 0 R /F5 8 0 R >> >> /Contents 9 0 R >>".to_vec(),
        font_object("Times-Roman"),
        font_object("Times-Bold"),
        font_object("Times-Italic"),
        font_object("Times-BoldItalic"),
        font_object("Courier"),
        stream_object(stream),
    ];
    assemble_pdf(&objects)
}

fn font_object(name: &str) -> Vec<u8> {
    format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>")
        .into_bytes()
}

fn content_stream(blocks: &[Block]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut y = PAGE_HEIGHT - MARGIN;
    for block in blocks {
        let mut x = MARGIN + block.indent;
        let line_start = x;
        for run in &block.runs {
            for piece in run.text.split_inclusive('\n') {
                let ends_line = piece.ends_with('\n');
                let piece = piece.strip_suffix('\n').unwrap_or(piece);
                if piece.is_empty() {
                    y -= block.leading;
                    x = line_start;
                    continue;
                }
                let mut first_word = true;
                let starts_with_space = piece.starts_with(char::is_whitespace);
                for word in piece.split_whitespace() {
                    let leading = if starts_with_space || !first_word {
                        text_width(run.face, run.size, &encode_winansi(" "))
                    } else {
                        0.0
                    };
                    let bytes = encode_winansi(word);
                    let width = text_width(run.face, run.size, &bytes);
                    if x > line_start && x + leading + width > PAGE_WIDTH - MARGIN {
                        y -= block.leading;
                        x = line_start;
                    } else {
                        x += leading;
                    }
                    emit_text(&mut output, run.face, run.size, x, y, &bytes);
                    x += width;
                    first_word = false;
                }
                if piece.ends_with(char::is_whitespace) && !piece.trim().is_empty() {
                    let bytes = encode_winansi(" ");
                    emit_text(&mut output, run.face, run.size, x, y, &bytes);
                    x += text_width(run.face, run.size, &bytes);
                }
                if ends_line {
                    y -= block.leading;
                    x = line_start;
                }
            }
        }
        y -= block.leading + block.after;
    }
    output
}

fn text_width(face: Face, size: f32, bytes: &[u8]) -> f32 {
    bytes
        .iter()
        .map(|byte| advance_width(face, *byte) as f32 * size / 1000.0)
        .sum()
}

fn emit_text(output: &mut Vec<u8>, face: Face, size: f32, x: f32, y: f32, bytes: &[u8]) {
    output.extend_from_slice(
        format!(
            "BT /{} {:.2} Tf 1 0 0 1 {:.2} {:.2} Tm (",
            face.resource_name(),
            size,
            x,
            y
        )
        .as_bytes(),
    );
    for byte in bytes {
        debug_assert!(
            byte.is_ascii() || *byte >= 0xa0 || winansi_glyph_name(*byte).is_some(),
            "encoded byte must have a WinAnsi glyph"
        );
        match byte {
            b'(' | b')' | b'\\' => {
                output.push(b'\\');
                output.push(*byte);
            }
            0..=31 => output.extend_from_slice(format!("\\{:03o}", byte).as_bytes()),
            _ => output.push(*byte),
        }
    }
    output.extend_from_slice(b") Tj ET\n");
}

fn stream_object(stream: Vec<u8>) -> Vec<u8> {
    let mut object = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
    object.extend_from_slice(&stream);
    object.extend_from_slice(b"endstream");
    object
}

fn assemble_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

#[cfg(test)]
pub(crate) fn extract_text_checked(bytes: &[u8]) -> String {
    pdf_extract::extract_text_from_mem(bytes).expect("pdf-extract must extract generated PDF")
}

#[cfg(test)]
mod tests {
    use super::extract_text_checked;
    use crate::pdf::render as render_newsletter;

    #[test]
    fn winansi_marks_unrepresentable_text() {
        let pdf = render_newsletter("em — ‘curly’ \"quotes\" · A😀B", "work", "Sun May 10, 2026");
        let text = extract_text_checked(&pdf);
        assert!(text.contains("— ‘curly’ \"quotes\" · A[?]B"), "{text:?}");
    }
}
