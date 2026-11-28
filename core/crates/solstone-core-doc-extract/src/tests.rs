// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Cursor, Write};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::{
    BackendOutcome, DocumentFormat, ExtractError, ExtractLimits, ExtractOutcome, FormatBackend,
    RawExtraction, extract_bytes, extract_with_backend, finalize_extraction, sanitize_warning,
    strip_upstream_notices,
};

fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, opts).expect("start zip entry");
            writer.write_all(data).expect("write zip entry");
        }
        writer.finish().expect("finish zip");
    }
    buf.into_inner()
}

fn build_docx(body_xml: &str, title: Option<&str>) -> Vec<u8> {
    let mut entries: Vec<(&str, Vec<u8>)> = vec![
        (
            "[Content_Types].xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#
                .to_vec(),
        ),
        (
            "_rels/.rels",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>"#
                .to_vec(),
        ),
        (
            "word/document.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>{body_xml}</w:body>
</w:document>"#
            )
            .into_bytes(),
        ),
    ];
    if let Some(title) = title {
        entries.push((
            "docProps/core.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
  xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:title>{title}</dc:title>
</cp:coreProperties>"#
            )
            .into_bytes(),
        ));
    }
    let borrowed: Vec<(&str, &[u8])> = entries.iter().map(|(k, v)| (*k, v.as_slice())).collect();
    build_zip(&borrowed)
}

fn build_xlsx(sheet_name: &str, shared_strings: &[&str], sheet_rows_xml: &str) -> Vec<u8> {
    let mut sst_items = String::new();
    for s in shared_strings {
        sst_items.push_str("<si><t>");
        sst_items.push_str(s);
        sst_items.push_str("</t></si>");
    }
    let sst = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{}" uniqueCount="{}">{sst_items}</sst>"#,
        shared_strings.len(),
        shared_strings.len()
    );
    let styles = r#"<?xml version="1.0" encoding="UTF-8"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <cellXfs count="2">
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0"/>
    <xf numFmtId="10" fontId="0" fillId="0" borderId="0" applyNumberFormat="1"/>
  </cellXfs>
</styleSheet>"#;
    let workbook = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="{sheet_name}" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#
    );
    let sheet = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>{sheet_rows_xml}</sheetData>
</worksheet>"#
    );
    build_zip(&[
        (
            "[Content_Types].xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
  <Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
</Types>"#,
        ),
        (
            "_rels/.rels",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        ),
        ("xl/workbook.xml", workbook.as_bytes()),
        (
            "xl/_rels/workbook.xml.rels",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#,
        ),
        ("xl/sharedStrings.xml", sst.as_bytes()),
        ("xl/styles.xml", styles.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
    ])
}

fn biff12_varint(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
}

fn biff12_rec(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // Record type encoding: 1 byte if < 128, 2 bytes with high bit set if >= 128.
    if id < 128 {
        out.push(id as u8);
    } else {
        out.push(((id & 0x7F) | 0x80) as u8);
        out.push((id >> 7) as u8);
    }
    biff12_varint(payload.len() as u32, &mut out);
    out.extend_from_slice(payload);
    out
}

fn biff12_wide(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut out = (units.len() as u32).to_le_bytes().to_vec();
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

fn build_xlsb(sheet_name: &str, cell_text: &str) -> Vec<u8> {
    const BRT_ROW_HDR: u32 = 0;
    const BRT_CELL_ST: u32 = 6;
    const BRT_BUNDLE_SH: u32 = 156;

    let mut sh = 0u32.to_le_bytes().to_vec();
    sh.extend_from_slice(&1u32.to_le_bytes());
    sh.extend(biff12_wide("rId1"));
    sh.extend(biff12_wide(sheet_name));
    let wb = biff12_rec(BRT_BUNDLE_SH, &sh);

    let mut cell = Vec::new();
    cell.extend_from_slice(&0u32.to_le_bytes()); // column 0
    cell.extend_from_slice(&0u32.to_le_bytes()); // style + ph
    cell.extend(biff12_wide(cell_text));

    let mut sheet = biff12_rec(BRT_ROW_HDR, &[0u8; 17]);
    sheet.extend(biff12_rec(BRT_CELL_ST, &cell));

    let rels = br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.bin"/>
</Relationships>"#;

    build_zip(&[
        ("xl/workbook.bin", &wb),
        ("xl/_rels/workbook.bin.rels", rels),
        ("xl/worksheets/sheet1.bin", &sheet),
    ])
}

fn build_pptx(slides: &[(&str, Option<&str>)]) -> Vec<u8> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut ct_overrides = String::new();
    let mut pres_rels = String::new();
    let mut sld_id_lst = String::new();

    for (idx, (slide_xml, notes_text)) in slides.iter().enumerate() {
        let num = idx + 1;
        let rid = format!("rId{num}");
        ct_overrides.push_str(&format!(
            r#"<Override PartName="/ppt/slides/slide{num}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
        ));
        pres_rels.push_str(&format!(
            r#"<Relationship Id="{rid}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{num}.xml"/>"#
        ));
        sld_id_lst.push_str(&format!(r#"<p:sldId id="{}" r:id="{rid}"/>"#, 256 + idx));
        entries.push((
            format!("ppt/slides/slide{num}.xml"),
            slide_xml.as_bytes().to_vec(),
        ));

        if let Some(notes) = notes_text {
            ct_overrides.push_str(&format!(
                r#"<Override PartName="/ppt/notesSlides/notesSlide{num}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml"/>"#
            ));
            entries.push((
                format!("ppt/slides/_rels/slide{num}.xml.rels"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide{num}.xml"/>
</Relationships>"#
                )
                .into_bytes(),
            ));
            entries.push((
                format!("ppt/notesSlides/notesSlide{num}.xml"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree><p:sp>
    <p:nvSpPr><p:cNvPr id="3" name="Notes"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>
    <p:txBody><a:bodyPr/><a:p><a:r><a:t>{notes}</a:t></a:r></a:p></p:txBody>
  </p:sp></p:spTree></p:cSld>
</p:notes>"#
                )
                .into_bytes(),
            ));
        }
    }

    let content_types = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  {ct_overrides}
</Types>"#
    );
    let root_rels = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#;
    let presentation = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst>{sld_id_lst}</p:sldIdLst>
</p:presentation>"#
    );
    let pres_rels_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{pres_rels}</Relationships>"#
    );

    entries.push(("[Content_Types].xml".to_owned(), content_types.into_bytes()));
    entries.push(("_rels/.rels".to_owned(), root_rels.as_bytes().to_vec()));
    entries.push(("ppt/presentation.xml".to_owned(), presentation.into_bytes()));
    entries.push((
        "ppt/_rels/presentation.xml.rels".to_owned(),
        pres_rels_xml.into_bytes(),
    ));

    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    build_zip(&borrowed)
}

fn slide_xml(title: &str, bullets: &[&str]) -> String {
    let mut paragraphs = format!("<a:p><a:r><a:t>{title}</a:t></a:r></a:p>");
    for b in bullets {
        paragraphs.push_str(&format!("<a:p><a:r><a:t>{b}</a:t></a:r></a:p>"));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/>{paragraphs}</p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#
    )
}

/// Build a valid MS-CFB v3 file holding up to 3 named top-level streams (each
/// stored in normal 512-byte sectors so mini-stream cutoff is avoided cleanly).
fn build_cfb(streams: &[(&str, &[u8])]) -> Vec<u8> {
    const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
    const FATSECT: u32 = 0xFFFF_FFFD;
    const FREESECT: u32 = 0xFFFF_FFFF;
    const NOSTREAM: u32 = 0xFFFF_FFFF;

    assert!(
        streams.len() <= 3,
        "test CFB builder supports up to 3 streams"
    );

    // Sector 0 = FAT, Sector 1 = Directory (4 x 128B slots), Sectors 2.. = streams.
    // Pad stream data to >= 4096 bytes (8 sectors) if needed or set mini_cutoff = 0
    // so every stream reads directly from regular FAT sectors.
    let mut stream_sectors: Vec<(u32, usize)> = Vec::new();
    let mut next_sector: u32 = 2;
    let mut fat: Vec<u32> = vec![FATSECT, ENDOFCHAIN];

    for (_, data) in streams {
        let count = data.len().div_ceil(512).max(1);
        let start = next_sector;
        for i in 0..count {
            let s = next_sector + i as u32;
            let link = if i + 1 < count { s + 1 } else { ENDOFCHAIN };
            fat.push(link);
        }
        next_sector += count as u32;
        stream_sectors.push((start, data.len()));
    }
    assert!(
        fat.len() <= 128,
        "test CFB builder uses a single FAT sector"
    );

    let total_sectors = next_sector as usize;
    let mut file = vec![0u8; 512 * (1 + total_sectors)];
    // Header
    file[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    file[0x18..0x1A].copy_from_slice(&0x003Eu16.to_le_bytes());
    file[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    file[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    file[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    file[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    file[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes()); // num FAT sectors
    file[0x30..0x34].copy_from_slice(&1u32.to_le_bytes()); // first dir sector = 1
    file[0x38..0x3C].copy_from_slice(&0u32.to_le_bytes()); // mini stream cutoff = 0 (all regular FAT)
    file[0x3C..0x40].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    file[0x44..0x48].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    file[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes()); // DIFAT[0] = sector 0
    for i in 1..109 {
        file[0x4C + i * 4..0x50 + i * 4].copy_from_slice(&FREESECT.to_le_bytes());
    }

    // FAT at sector 0 (offset 512)
    for i in 0..128 {
        let val = fat.get(i).copied().unwrap_or(FREESECT);
        file[512 + i * 4..512 + i * 4 + 4].copy_from_slice(&val.to_le_bytes());
    }

    // Directory at sector 1 (offset 1024)
    let write_dir =
        |buf: &mut [u8], name: &str, kind: u8, right: u32, child: u32, start: u32, size: u32| {
            let units: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            for (i, u) in units.iter().enumerate() {
                buf[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
            }
            buf[0x40..0x42].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
            buf[0x42] = kind;
            buf[0x43] = 1; // black
            buf[0x44..0x48].copy_from_slice(&NOSTREAM.to_le_bytes());
            buf[0x48..0x4C].copy_from_slice(&right.to_le_bytes());
            buf[0x4C..0x50].copy_from_slice(&child.to_le_bytes());
            buf[0x74..0x78].copy_from_slice(&start.to_le_bytes());
            buf[0x78..0x7C].copy_from_slice(&size.to_le_bytes());
        };

    let root_child = if streams.is_empty() { NOSTREAM } else { 1 };
    write_dir(
        &mut file[1024..1152],
        "Root Entry",
        5,
        NOSTREAM,
        root_child,
        ENDOFCHAIN,
        0,
    );

    for (idx, (name, data)) in streams.iter().enumerate() {
        let slot = idx + 1;
        let right = if slot < streams.len() {
            (slot + 1) as u32
        } else {
            NOSTREAM
        };
        let (start, size) = stream_sectors[idx];
        let dir_off = 1024 + slot * 128;
        write_dir(
            &mut file[dir_off..dir_off + 128],
            name,
            2,
            right,
            NOSTREAM,
            start,
            size as u32,
        );
        let data_off = 512 * (1 + start as usize);
        file[data_off..data_off + data.len()].copy_from_slice(data);
    }

    file
}

fn build_legacy_doc(text: &str, encrypted: bool) -> Vec<u8> {
    // Minimal Word 97 FIB + compressed ANSI piece table in 1Table.
    let mut word_doc = vec![0u8; 2048];
    word_doc[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes()); // wIdent
    word_doc[2..4].copy_from_slice(&0x00C1u16.to_le_bytes()); // nFib
    let mut flags: u16 = 1 << 9; // fWhichTblStm = 1Table
    if encrypted {
        flags |= 1 << 8; // fEncrypted
    }
    word_doc[0x0A..0x0C].copy_from_slice(&flags.to_le_bytes());

    let text_bytes: Vec<u8> = text
        .as_bytes()
        .iter()
        .copied()
        .chain(std::iter::once(b'\r'))
        .collect();
    let cp_len = text_bytes.len() as u32;
    word_doc[0x4C..0x50].copy_from_slice(&cp_len.to_le_bytes()); // ccpText

    // Place text at offset 1024 inside WordDocument stream.
    let text_offset: u32 = 1024;
    word_doc[text_offset as usize..text_offset as usize + text_bytes.len()]
        .copy_from_slice(&text_bytes);

    // Clx in 1Table: 0x02 (Pcdt), u32 lcb=16, PlcfPcd: cp[0]=0, cp[1]=cp_len, Pcd(8 bytes)
    let mut table1 = Vec::new();
    table1.push(0x02);
    table1.extend_from_slice(&16u32.to_le_bytes());
    table1.extend_from_slice(&0u32.to_le_bytes());
    table1.extend_from_slice(&cp_len.to_le_bytes());
    // Pcd: 2B flags, 4B fc (compressed ANSI: fc = (offset * 2) | 0x4000_0000), 2B prm
    table1.extend_from_slice(&0u16.to_le_bytes());
    let fc_compressed = (text_offset * 2) | 0x4000_0000;
    table1.extend_from_slice(&fc_compressed.to_le_bytes());
    table1.extend_from_slice(&0u16.to_le_bytes());

    word_doc[0x01A2..0x01A6].copy_from_slice(&0u32.to_le_bytes()); // fcClx = 0
    word_doc[0x01A6..0x01AA].copy_from_slice(&(table1.len() as u32).to_le_bytes()); // lcbClx

    build_cfb(&[("WordDocument", &word_doc), ("1Table", &table1)])
}

fn biff8_rec(rec_type: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&rec_type.to_le_bytes());
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn build_legacy_xls(sheet_name: &str, cell_text: &str, encrypted: bool) -> Vec<u8> {
    // BIFF8 Workbook globals + single worksheet with LABEL record.
    let mut globals = Vec::new();
    // BOF (0x0809): BIFF8 (0x0600), workbook globals (0x0005)
    let mut bof_glob = vec![0u8; 16];
    bof_glob[0..2].copy_from_slice(&0x0600u16.to_le_bytes());
    bof_glob[2..4].copy_from_slice(&0x0005u16.to_le_bytes());
    globals.extend(biff8_rec(0x0809, &bof_glob));

    if encrypted {
        // FILEPASS (0x002F)
        globals.extend(biff8_rec(0x002F, &[0x01, 0x00, 0x01, 0x00, 0x01, 0x00]));
    }

    // BOUNDSHEET (0x0085): placeholder offset (patched below), flags=0, short-string sheet_name
    let boundsheet_offset_pos = globals.len() + 4;
    let mut bs = Vec::new();
    bs.extend_from_slice(&0u32.to_le_bytes()); // lbPlyPos placeholder
    bs.extend_from_slice(&0u16.to_le_bytes()); // visibility + worksheet type
    bs.push(sheet_name.len() as u8);
    bs.push(0u8); // compressed 8-bit
    bs.extend_from_slice(sheet_name.as_bytes());
    globals.extend(biff8_rec(0x0085, &bs));
    globals.extend(biff8_rec(0x000A, &[])); // EOF

    let sheet_start = globals.len() as u32;
    globals[boundsheet_offset_pos..boundsheet_offset_pos + 4]
        .copy_from_slice(&sheet_start.to_le_bytes());

    // Worksheet stream: BOF (0x0809, type 0x0010), LABEL (0x0204), EOF (0x000A)
    let mut sheet = Vec::new();
    let mut bof_ws = vec![0u8; 16];
    bof_ws[0..2].copy_from_slice(&0x0600u16.to_le_bytes());
    bof_ws[2..4].copy_from_slice(&0x0010u16.to_le_bytes());
    sheet.extend(biff8_rec(0x0809, &bof_ws));

    let mut label = Vec::new();
    label.extend_from_slice(&0u16.to_le_bytes()); // row 0
    label.extend_from_slice(&0u16.to_le_bytes()); // col 0
    label.extend_from_slice(&0u16.to_le_bytes()); // xf 0
    label.extend_from_slice(&(cell_text.len() as u16).to_le_bytes());
    label.push(0u8); // 8-bit
    label.extend_from_slice(cell_text.as_bytes());
    sheet.extend(biff8_rec(0x0204, &label));
    sheet.extend(biff8_rec(0x000A, &[]));

    let mut workbook = globals;
    workbook.extend(sheet);
    build_cfb(&[("Workbook", &workbook)])
}

fn ppt_record(ver_inst: u16, rec_type: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&ver_inst.to_le_bytes());
    out.extend_from_slice(&rec_type.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn build_legacy_ppt(slide_text: &str, encrypted: bool) -> Vec<u8> {
    if encrypted {
        // Current User stream with headerToken = 0xF3D1C4DF (encrypted PPT).
        let mut cu_payload = vec![0u8; 24];
        cu_payload[0..4].copy_from_slice(&20u32.to_le_bytes()); // size
        cu_payload[4..8].copy_from_slice(&0xF3D1_C4DFu32.to_le_bytes()); // encrypted token
        let cu = ppt_record(0x0000, 0x0FF6, &cu_payload);
        let dummy_doc = ppt_record(0x000F, 0x03E8, &[]);
        return build_cfb(&[("Current User", &cu), ("PowerPoint Document", &dummy_doc)]);
    }

    // Minimal SlideContainer (0x03EE) with TextHeaderAtom (0x0F9F) + TextBytesAtom (0x0FA8).
    let text_header = ppt_record(0x0000, 0x0F9F, &0u32.to_le_bytes()); // title
    let text_bytes = ppt_record(0x0000, 0x0FA8, slide_text.as_bytes());
    let mut drawing_children = Vec::new();
    drawing_children.extend(text_header);
    drawing_children.extend(text_bytes);
    let client_textbox = ppt_record(0x000F, 0xF00D, &drawing_children);
    let slide = ppt_record(0x000F, 0x03EE, &client_textbox);

    build_cfb(&[("PowerPoint Document", &slide)])
}

fn build_encrypted_ooxml_cfb() -> Vec<u8> {
    let mut enc_info = vec![0u8; 64];
    enc_info[0..2].copy_from_slice(&4u16.to_le_bytes());
    enc_info[2..4].copy_from_slice(&4u16.to_le_bytes());
    enc_info[4..8].copy_from_slice(&0x40u32.to_le_bytes());
    enc_info[8..21].copy_from_slice(b"<encryption/>");
    let enc_pkg = vec![0xAAu8; 64];
    build_cfb(&[
        ("EncryptionInfo", &enc_info),
        ("EncryptedPackage", &enc_pkg),
    ])
}

#[test]
fn extracts_docx_structure_tables_title_and_docm() {
    let body = r#"
      <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly Plan</w:t></w:r></w:p>
      <w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold world</w:t></w:r></w:p>
      <w:tbl>
        <w:tr>
          <w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc>
          <w:tc><w:p><w:r><w:t>Status</w:t></w:r></w:p></w:tc>
        </w:tr>
        <w:tr>
          <w:tc><w:p><w:r><w:t>Engine</w:t></w:r></w:p></w:tc>
          <w:tc><w:p><w:r><w:t>Ready</w:t></w:r></w:p></w:tc>
        </w:tr>
      </w:tbl>
    "#;
    let bytes = build_docx(body, Some("Q4 Planning Brief"));
    let out = extract_bytes(&bytes, Some("plan.docx"), &ExtractLimits::desktop())
        .expect("docx extraction should succeed");
    let ExtractOutcome::Extracted(doc) = out else {
        panic!("expected Extracted, got {out:?}");
    };
    assert_eq!(doc.format, DocumentFormat::Docx);
    assert_eq!(doc.title.as_deref(), Some("Q4 Planning Brief"));
    assert!(!doc.truncated);
    assert!(
        doc.markdown.contains("Quarterly Plan"),
        "md: {}",
        doc.markdown
    );
    assert!(
        doc.markdown.contains("**bold world**"),
        "md: {}",
        doc.markdown
    );
    assert!(
        doc.markdown.contains("| Item | Status |"),
        "md: {}",
        doc.markdown
    );
    assert!(doc.markdown.contains("Ready"), "md: {}", doc.markdown);

    // Without docProps/core.xml title is None, and .docm extension maps to Docx.
    let no_title = build_docx(r#"<w:p><w:r><w:t>Macro body</w:t></w:r></w:p>"#, None);
    let out_docm = extract_bytes(&no_title, Some("macro.docm"), &ExtractLimits::desktop())
        .expect("docm extraction should succeed");
    let ExtractOutcome::Extracted(docm) = out_docm else {
        panic!("expected Extracted, got {out_docm:?}");
    };
    assert_eq!(docm.format, DocumentFormat::Docx);
    assert_eq!(docm.title, None);
    assert!(docm.plain_text.contains("Macro body"));
}

#[test]
fn extracts_xlsx_xlsm_and_real_biff12_xlsb() {
    let rows = r#"
      <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
      <row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2" s="1"><v>0.155</v></c></row>
    "#;
    let xlsx = build_xlsx("Budget", &["Metric", "Rate", "Growth"], rows);
    for hint in [Some("sheet.xlsx"), Some("sheet.xlsm")] {
        let out = extract_bytes(&xlsx, hint, &ExtractLimits::desktop())
            .expect("xlsx extraction should succeed");
        let ExtractOutcome::Extracted(doc) = out else {
            panic!("expected Extracted, got {out:?}");
        };
        assert_eq!(doc.format, DocumentFormat::Xlsx);
        assert!(doc.markdown.contains("Budget"), "md: {}", doc.markdown);
        assert!(doc.markdown.contains("Growth"), "md: {}", doc.markdown);
        assert!(
            doc.markdown.contains("15.50%") || doc.markdown.contains("15.5%"),
            "expected formatted percent in md: {}",
            doc.markdown
        );
    }

    let xlsb = build_xlsb("BinaryTab", "Binary workbook cell");
    let out = extract_bytes(&xlsb, Some("data.xlsb"), &ExtractLimits::desktop())
        .expect("xlsb extraction should succeed");
    let ExtractOutcome::Extracted(doc) = out else {
        panic!("expected Extracted for xlsb, got {out:?}");
    };
    assert_eq!(doc.format, DocumentFormat::Xlsb);
    assert!(doc.markdown.contains("BinaryTab"), "md: {}", doc.markdown);
    assert!(
        doc.markdown.contains("Binary workbook cell"),
        "md: {}",
        doc.markdown
    );
}

#[test]
fn extracts_pptx_slide_order_bullets_notes_and_aliases() {
    let s1 = slide_xml("Opening Slide", &["First bullet", "Second bullet"]);
    let s2 = slide_xml("Closing Slide", &["Final point"]);
    let pptx = build_pptx(&[(&s1, Some("Speaker note on opening slide.")), (&s2, None)]);
    for hint in [Some("deck.pptx"), Some("deck.pptm"), Some("deck.ppsx")] {
        let out = extract_bytes(&pptx, hint, &ExtractLimits::desktop())
            .expect("pptx extraction should succeed");
        let ExtractOutcome::Extracted(doc) = out else {
            panic!("expected Extracted, got {out:?}");
        };
        assert_eq!(doc.format, DocumentFormat::Pptx);
        let pos1 = doc.markdown.find("Opening Slide").expect("slide 1 present");
        let pos2 = doc.markdown.find("Closing Slide").expect("slide 2 present");
        assert!(pos1 < pos2, "slide order preserved: {}", doc.markdown);
        assert!(doc.markdown.contains("First bullet"));
        assert!(
            doc.markdown.contains("Speaker note on opening slide."),
            "notes missing: {}",
            doc.markdown
        );
    }
}

#[test]
fn extracts_legacy_binary_doc_xls_and_ppt() {
    let doc_bytes = build_legacy_doc("Legacy Word paragraph text", false);
    let ExtractOutcome::Extracted(doc) =
        extract_bytes(&doc_bytes, Some("old.doc"), &ExtractLimits::desktop())
            .expect("legacy doc should extract")
    else {
        panic!("expected Extracted for legacy doc");
    };
    assert_eq!(doc.format, DocumentFormat::Doc);
    assert!(
        doc.plain_text.contains("Legacy Word paragraph text"),
        "doc text: {}",
        doc.plain_text
    );

    let xls_bytes = build_legacy_xls("Ledger", "Legacy Excel value", false);
    let ExtractOutcome::Extracted(xls) =
        extract_bytes(&xls_bytes, Some("old.xls"), &ExtractLimits::desktop())
            .expect("legacy xls should extract")
    else {
        panic!("expected Extracted for legacy xls");
    };
    assert_eq!(xls.format, DocumentFormat::Xls);
    assert!(
        xls.plain_text.contains("Legacy Excel value"),
        "xls text: {}",
        xls.plain_text
    );

    let ppt_bytes = build_legacy_ppt("Legacy slide headline", false);
    let ExtractOutcome::Extracted(ppt) =
        extract_bytes(&ppt_bytes, Some("old.ppt"), &ExtractLimits::desktop())
            .expect("legacy ppt should extract")
    else {
        panic!("expected Extracted for legacy ppt");
    };
    assert_eq!(ppt.format, DocumentFormat::Ppt);
    assert!(
        ppt.plain_text.contains("Legacy slide headline"),
        "ppt text: {}",
        ppt.plain_text
    );
}

#[test]
fn container_sniffing_handles_unhinted_mismatched_and_non_office_inputs() {
    let docx = build_docx(r#"<w:p><w:r><w:t>Sniffed docx</w:t></w:r></w:p>"#, None);
    let xlsx = build_xlsx(
        "Data",
        &["Sniffed xlsx"],
        r#"<row r="1"><c r="A1" t="s"><v>0</v></c></row>"#,
    );
    let xlsb = build_xlsb("BinSheet", "Sniffed xlsb");
    let s1 = slide_xml("Sniffed pptx", &[]);
    let pptx = build_pptx(&[(&s1, None)]);
    let doc = build_legacy_doc("Sniffed legacy doc", false);
    let xls = build_legacy_xls("Sheet1", "Sniffed legacy xls", false);
    let ppt = build_legacy_ppt("Sniffed legacy ppt", false);

    // Unhinted detection across all 7 formats.
    for (bytes, expected_fmt, needle) in [
        (&docx, DocumentFormat::Docx, "Sniffed docx"),
        (&xlsx, DocumentFormat::Xlsx, "Sniffed xlsx"),
        (&xlsb, DocumentFormat::Xlsb, "Sniffed xlsb"),
        (&pptx, DocumentFormat::Pptx, "Sniffed pptx"),
        (&doc, DocumentFormat::Doc, "Sniffed legacy doc"),
        (&xls, DocumentFormat::Xls, "Sniffed legacy xls"),
        (&ppt, DocumentFormat::Ppt, "Sniffed legacy ppt"),
    ] {
        let out = extract_bytes(bytes, None, &ExtractLimits::desktop()).expect("unhinted extract");
        let ExtractOutcome::Extracted(parsed) = out else {
            panic!("expected Extracted for {expected_fmt:?}, got {out:?}");
        };
        assert_eq!(parsed.format, expected_fmt);
        assert!(parsed.plain_text.contains(needle));
    }

    // Cross-family mismatches: real container wins over misleading extension.
    let ExtractOutcome::Extracted(x_as_d) =
        extract_bytes(&xlsx, Some("mislabeled.docx"), &ExtractLimits::desktop())
            .expect("xlsx named docx should sniff as xlsx")
    else {
        panic!("expected Extracted");
    };
    assert_eq!(x_as_d.format, DocumentFormat::Xlsx);

    let ExtractOutcome::Extracted(docx_as_doc) =
        extract_bytes(&docx, Some("modern.doc"), &ExtractLimits::desktop())
            .expect("docx named doc should sniff as docx")
    else {
        panic!("expected Extracted");
    };
    assert_eq!(docx_as_doc.format, DocumentFormat::Docx);

    let ExtractOutcome::Extracted(doc_as_docx) =
        extract_bytes(&doc, Some("legacy.docx"), &ExtractLimits::desktop())
            .expect("legacy doc named docx must not be misclassified as encrypted")
    else {
        panic!("expected Extracted");
    };
    assert_eq!(doc_as_docx.format, DocumentFormat::Doc);

    let ExtractOutcome::Extracted(xls_as_doc) =
        extract_bytes(&xls, Some("sheet.doc"), &ExtractLimits::desktop())
            .expect("xls named doc should sniff as xls")
    else {
        panic!("expected Extracted");
    };
    assert_eq!(xls_as_doc.format, DocumentFormat::Xls);

    // Non-Office ZIP and non-Office CFB return Unsupported when unhinted, never Encrypted.
    let plain_zip = build_zip(&[("hello.txt", b"just a zip")]);
    assert_eq!(
        extract_bytes(&plain_zip, None, &ExtractLimits::desktop()).expect("plain zip"),
        ExtractOutcome::Unsupported
    );
    assert_eq!(
        extract_bytes(&plain_zip, Some("archive.zip"), &ExtractLimits::desktop()).expect("zip ext"),
        ExtractOutcome::Unsupported
    );
    let non_office_cfb = build_cfb(&[("CustomPayload", b"arbitrary ole bytes")]);
    assert_eq!(
        extract_bytes(&non_office_cfb, None, &ExtractLimits::desktop()).expect("non-office cfb"),
        ExtractOutcome::Unsupported
    );
    assert_eq!(
        extract_bytes(
            b"%PDF-1.7\nhello",
            Some("doc.pdf"),
            &ExtractLimits::desktop()
        )
        .expect("pdf is unsupported"),
        ExtractOutcome::Unsupported
    );
    assert_eq!(
        extract_bytes(
            b"random bytes",
            Some("notes.pages"),
            &ExtractLimits::desktop()
        )
        .expect("pages is unsupported"),
        ExtractOutcome::Unsupported
    );
}

#[test]
fn valid_text_free_documents_return_empty_not_invalid() {
    let empty_docx = build_docx("", None);
    assert_eq!(
        extract_bytes(&empty_docx, Some("blank.docx"), &ExtractLimits::desktop())
            .expect("empty docx is valid"),
        ExtractOutcome::Empty {
            format: DocumentFormat::Docx
        }
    );

    let blank_xlsx = build_xlsx("EmptySheet", &[], "");
    assert_eq!(
        extract_bytes(&blank_xlsx, Some("blank.xlsx"), &ExtractLimits::desktop())
            .expect("blank xlsx is valid"),
        ExtractOutcome::Empty {
            format: DocumentFormat::Xlsx
        }
    );

    let image_only_slide = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:spTree/></p:cSld>
</p:sld>"#;
    let empty_pptx = build_pptx(&[(image_only_slide, None)]);
    assert_eq!(
        extract_bytes(
            &empty_pptx,
            Some("diagrams.pptx"),
            &ExtractLimits::desktop()
        )
        .expect("image-only pptx is valid"),
        ExtractOutcome::Empty {
            format: DocumentFormat::Pptx
        }
    );
}

#[test]
fn truncation_respects_utf8_boundaries_desktop_vs_mobile_and_sanitizes_notices() {
    // Multi-byte UTF-8 characters straddling exact char limit on both DOCX and XLSX.
    let utf8_text = "é日本語🚀".repeat(20); // 100 chars
    let docx = build_docx(
        &format!("<w:p><w:r><w:t>{utf8_text}</w:t></w:r></w:p>"),
        None,
    );
    let mut tight = ExtractLimits::desktop();
    tight.max_output_chars = 17;
    let ExtractOutcome::Extracted(doc) =
        extract_bytes(&docx, Some("utf8.docx"), &tight).expect("truncated docx succeeds")
    else {
        panic!("expected Extracted");
    };
    assert!(doc.truncated);
    assert!(doc.markdown.chars().count() <= 17);
    assert!(doc.plain_text.chars().count() <= 17);
    assert!(!doc.warnings.is_empty());

    // XLSX exceeding max_output_chars truncates cleanly (does not error as Incomplete).
    let long_cell = "éβ".repeat(40);
    let xlsx = build_xlsx(
        "Long",
        &[&long_cell],
        r#"<row r="1"><c r="A1" t="s"><v>0</v></c></row>"#,
    );
    let ExtractOutcome::Extracted(sheet) =
        extract_bytes(&xlsx, Some("long.xlsx"), &tight).expect("truncated xlsx succeeds")
    else {
        panic!("expected Extracted");
    };
    assert!(sheet.truncated);
    assert!(sheet.markdown.chars().count() <= 17);
    assert!(sheet.plain_text.chars().count() <= 17);

    // Desktop vs mobile behavioral distinction: ~300k chars fits desktop (512k)
    // untruncated and truncates under mobile (256k).
    let chunk = "alpha beta gamma delta epsilon ".repeat(20); // 620 chars
    let mut body = String::new();
    for _ in 0..500 {
        body.push_str("<w:p><w:r><w:t>");
        body.push_str(&chunk);
        body.push_str("</w:t></w:r></w:p>");
    }
    let big_docx = build_docx(&body, None);
    let ExtractOutcome::Extracted(desk) =
        extract_bytes(&big_docx, Some("big.docx"), &ExtractLimits::desktop())
            .expect("desktop handles 310k chars untruncated")
    else {
        panic!("expected Extracted");
    };
    assert!(!desk.truncated);
    let ExtractOutcome::Extracted(mob) =
        extract_bytes(&big_docx, Some("big.docx"), &ExtractLimits::mobile())
            .expect("mobile truncates 310k chars cleanly")
    else {
        panic!("expected Extracted");
    };
    assert!(mob.truncated);
    assert!(mob.markdown.chars().count() <= ExtractLimits::mobile().max_output_chars);
    assert!(mob.plain_text.chars().count() <= ExtractLimits::mobile().max_output_chars);

    // When markdown exceeds limit while plain_text fits, truncated must still be true.
    let raw = RawExtraction {
        format: DocumentFormat::Docx,
        title: None,
        markdown: "**hello world**".to_owned(),
        plain_text: "hello world".to_owned(),
        warnings: vec!["hit limit (office_oxide::limits::set_max_text_chars)".to_owned()],
    };
    let mut md_only_tight = ExtractLimits::desktop();
    md_only_tight.max_output_chars = 12;
    let ExtractOutcome::Extracted(md_trunc) = finalize_extraction(raw, &md_only_tight) else {
        panic!("expected Extracted");
    };
    assert!(md_trunc.truncated);
    assert!(
        md_trunc
            .warnings
            .iter()
            .all(|w| !w.contains("office_oxide::"))
    );

    // Upstream notice lines alone must never fake non-empty output or leak crate paths.
    let notice_only = RawExtraction {
        format: DocumentFormat::Docx,
        title: None,
        markdown:
            "[output truncated: text limit reached (office_oxide::limits::set_max_text_chars)]"
                .to_owned(),
        plain_text: String::new(),
        warnings: Vec::new(),
    };
    assert_eq!(
        finalize_extraction(notice_only, &ExtractLimits::desktop()),
        ExtractOutcome::Empty {
            format: DocumentFormat::Docx
        }
    );

    let (stripped, notices) = strip_upstream_notices(
        "Real text\n[output truncated: 10 chars (office_oxide::limits::set_max_text_chars)]\nMore text",
    );
    assert!(!stripped.contains("office_oxide::"));
    assert_eq!(notices.len(), 1);
    let sanitized = sanitize_warning(&notices[0]).expect("sanitized notice");
    assert!(!sanitized.contains("office_oxide::"));
}

#[test]
fn rejects_input_exceeding_max_input_bytes_with_positive_twin() {
    let docx = build_docx(r#"<w:p><w:r><w:t>Small doc</w:t></w:r></w:p>"#, None);
    let mut tight = ExtractLimits::desktop();
    tight.max_input_bytes = (docx.len() as u64) - 1;
    let err = extract_bytes(&docx, Some("a.docx"), &tight).expect_err("over max_input_bytes");
    assert!(matches!(err, ExtractError::LimitExceeded(_)), "got {err:?}");

    // Positive twin at exact size succeeds.
    tight.max_input_bytes = docx.len() as u64;
    assert!(matches!(
        extract_bytes(&docx, Some("a.docx"), &tight),
        Ok(ExtractOutcome::Extracted(_))
    ));
}

#[test]
fn rejects_single_part_multi_part_and_partial_slide_zip_bombs_with_positive_twins() {
    let mut limits = ExtractLimits::mobile();
    limits.max_part_bytes = 512 * 1024;
    limits.max_uncompressed_bytes = 1024 * 1024;

    // 1. Single-part bomb (> max_part_bytes) + positive twin.
    let small_docx = build_docx(r#"<w:p><w:r><w:t>Safe</w:t></w:r></w:p>"#, None);
    assert!(matches!(
        extract_bytes(&small_docx, Some("ok.docx"), &limits),
        Ok(ExtractOutcome::Extracted(_))
    ));

    let bomb_padding = "A".repeat(700 * 1024);
    let single_part_bomb = build_docx(
        &format!("<w:p><w:r><w:t>{bomb_padding}</w:t></w:r></w:p>"),
        None,
    );
    let err = extract_bytes(&single_part_bomb, Some("bomb.docx"), &limits)
        .expect_err("single-part bomb must be rejected");
    assert!(matches!(err, ExtractError::LimitExceeded(_)), "got {err:?}");

    // 2. Multi-part sum bomb: 4 parts x 350 KiB = 1.4 MiB > 1 MiB total, each < 512 KiB part cap.
    let part_blob = vec![b'B'; 350 * 1024];
    let multi_part_bomb = build_zip(&[
        (
            "[Content_Types].xml",
            br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#,
        ),
        (
            "word/document.xml",
            br#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>ok</w:t></w:r></w:p></w:body></w:document>"#,
        ),
        ("word/pad1.bin", &part_blob),
        ("word/pad2.bin", &part_blob),
        ("word/pad3.bin", &part_blob),
        ("word/pad4.bin", &part_blob),
    ]);
    let err = extract_bytes(&multi_part_bomb, Some("multi.docx"), &limits)
        .expect_err("multi-part bomb must be rejected");
    assert!(matches!(err, ExtractError::LimitExceeded(_)), "got {err:?}");

    // 3. Multi-slide PPTX: slide 1 valid, slide 2 is a zip bomb (> max_part_bytes).
    let good_s1 = slide_xml("Valid slide 1", &["Point one"]);
    let good_s2 = slide_xml("Valid slide 2", &["Point two"]);
    let good_pptx = build_pptx(&[(&good_s1, None), (&good_s2, None)]);
    assert!(matches!(
        extract_bytes(&good_pptx, Some("ok.pptx"), &limits),
        Ok(ExtractOutcome::Extracted(_))
    ));

    let bomb_s2 = slide_xml("Bomb slide", &[&"Z".repeat(700 * 1024)]);
    let bomb_pptx = build_pptx(&[(&good_s1, None), (&bomb_s2, None)]);
    let err = extract_bytes(&bomb_pptx, Some("bomb.pptx"), &limits)
        .expect_err("partial-slide pptx bomb must be rejected, not salvaged");
    assert!(matches!(err, ExtractError::LimitExceeded(_)), "got {err:?}");
}

#[test]
fn rejects_xlsx_shared_string_fanout_bomb_under_desktop_and_mobile_with_positive_twin() {
    // Small positive twin from same builder succeeds.
    let small_rows = r#"<row r="1"><c r="A1" t="s"><v>0</v></c></row>"#;
    let small_xlsx = build_xlsx("Data", &["Normal shared string"], small_rows);
    assert!(matches!(
        extract_bytes(&small_xlsx, Some("ok.xlsx"), &ExtractLimits::mobile()),
        Ok(ExtractOutcome::Extracted(_))
    ));

    // Fan-out bomb: one 32 KB shared string (Excel cell max is 32,767 chars)
    // referenced by 200 cells -> ~6.55M chars (> 4M desktop and > 1.5M mobile
    // max_expanded_text_chars), stored in a tiny zip.
    let big_str = "X".repeat(32_767);
    let mut rows_200 = String::new();
    for r in 1..=200 {
        rows_200.push_str(&format!(
            r#"<row r="{r}"><c r="A{r}" t="s"><v>0</v></c></row>"#
        ));
    }
    let bomb = build_xlsx("Bomb", &[&big_str], &rows_200);
    assert!(bomb.len() < 16 * 1024, "bomb should stay tiny compressed");

    for preset in [ExtractLimits::desktop(), ExtractLimits::mobile()] {
        let err = extract_bytes(&bomb, Some("bomb.xlsx"), &preset)
            .expect_err("shared-string fanout bomb must fail with LimitExceeded");
        assert!(
            matches!(err, ExtractError::LimitExceeded(_)),
            "expected LimitExceeded, got {err:?}"
        );
    }

    // Mid-size fan-out (~1.96M chars): accepted (truncated) under desktop (4M)
    // and rejected under mobile (1.5M), proving mobile preset is actively enforced.
    let mut rows_60 = String::new();
    for r in 1..=60 {
        rows_60.push_str(&format!(
            r#"<row r="{r}"><c r="A{r}" t="s"><v>0</v></c></row>"#
        ));
    }
    let mid_fanout = build_xlsx("MidFanout", &[&big_str], &rows_60);
    assert!(matches!(
        extract_bytes(&mid_fanout, Some("mid.xlsx"), &ExtractLimits::desktop()),
        Ok(ExtractOutcome::Extracted(doc)) if doc.truncated
    ));
    assert!(matches!(
        extract_bytes(&mid_fanout, Some("mid.xlsx"), &ExtractLimits::mobile()),
        Err(ExtractError::LimitExceeded(_))
    ));
}

#[test]
fn handles_xml_entity_expansion_and_deep_nesting_safely() {
    let billion_laughs = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE lolz [
  <!ENTITY lol "lol">
  <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
  <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
  <!ENTITY lol4 "&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;">
]>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>&lol4;</w:t></w:r></w:p></w:body>
</w:document>"#;
    let bl_docx = build_zip(&[
        (
            "[Content_Types].xml",
            br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#,
        ),
        ("word/document.xml", billion_laughs.as_bytes()),
    ]);
    match extract_bytes(&bl_docx, Some("bl.docx"), &ExtractLimits::mobile()) {
        Ok(ExtractOutcome::Extracted(doc)) => {
            assert!(doc.plain_text.len() < 1024, "entity expanded unexpectedly");
        }
        Ok(ExtractOutcome::Empty { .. })
        | Err(ExtractError::Invalid(_) | ExtractError::LimitExceeded(_)) => {}
        other => panic!("unexpected billion-laughs outcome: {other:?}"),
    }

    // 10,000 nested <w:tbl> elements after a valid paragraph: must finish without
    // stack overflow, OOM, or upstream crate path leaks.
    let mut deep = String::from("<w:p><w:r><w:t>Before deep table</w:t></w:r></w:p>");
    for _ in 0..10_000 {
        deep.push_str("<w:tbl><w:tr><w:tc>");
    }
    deep.push_str("<w:p><w:r><w:t>Innermost</w:t></w:r></w:p>");
    for _ in 0..10_000 {
        deep.push_str("</w:tc></w:tr></w:tbl>");
    }
    let deep_docx = build_docx(&deep, None);
    match extract_bytes(&deep_docx, Some("deep.docx"), &ExtractLimits::mobile()) {
        Ok(ExtractOutcome::Extracted(doc)) => {
            assert!(doc.markdown.contains("Before deep table"));
            assert!(!doc.markdown.contains("office_oxide::"));
            assert!(doc.warnings.iter().all(|w| !w.contains("office_oxide::")));
        }
        Err(ExtractError::LimitExceeded(_) | ExtractError::Invalid(_)) => {}
        other => panic!("unexpected deep-xml outcome: {other:?}"),
    }
}

#[test]
fn rejects_empty_truncated_and_corrupt_inputs_without_panicking() {
    // 0-byte input
    assert!(matches!(
        extract_bytes(&[], Some("empty.docx"), &ExtractLimits::desktop()),
        Err(ExtractError::Invalid(_))
    ));
    assert!(matches!(
        extract_bytes(&[], None, &ExtractLimits::desktop()),
        Err(ExtractError::Invalid(_))
    ));

    // Truncated ZIP
    let docx = build_docx(r#"<w:p><w:r><w:t>Hello</w:t></w:r></w:p>"#, None);
    assert!(matches!(
        extract_bytes(
            &docx[..docx.len() / 2],
            Some("cut.docx"),
            &ExtractLimits::desktop()
        ),
        Err(ExtractError::Invalid(_))
    ));

    // Valid ZIP with truncated/malformed primary XML
    let bad_xml = build_zip(&[
        (
            "[Content_Types].xml",
            br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#,
        ),
        (
            "word/document.xml",
            b"<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>unclosed",
        ),
    ]);
    assert!(matches!(
        extract_bytes(&bad_xml, Some("badxml.docx"), &ExtractLimits::desktop()),
        Err(ExtractError::Invalid(_))
    ));

    // Corrupt / truncated CFB
    let mut bad_cfb = vec![0u8; 256];
    bad_cfb[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    assert!(matches!(
        extract_bytes(&bad_cfb, Some("bad.doc"), &ExtractLimits::desktop()),
        Err(ExtractError::Invalid(_))
    ));
}

#[test]
fn detects_encrypted_ooxml_and_encrypted_legacy_office_files() {
    let enc_ooxml = build_encrypted_ooxml_cfb();
    for hint in [
        Some("secret.docx"),
        Some("secret.xlsx"),
        Some("secret.doc"),
        None,
    ] {
        assert_eq!(
            extract_bytes(&enc_ooxml, hint, &ExtractLimits::desktop())
                .expect("encrypted OOXML should return Encrypted"),
            ExtractOutcome::Encrypted,
            "failed for hint={hint:?}"
        );
    }

    let enc_doc = build_legacy_doc("Secret", true);
    assert_eq!(
        extract_bytes(&enc_doc, Some("secret.doc"), &ExtractLimits::desktop())
            .expect("encrypted doc"),
        ExtractOutcome::Encrypted
    );

    let enc_xls = build_legacy_xls("Secret", "Val", true);
    assert_eq!(
        extract_bytes(&enc_xls, Some("secret.xls"), &ExtractLimits::desktop())
            .expect("encrypted xls"),
        ExtractOutcome::Encrypted
    );

    let enc_ppt = build_legacy_ppt("Secret", true);
    assert_eq!(
        extract_bytes(&enc_ppt, Some("secret.ppt"), &ExtractLimits::desktop())
            .expect("encrypted ppt"),
        ExtractOutcome::Encrypted
    );
}

#[test]
fn catches_backend_panics_and_returns_internal_error() {
    struct PanickingBackend;
    impl FormatBackend for PanickingBackend {
        fn extract(
            &self,
            _bytes: &[u8],
            _format: DocumentFormat,
            _limits: &ExtractLimits,
        ) -> Result<BackendOutcome, ExtractError> {
            panic!("simulated backend panic");
        }
    }

    let docx = build_docx(r#"<w:p><w:r><w:t>Hello</w:t></w:r></w:p>"#, None);
    let err = extract_with_backend(
        &docx,
        Some("test.docx"),
        &ExtractLimits::desktop(),
        &PanickingBackend,
    )
    .expect_err("panic must map to ExtractError::Internal");
    assert!(matches!(err, ExtractError::Internal(msg) if msg.contains("simulated backend panic")));
}

#[test]
fn sniff_ignores_nested_ole_streams_and_normalizes_zip_backslashes() {
    use crate::sniff::{SniffedContainer, sniff_container};

    // Windows-style backslash entry paths in ZIP central directory still sniff as OOXML.
    let backslash_zip = build_zip(&[(r"word\document.xml", b"<w:document/>")]);
    assert_eq!(
        sniff_container(&backslash_zip, &ExtractLimits::desktop()).expect("backslash zip sniff"),
        SniffedContainer::Ooxml(DocumentFormat::Docx)
    );

    // MS-CFB PowerPoint deck with an embedded OLE storage ("ObjectPool") whose
    // child stream is named "WordDocument": top-level sibling tree traversal
    // must classify the outer file as Ppt, not Doc.
    const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
    const FATSECT: u32 = 0xFFFF_FFFD;
    const FREESECT: u32 = 0xFFFF_FFFF;
    const NOSTREAM: u32 = 0xFFFF_FFFF;

    let text_header = ppt_record(0x0000, 0x0F9F, &0u32.to_le_bytes());
    let text_bytes = ppt_record(0x0000, 0x0FA8, b"Outer PowerPoint headline");
    let mut drawing_children = Vec::new();
    drawing_children.extend(text_header);
    drawing_children.extend(text_bytes);
    let client_textbox = ppt_record(0x000F, 0xF00D, &drawing_children);
    let slide = ppt_record(0x000F, 0x03EE, &client_textbox);
    assert!(slide.len() <= 512);

    // 4 sectors: 0=FAT, 1=Dir (4 slots), 2=PowerPoint Document, 3=nested WordDocument
    let mut file = vec![0u8; 512 * 5];
    file[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    file[0x18..0x1A].copy_from_slice(&0x003Eu16.to_le_bytes());
    file[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    file[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    file[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    file[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    file[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
    file[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
    file[0x38..0x3C].copy_from_slice(&0u32.to_le_bytes());
    file[0x3C..0x40].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    file[0x44..0x48].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    file[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes());
    for i in 1..109 {
        file[0x4C + i * 4..0x50 + i * 4].copy_from_slice(&FREESECT.to_le_bytes());
    }

    let fat = [FATSECT, ENDOFCHAIN, ENDOFCHAIN, ENDOFCHAIN];
    for i in 0..128 {
        let val = fat.get(i).copied().unwrap_or(FREESECT);
        file[512 + i * 4..512 + i * 4 + 4].copy_from_slice(&val.to_le_bytes());
    }

    let write_dir =
        |buf: &mut [u8], name: &str, kind: u8, right: u32, child: u32, start: u32, size: u32| {
            let units: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            for (i, u) in units.iter().enumerate() {
                buf[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
            }
            buf[0x40..0x42].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
            buf[0x42] = kind;
            buf[0x43] = 1;
            buf[0x44..0x48].copy_from_slice(&NOSTREAM.to_le_bytes());
            buf[0x48..0x4C].copy_from_slice(&right.to_le_bytes());
            buf[0x4C..0x50].copy_from_slice(&child.to_le_bytes());
            buf[0x74..0x78].copy_from_slice(&start.to_le_bytes());
            buf[0x78..0x7C].copy_from_slice(&size.to_le_bytes());
        };

    // Slot 0: Root Entry -> child = slot 1 ("PowerPoint Document")
    write_dir(
        &mut file[1024..1152],
        "Root Entry",
        5,
        NOSTREAM,
        1,
        ENDOFCHAIN,
        0,
    );
    // Slot 1: PowerPoint Document -> right sibling = slot 2 ("ObjectPool")
    write_dir(
        &mut file[1152..1280],
        "PowerPoint Document",
        2,
        2,
        NOSTREAM,
        2,
        slide.len() as u32,
    );
    // Slot 2: ObjectPool (storage) -> child = slot 3 ("WordDocument")
    write_dir(&mut file[1280..1408], "ObjectPool", 1, NOSTREAM, 3, 0, 0);
    // Slot 3: nested WordDocument stream inside ObjectPool
    write_dir(
        &mut file[1408..1536],
        "WordDocument",
        2,
        NOSTREAM,
        NOSTREAM,
        3,
        16,
    );
    file[512 * 3..512 * 3 + slide.len()].copy_from_slice(&slide);

    assert_eq!(
        sniff_container(&file, &ExtractLimits::desktop()).expect("nested ole cfb sniff"),
        SniffedContainer::LegacyCfb(DocumentFormat::Ppt)
    );
    let ExtractOutcome::Extracted(ppt) =
        extract_bytes(&file, None, &ExtractLimits::desktop()).expect("extract nested ole ppt")
    else {
        panic!("expected Extracted for outer ppt");
    };
    assert_eq!(ppt.format, DocumentFormat::Ppt);
    assert!(ppt.plain_text.contains("Outer PowerPoint headline"));
}
