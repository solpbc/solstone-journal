// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Write};

const MAGIC: &[u8] = b"!<arch>\n";
const HEADER_LEN: usize = 60;

pub fn write_archive<W: Write>(out: &mut W, members: &[(&str, &[u8])]) -> io::Result<()> {
    out.write_all(MAGIC)?;
    for (name, body) in members {
        write_member(out, name, body)?;
    }
    Ok(())
}

fn write_member<W: Write>(out: &mut W, name: &str, body: &[u8]) -> io::Result<()> {
    if name.len() > 15 || name.as_bytes().contains(&b' ') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ar member name too long or spaced: {name}"),
        ));
    }
    let mut header = [b' '; HEADER_LEN];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[name.len()] = b'/';
    write_ascii(&mut header[16..28], 0);
    write_ascii(&mut header[28..34], 0);
    write_ascii(&mut header[34..40], 0);
    write_ascii(&mut header[40..48], 0o100644);
    write_ascii(&mut header[48..58], body.len() as u64);
    header[58] = b'`';
    header[59] = b'\n';
    out.write_all(&header)?;
    out.write_all(body)?;
    if body.len() % 2 == 1 {
        out.write_all(&[b'\n'])?;
    }
    Ok(())
}

fn write_ascii(slot: &mut [u8], value: u64) {
    let text = value.to_string();
    slot[..text.len()].copy_from_slice(text.as_bytes());
}

pub fn read_archive(bytes: &[u8]) -> io::Result<Vec<(String, Vec<u8>)>> {
    if !bytes.starts_with(MAGIC) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a unix ar archive",
        ));
    }
    let mut offset = MAGIC.len();
    let mut members = Vec::new();
    while offset + HEADER_LEN <= bytes.len() {
        let header = &bytes[offset..offset + HEADER_LEN];
        offset += HEADER_LEN;
        let name = parse_name(header)?;
        let size = parse_ascii(&header[48..58])?;
        let end = offset + size;
        if end > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated ar member",
            ));
        }
        members.push((name, bytes[offset..end].to_vec()));
        offset = end;
        if size % 2 == 1 {
            offset += 1;
        }
    }
    Ok(members)
}

fn parse_name(header: &[u8]) -> io::Result<String> {
    let raw = header[..16]
        .iter()
        .copied()
        .take_while(|byte| *byte != b' ')
        .collect::<Vec<_>>();
    let text = String::from_utf8(raw)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(text.trim_end_matches('/').to_owned())
}

fn parse_ascii(slot: &[u8]) -> io::Result<usize> {
    let text = std::str::from_utf8(slot)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .trim();
    text.parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn member<'a>(members: &'a [(String, Vec<u8>)], name: &str) -> io::Result<&'a [u8]> {
    members
        .iter()
        .find(|(found, _)| found == name)
        .map(|(_, body)| body.as_slice())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("missing ar member {name}")))
}
