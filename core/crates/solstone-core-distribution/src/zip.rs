// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Read};

use flate2::read::DeflateDecoder;

const LOCAL_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
const STORED: u16 = 0;
const DEFLATE: u16 = 8;

#[derive(Debug, Clone)]
pub struct ZipMember {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub fn read_members(bytes: &[u8]) -> io::Result<Vec<ZipMember>> {
    let mut offset = 0;
    let mut members = Vec::new();
    while offset + 30 <= bytes.len() {
        if bytes[offset..offset + 4] != LOCAL_MAGIC {
            break;
        }
        let method = u16::from_le_bytes(bytes[offset + 8..offset + 10].try_into().unwrap());
        let compressed =
            u32::from_le_bytes(bytes[offset + 18..offset + 22].try_into().unwrap()) as usize;
        let name_len =
            u16::from_le_bytes(bytes[offset + 26..offset + 28].try_into().unwrap()) as usize;
        let extra_len =
            u16::from_le_bytes(bytes[offset + 28..offset + 30].try_into().unwrap()) as usize;
        let name_at = offset + 30;
        let data_at = name_at + name_len + extra_len;
        let data_end = data_at + compressed;
        if data_end > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated zip member",
            ));
        }
        let name = std::str::from_utf8(&bytes[name_at..name_at + name_len])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .to_owned();
        crate::archive::refuse_escape(&name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.as_str()))?;
        let payload = match method {
            STORED => bytes[data_at..data_end].to_vec(),
            DEFLATE => {
                let mut decoder = DeflateDecoder::new(&bytes[data_at..data_end]);
                let mut out = Vec::new();
                decoder.read_to_end(&mut out)?;
                out
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsupported zip method {other}"),
                ));
            }
        };
        members.push(ZipMember {
            name,
            bytes: payload,
        });
        offset = data_end;
    }
    Ok(members)
}

pub fn member<'a>(members: &'a [ZipMember], name: &str) -> io::Result<&'a [u8]> {
    members
        .iter()
        .find(|item| item.name == name)
        .map(|item| item.bytes.as_slice())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("zip member {name} missing"),
            )
        })
}

pub fn write_stored_zip(files: &[(&str, &[u8])]) -> io::Result<Vec<u8>> {
    let mut local = Vec::new();
    let mut central = Vec::new();
    for (name, body) in files {
        crate::archive::refuse_escape(name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.as_str()))?;
        let name_bytes = name.as_bytes();
        let offset = local.len() as u32;
        local.extend_from_slice(&LOCAL_MAGIC);
        local.extend_from_slice(&20_u16.to_le_bytes());
        local.extend_from_slice(&0_u16.to_le_bytes());
        local.extend_from_slice(&STORED.to_le_bytes());
        local.extend_from_slice(&0_u16.to_le_bytes());
        local.extend_from_slice(&0_u16.to_le_bytes());
        local.extend_from_slice(&0_u32.to_le_bytes());
        local.extend_from_slice(&(body.len() as u32).to_le_bytes());
        local.extend_from_slice(&(body.len() as u32).to_le_bytes());
        local.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        local.extend_from_slice(&0_u16.to_le_bytes());
        local.extend_from_slice(name_bytes);
        local.extend_from_slice(body);

        central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&STORED.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u32.to_le_bytes());
        central.extend_from_slice(&(body.len() as u32).to_le_bytes());
        central.extend_from_slice(&(body.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let central_at = local.len() as u32;
    let mut out = local;
    out.extend_from_slice(&central);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    Ok(out)
}
