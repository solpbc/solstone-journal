// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::digest::sha256_hex;
use crate::record::FileRecord;
use crate::relocate::{from_system_path, to_system_path};
use crate::stage::staged_files;
use crate::tar::gzip_bytes;

const LEAD_LEN: usize = 96;
const HEADER_MAGIC: [u8; 8] = [0x8e, 0xad, 0xe8, 0x01, 0x00, 0x00, 0x00, 0x00];
const RPM_STRING: i32 = 6;
const RPM_STRING_ARRAY: i32 = 8;
const TAG_NAME: i32 = 1000;
const TAG_VERSION: i32 = 1001;
const TAG_RELEASE: i32 = 1002;
const TAG_ARCH: i32 = 1022;
const TAG_REQUIRENAME: i32 = 1049;

pub struct RpmMeta<'a> {
    pub version: &'a str,
    pub arch: &'a str,
}

pub fn write_rpm(stage: &Path, dest: &Path, meta: RpmMeta<'_>) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = gzip_cpio(stage)?;
    let mut lead = [0_u8; LEAD_LEN];
    lead[0..4].copy_from_slice(&0xedabeedb_u32.to_be_bytes());
    lead[4] = 3;
    lead[5] = 0;
    let name = b"solstone-journal";
    lead[10..10 + name.len()].copy_from_slice(name);
    lead[76..78].copy_from_slice(&1_u16.to_be_bytes());
    let signature = empty_header();
    let header = main_header(meta)?;
    let mut out = Vec::new();
    out.extend_from_slice(&lead);
    out.extend_from_slice(&signature);
    out.extend_from_slice(&header);
    out.extend_from_slice(&payload);
    fs::write(dest, out)
}

fn empty_header() -> Vec<u8> {
    let mut bytes = HEADER_MAGIC.to_vec();
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes
}

fn main_header(meta: RpmMeta<'_>) -> io::Result<Vec<u8>> {
    let mut store = Vec::new();
    let mut index = Vec::new();
    add_string(&mut index, &mut store, TAG_NAME, "solstone-journal");
    add_string(&mut index, &mut store, TAG_VERSION, meta.version);
    add_string(&mut index, &mut store, TAG_RELEASE, "1");
    add_string(&mut index, &mut store, TAG_ARCH, meta.arch);
    add_string_array(&mut index, &mut store, TAG_REQUIRENAME, &["libc.so.6"]);
    let mut header = HEADER_MAGIC.to_vec();
    header.extend_from_slice(&(index.len() as u32 / 16).to_be_bytes());
    header.extend_from_slice(&(store.len() as u32).to_be_bytes());
    header.extend_from_slice(&index);
    header.extend_from_slice(&store);
    Ok(header)
}

fn add_string(index: &mut Vec<u8>, store: &mut Vec<u8>, tag: i32, value: &str) {
    let offset = store.len() as i32;
    store.extend_from_slice(value.as_bytes());
    store.push(0);
    push_index(index, tag, RPM_STRING, offset, 1);
}

fn add_string_array(index: &mut Vec<u8>, store: &mut Vec<u8>, tag: i32, values: &[&str]) {
    let offset = store.len() as i32;
    for value in values {
        store.extend_from_slice(value.as_bytes());
        store.push(0);
    }
    push_index(index, tag, RPM_STRING_ARRAY, offset, values.len() as i32);
}

fn push_index(index: &mut Vec<u8>, tag: i32, kind: i32, offset: i32, count: i32) {
    index.extend_from_slice(&tag.to_be_bytes());
    index.extend_from_slice(&kind.to_be_bytes());
    index.extend_from_slice(&offset.to_be_bytes());
    index.extend_from_slice(&count.to_be_bytes());
}

fn gzip_cpio(stage: &Path) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut ino = 1_u32;
    for dest in staged_files(stage)? {
        let archive = to_system_path(&dest).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unstaged dest {dest} has no system prefix"),
            )
        })?;
        let path = stage.join(&dest);
        let bytes = fs::read(&path)?;
        let mode = fs::metadata(&path)?.permissions().mode() & 0o7777;
        write_cpio_member(&mut raw, ino, &archive, &bytes, mode)?;
        ino += 1;
    }
    write_cpio_member(&mut raw, ino, "TRAILER!!!", &[], 0)?;
    gzip_bytes(&raw)
}

fn write_cpio_member(
    out: &mut Vec<u8>,
    ino: u32,
    name: &str,
    bytes: &[u8],
    mode: u32,
) -> io::Result<()> {
    let name_bytes = format!("{name}\0");
    let namesize = name_bytes.len() as u32;
    let mut header = String::from("070701");
    header.push_str(&format!("{ino:08x}"));
    header.push_str(&format!("{mode:08x}"));
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000001");
    header.push_str("00000000");
    header.push_str(&format!("{:08x}", bytes.len() as u32));
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str(&format!("{namesize:08x}"));
    header.push_str("00000000");
    out.write_all(header.as_bytes())?;
    out.write_all(name_bytes.as_bytes())?;
    pad4(out, 6 + 13 * 8 + name_bytes.len())?;
    out.write_all(bytes)?;
    pad4(out, bytes.len())?;
    Ok(())
}

fn pad4(out: &mut Vec<u8>, used: usize) -> io::Result<()> {
    let pad = (4 - (used % 4)) % 4;
    if pad > 0 {
        out.write_all(&[0_u8; 4][..pad])?;
    }
    Ok(())
}

fn skip_headers(bytes: &[u8]) -> io::Result<usize> {
    if bytes.len() < LEAD_LEN + 16 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated rpm"));
    }
    let mut offset = LEAD_LEN;
    offset = skip_one_header(bytes, offset)?;
    offset = skip_one_header(bytes, offset)?;
    Ok(offset)
}

fn skip_one_header(bytes: &[u8], offset: usize) -> io::Result<usize> {
    if offset + 16 > bytes.len() || bytes[offset..offset + 8] != HEADER_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid rpm header magic",
        ));
    }
    let index = u32::from_be_bytes(bytes[offset + 8..offset + 12].try_into().unwrap()) as usize;
    let data = u32::from_be_bytes(bytes[offset + 12..offset + 16].try_into().unwrap()) as usize;
    Ok(offset + 16 + index * 16 + data)
}

pub fn rpm_records(path: &Path) -> io::Result<Vec<FileRecord>> {
    let bytes = fs::read(path)?;
    let start = skip_headers(&bytes)?;
    let raw = crate::tar::gunzip_bytes(&bytes[start..])?;
    read_cpio_records(&raw)
}

fn read_cpio_records(raw: &[u8]) -> io::Result<Vec<FileRecord>> {
    let mut offset = 0;
    let mut records = Vec::new();
    while offset + 110 <= raw.len() {
        if &raw[offset..offset + 6] != b"070701" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid cpio magic",
            ));
        }
        let mode = parse_hex(&raw[offset + 14..offset + 22])?;
        let filesize = parse_hex(&raw[offset + 54..offset + 62])?;
        let namesize = parse_hex(&raw[offset + 94..offset + 102])?;
        let name_start = offset + 110;
        let name_end = name_start + namesize;
        if name_end > raw.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated cpio name",
            ));
        }
        let name = std::str::from_utf8(&raw[name_start..name_end - 1])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let after_name = align4(name_end);
        let data_end = after_name + filesize;
        if name == "TRAILER!!!" {
            break;
        }
        if data_end > raw.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated cpio file",
            ));
        }
        let dest = from_system_path(name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("rpm member {name} is outside the system prefix"),
            )
        })?;
        let digest = sha256_hex(&raw[after_name..data_end]);
        records.push(FileRecord::file(dest, mode as u32, digest));
        offset = align4(data_end);
    }
    records.sort();
    Ok(records)
}

fn parse_hex(bytes: &[u8]) -> io::Result<usize> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    usize::from_str_radix(text, 16)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

pub fn rpm_requires(path: &Path) -> io::Result<Vec<String>> {
    let bytes = fs::read(path)?;
    let sig_end = skip_one_header(&bytes, LEAD_LEN)?;
    read_require_names(&bytes[sig_end..])
}

pub fn rpm_arch(path: &Path) -> io::Result<String> {
    let bytes = fs::read(path)?;
    let sig_end = skip_one_header(&bytes, LEAD_LEN)?;
    read_string_tag(&bytes[sig_end..], TAG_ARCH)
}

fn read_string_tag(header: &[u8], wanted: i32) -> io::Result<String> {
    if header.len() < 16 || header[..8] != HEADER_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid rpm header",
        ));
    }
    let index = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
    let data = u32::from_be_bytes(header[12..16].try_into().unwrap()) as usize;
    let store_at = 16 + index * 16;
    if store_at + data > header.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated rpm header store",
        ));
    }
    let store = &header[store_at..store_at + data];
    for slot in 0..index {
        let base = 16 + slot * 16;
        let tag = i32::from_be_bytes(header[base..base + 4].try_into().unwrap());
        if tag != wanted {
            continue;
        }
        let offset = i32::from_be_bytes(header[base + 8..base + 12].try_into().unwrap()) as usize;
        let end = store[offset..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unterminated rpm string"))?;
        return std::str::from_utf8(&store[offset..offset + end])
            .map(str::to_owned)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("rpm tag {wanted} missing"),
    ))
}

fn read_require_names(header: &[u8]) -> io::Result<Vec<String>> {
    if header.len() < 16 || header[..8] != HEADER_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid rpm header",
        ));
    }
    let index = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
    let data = u32::from_be_bytes(header[12..16].try_into().unwrap()) as usize;
    let store_at = 16 + index * 16;
    if store_at + data > header.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated rpm header store",
        ));
    }
    let store = &header[store_at..store_at + data];
    for slot in 0..index {
        let base = 16 + slot * 16;
        let tag = i32::from_be_bytes(header[base..base + 4].try_into().unwrap());
        if tag != TAG_REQUIRENAME {
            continue;
        }
        let offset = i32::from_be_bytes(header[base + 8..base + 12].try_into().unwrap()) as usize;
        let count = i32::from_be_bytes(header[base + 12..base + 16].try_into().unwrap()) as usize;
        let mut names = Vec::new();
        let mut cursor = offset;
        for _ in 0..count {
            let end = store[cursor..]
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "unterminated require")
                })?;
            names.push(
                std::str::from_utf8(&store[cursor..cursor + end])
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                    .to_owned(),
            );
            cursor += end + 1;
        }
        return Ok(names);
    }
    Ok(Vec::new())
}
