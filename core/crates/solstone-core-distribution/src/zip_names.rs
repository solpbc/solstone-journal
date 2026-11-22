// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Read, Seek};

use solstone_core_journal_io::zip2_name::{Zip2NameFailure, prove_zip2_directory};
use zip::ZipArchive;

pub(crate) fn prove_open_zip<R: Read + Seek>(
    mut archive: ZipArchive<R>,
) -> Result<(ZipArchive<R>, Vec<String>), Zip2NameFailure> {
    let mut coords = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| Zip2NameFailure::Unreadable {
                detail: error.to_string().into(),
            })?;
        coords.push((entry.header_start(), entry.central_header_start()));
    }
    let central_start = archive.central_directory_start();
    let mut reader = archive.into_inner();
    let names = prove_zip2_directory(&mut reader, central_start, &coords)?;
    let archive = ZipArchive::new(reader).map_err(|error| Zip2NameFailure::Unreadable {
        detail: error.to_string().into(),
    })?;
    Ok((archive, names))
}

#[cfg(test)]
pub(crate) mod fixtures {
    use solstone_core_journal_io::zip2_name::crc32_ieee;

    pub(crate) const UTF8_FLAG: u16 = 1 << 11;

    pub(crate) struct Member {
        pub name: Vec<u8>,
        pub flags: u16,
        pub extra: Vec<u8>,
        pub data: Vec<u8>,
    }

    pub(crate) fn ascii(name: &str, data: &[u8]) -> Member {
        Member {
            name: name.as_bytes().to_vec(),
            flags: UTF8_FLAG,
            extra: Vec::new(),
            data: data.to_vec(),
        }
    }

    pub(crate) fn colliding(name: &str, data: &[u8]) -> Vec<u8> {
        build_zip(&[
            ascii(name, data),
            Member {
                name: vec![0x82],
                flags: 0,
                extra: Vec::new(),
                data: b"a".to_vec(),
            },
            Member {
                name: "é".as_bytes().to_vec(),
                flags: UTF8_FLAG,
                extra: Vec::new(),
                data: b"b".to_vec(),
            },
        ])
    }

    pub(crate) fn build_zip(members: &[Member]) -> Vec<u8> {
        let mut local = Vec::new();
        let mut local_offsets = Vec::new();
        for member in members {
            local_offsets.push(local.len() as u32);
            let crc = crc32_ieee(&member.data);
            let size = member.data.len() as u32;
            local.extend_from_slice(b"PK\x03\x04");
            local.extend_from_slice(&20_u16.to_le_bytes());
            local.extend_from_slice(&member.flags.to_le_bytes());
            local.extend_from_slice(&0_u16.to_le_bytes());
            local.extend_from_slice(&0_u16.to_le_bytes());
            local.extend_from_slice(&0_u16.to_le_bytes());
            local.extend_from_slice(&crc.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&(member.name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0_u16.to_le_bytes());
            local.extend_from_slice(&member.name);
            local.extend_from_slice(&member.data);
        }
        let cd_start = local.len() as u32;
        let mut central = Vec::new();
        for (index, member) in members.iter().enumerate() {
            let crc = crc32_ieee(&member.data);
            let size = member.data.len() as u32;
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&member.flags.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(member.name.len() as u16).to_le_bytes());
            central.extend_from_slice(&(member.extra.len() as u16).to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(&local_offsets[index].to_le_bytes());
            central.extend_from_slice(&member.name);
            central.extend_from_slice(&member.extra);
        }
        let mut bytes = local;
        bytes.extend_from_slice(&central);
        let count = members.len() as u16;
        bytes.extend_from_slice(b"PK\x05\x06");
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&(central.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&cd_start.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes
    }
}
