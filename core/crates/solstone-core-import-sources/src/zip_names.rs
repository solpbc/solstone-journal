// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Read, Seek};
use std::path::Path;

use solstone_core_journal_io::zip2_name::{Zip2NameFailure, prove_zip2_directory};
use zip::ZipArchive;

use crate::ImportSourcesError;

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

pub(crate) fn zip2_failure_to_zip_error(failure: Zip2NameFailure) -> zip::result::ZipError {
    zip::result::ZipError::InvalidArchive(std::borrow::Cow::Owned(failure.to_string()))
}

pub(crate) fn archive_invalid(path: &Path, failure: Zip2NameFailure) -> ImportSourcesError {
    ImportSourcesError::ArchiveInvalid {
        path: path.to_path_buf(),
        detail: failure.to_string(),
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use solstone_core_journal_io::zip2_name::crc32_ieee;

    pub(crate) const UTF8_FLAG: u16 = 1 << 11;
    pub(crate) const UNICODE_PATH: u16 = 0x7075;

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
        // Zip 9 keys IndexMap by file_name_raw after applying a valid 0x7075
        // extra, so a unicode-path remap of "other" onto `name` collapses.
        // Unflagged CP437 0x82 and flagged UTF-8 é keep distinct raw keys.
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

    pub(crate) fn unproved(name: &str, data: &[u8]) -> Vec<u8> {
        build_zip(&[Member {
            name: name.as_bytes().to_vec(),
            flags: UTF8_FLAG,
            extra: extra_7075(name.as_bytes(), name.as_bytes(), Some(0)),
            data: data.to_vec(),
        }])
    }

    pub(crate) fn extra_7075(raw_name: &[u8], payload: &[u8], crc: Option<u32>) -> Vec<u8> {
        let crc = crc.unwrap_or_else(|| crc32_ieee(raw_name));
        let mut extra = Vec::new();
        extra.extend_from_slice(&UNICODE_PATH.to_le_bytes());
        let data_len = 1 + 4 + payload.len();
        extra.extend_from_slice(&(data_len as u16).to_le_bytes());
        extra.push(1);
        extra.extend_from_slice(&crc.to_le_bytes());
        extra.extend_from_slice(payload);
        extra
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
