// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Zip 2.4.2 filename identity from archive bytes.
//!
//! Names come from the central header, then Info-ZIP Unicode Path extras
//! `0x7075` in order. Local-header extras are not part of this identity.

use solstone_core_cli_copy::MessageDetail;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

/// Why a zip member name cannot be used as a zip-2 identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zip2NameFailure {
    /// The archive bytes cannot be read as a zip central/local header.
    Unreadable { detail: MessageDetail },
    /// Header bytes were readable, but the zip-2 name is not proved.
    Unproved { index: usize, detail: MessageDetail },
    /// Two members prove to the same identity.
    Collision { name: String },
}

impl Zip2NameFailure {
    pub fn into_detail(self) -> MessageDetail {
        match self {
            Self::Unreadable { detail } => {
                detail.context("journal.cli.transcribe.error.writer.zip.unreadable")
            }
            Self::Unproved { index, detail } => detail.context_with(
                "journal.cli.transcribe.error.writer.zip.unproved",
                [("index", &index)],
            ),
            Self::Collision { name } => MessageDetail::message(
                "journal.cli.transcribe.error.writer.zip.zip.member.name.collision.name",
                [("name", &(name))],
            ),
        }
    }
}

impl fmt::Display for Zip2NameFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.clone().into_detail().fmt(formatter)
    }
}

impl Error for Zip2NameFailure {}

/// IBM CP437 scalars for bytes `0x80..=0xFF`.
const CP437_HIGH: [u32; 128] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7, 0x00EA, 0x00EB, 0x00E8, 0x00EF,
    0x00EE, 0x00EC, 0x00C4, 0x00C5, 0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00A2, 0x00A3, 0x00A5, 0x20A7, 0x0192, 0x00E1, 0x00ED, 0x00F3, 0x00FA,
    0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255D, 0x255C, 0x255B, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4, 0x03A6, 0x0398, 0x03A9, 0x03B4,
    0x221E, 0x03C6, 0x03B5, 0x2229, 0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];

const IEEE_POLY: u32 = 0xEDB8_8320;
const CENTRAL_SIGNATURE: [u8; 4] = *b"PK\x01\x02";
const LOCAL_SIGNATURE: [u8; 4] = *b"PK\x03\x04";
const CENTRAL_HEADER_LEN: usize = 46;
const UTF8_FLAG: u16 = 1 << 11;
const UNICODE_PATH_EXTRA: u16 = 0x7075;

/// IEEE CRC-32 (poly `0xEDB88320`).
pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ IEEE_POLY
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Check the raw central-directory census before proving exposed member names.
///
/// ZIP9 may collapse members while constructing its index. Walk only the central
/// records from its public directory offset; do not infer a complete population
/// from the surviving entries. Refuse any lost or unexplained coordinate.
pub fn prove_zip2_directory<R: Read + Seek>(
    reader: &mut R,
    central_start: u64,
    entries: &[(u64, u64)],
) -> Result<Vec<String>, Zip2NameFailure> {
    let unreadable = |detail: MessageDetail| Zip2NameFailure::Unreadable { detail };
    let file_end = reader
        .seek(SeekFrom::End(0))
        .map_err(|error| unreadable(error.to_string().into()))?;
    let mut remaining: std::collections::BTreeSet<u64> =
        entries.iter().map(|entry| entry.1).collect();
    if remaining.len() != entries.len() {
        return Err(unreadable(MessageDetail::message(
            "journal.cli.transcribe.error.writer.zip.repeated.exposed.central.coordinate",
            [],
        )));
    }
    let mut cursor = central_start;
    let mut count = 0;
    loop {
        let signature =
            read_at(reader, cursor, 4).map_err(|error| unreadable(error.to_string().into()))?;
        if signature != CENTRAL_SIGNATURE {
            break;
        }
        if !remaining.remove(&cursor) {
            return Err(unreadable(MessageDetail::message(
                "journal.cli.transcribe.error.writer.zip.raw.central.member.was.lost.from.the.zip",
                [],
            )));
        }
        let header = read_at(reader, cursor, CENTRAL_HEADER_LEN)
            .map_err(|error| unreadable(error.to_string().into()))?;
        let name_len = u16::from_le_bytes([header[28], header[29]]) as u64;
        let extra_len = u16::from_le_bytes([header[30], header[31]]) as u64;
        let comment_len = u16::from_le_bytes([header[32], header[33]]) as u64;
        cursor = cursor
            .checked_add(CENTRAL_HEADER_LEN as u64 + name_len + extra_len + comment_len)
            .filter(|next| *next <= file_end)
            .ok_or_else(|| {
                unreadable(MessageDetail::message(
                    "journal.cli.transcribe.error.writer.zip.central.record.extends.beyond.archive",
                    [],
                ))
            })?;
        count += 1;
    }
    if count != entries.len() || !remaining.is_empty() {
        return Err(unreadable(MessageDetail::message(
            "journal.cli.transcribe.error.writer.zip.raw.and.exposed.central.populations.disagree",
            [],
        )));
    }
    prove_zip2_names(reader, entries)
}

/// Prove zip 2.4.2 member identities from archive bytes at the given offsets.
///
/// `entries` is `(header_start, central_header_start)` in zip index order.
pub fn prove_zip2_names<R: Read + Seek>(
    reader: &mut R,
    entries: &[(u64, u64)],
) -> Result<Vec<String>, Zip2NameFailure> {
    let mut names = Vec::with_capacity(entries.len());
    for (index, &(header_start, central_header_start)) in entries.iter().enumerate() {
        names.push(prove_one(
            reader,
            index,
            header_start,
            central_header_start,
        )?);
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (index, name) in names.iter().enumerate() {
        if let Some(_prior) = seen.insert(name.as_str(), index) {
            return Err(Zip2NameFailure::Collision { name: name.clone() });
        }
    }
    Ok(names)
}

fn prove_one<R: Read + Seek>(
    reader: &mut R,
    index: usize,
    header_start: u64,
    central_header_start: u64,
) -> Result<String, Zip2NameFailure> {
    let central = read_at(reader, central_header_start, CENTRAL_HEADER_LEN).map_err(|error| {
        Zip2NameFailure::Unreadable {
            detail: MessageDetail::message("journal.cli.transcribe.error.writer.zip.central.header.at.central.header.start.error", [("central_header_start", &(central_header_start)), ("error", &(error))]),
        }
    })?;
    if central[0..4] != CENTRAL_SIGNATURE {
        return Err(Zip2NameFailure::Unreadable {
            detail: MessageDetail::message(
                "journal.cli.transcribe.error.writer.zip.central.signature.at.central.header.start",
                [("central_header_start", &(central_header_start))],
            ),
        });
    }
    let local = read_at(reader, header_start, 4).map_err(|error| Zip2NameFailure::Unreadable {
        detail: MessageDetail::message(
            "journal.cli.transcribe.error.writer.zip.local.header.at.header.start.error",
            [("header_start", &(header_start)), ("error", &(error))],
        ),
    })?;
    if local != LOCAL_SIGNATURE {
        return Err(Zip2NameFailure::Unreadable {
            detail: MessageDetail::message(
                "journal.cli.transcribe.error.writer.zip.local.signature.at.header.start",
                [("header_start", &(header_start))],
            ),
        });
    }
    let flags = u16::from_le_bytes([central[8], central[9]]);
    let name_len = u16::from_le_bytes([central[28], central[29]]) as usize;
    let extra_len = u16::from_le_bytes([central[30], central[31]]) as usize;
    let name_at = central_header_start + CENTRAL_HEADER_LEN as u64;
    let mut raw =
        read_at(reader, name_at, name_len).map_err(|error| Zip2NameFailure::Unreadable {
            detail: MessageDetail::message(
                "journal.cli.transcribe.error.writer.zip.central.name.at.name.at.error",
                [("name_at", &(name_at)), ("error", &(error))],
            ),
        })?;
    let extra_at = name_at + name_len as u64;
    let extra =
        read_at(reader, extra_at, extra_len).map_err(|error| Zip2NameFailure::Unreadable {
            detail: MessageDetail::message(
                "journal.cli.transcribe.error.writer.zip.central.extra.at.extra.at.error",
                [("extra_at", &(extra_at)), ("error", &(error))],
            ),
        })?;
    let overridden = apply_unicode_path_extras(index, &mut raw, &extra)?;
    decode_identity(
        index,
        if overridden { flags | UTF8_FLAG } else { flags },
        &raw,
    )
}

fn apply_unicode_path_extras(
    index: usize,
    raw: &mut Vec<u8>,
    extra: &[u8],
) -> Result<bool, Zip2NameFailure> {
    let mut offset = 0;
    let mut overridden = false;
    while offset + 4 <= extra.len() {
        let id = u16::from_le_bytes([extra[offset], extra[offset + 1]]);
        let len = u16::from_le_bytes([extra[offset + 2], extra[offset + 3]]) as usize;
        offset += 4;
        if offset.checked_add(len).is_none_or(|end| end > extra.len()) {
            break;
        }
        let data = &extra[offset..offset + len];
        offset += len;
        if id != UNICODE_PATH_EXTRA {
            continue;
        }
        if data.len() < 5 {
            return Err(Zip2NameFailure::Unproved {
                index,
                detail: MessageDetail::message(
                    "journal.cli.transcribe.error.writer.zip.unicode.path.extra.too.small",
                    [],
                ),
            });
        }
        let claimed = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        if claimed != crc32_ieee(raw) {
            return Err(Zip2NameFailure::Unproved {
                index,
                detail: MessageDetail::message(
                    "journal.cli.transcribe.error.writer.zip.unicode.path.extra.crc.mismatch",
                    [],
                ),
            });
        }
        let payload = &data[5..];
        if std::str::from_utf8(payload).is_err() {
            return Err(Zip2NameFailure::Unproved {
                index,
                detail: MessageDetail::message(
                    "journal.cli.transcribe.error.writer.zip.unicode.path.extra.is.not.utf8",
                    [],
                ),
            });
        }
        raw.clear();
        raw.extend_from_slice(payload);
        overridden = true;
    }
    Ok(overridden)
}

fn decode_identity(index: usize, flags: u16, raw: &[u8]) -> Result<String, Zip2NameFailure> {
    if flags & UTF8_FLAG != 0 {
        return String::from_utf8(raw.to_vec()).map_err(|_| Zip2NameFailure::Unproved {
            index,
            detail: MessageDetail::message(
                "journal.cli.transcribe.error.writer.zip.flagged.name.is.not.utf8",
                [],
            ),
        });
    }
    let mut name = String::new();
    for &byte in raw {
        if byte < 0x80 {
            name.push(char::from(byte));
        } else {
            let scalar = CP437_HIGH[usize::from(byte) - 0x80];
            let Some(character) = char::from_u32(scalar) else {
                return Err(Zip2NameFailure::Unproved {
                    index,
                    detail: MessageDetail::message(
                        "journal.cli.transcribe.error.writer.zip.cp437.scalar",
                        [],
                    ),
                });
            };
            name.push(character);
        }
    }
    Ok(name)
}

fn read_at<R: Read + Seek>(reader: &mut R, start: u64, len: usize) -> io::Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0_u8; len];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::{
        CP437_HIGH, UNICODE_PATH_EXTRA, UTF8_FLAG, Zip2NameFailure, crc32_ieee, prove_zip2_names,
    };
    use std::io::Cursor;

    const LOCAL_LEN: usize = 30;
    const CENTRAL_LEN: usize = 46;

    struct Member {
        name: Vec<u8>,
        flags: u16,
        extra: Vec<u8>,
        data: Vec<u8>,
    }

    fn stored_crc(data: &[u8]) -> u32 {
        crc32_ieee(data)
    }

    fn extra_7075(raw_name: &[u8], payload: &[u8], crc: Option<u32>) -> Vec<u8> {
        let crc = crc.unwrap_or_else(|| crc32_ieee(raw_name));
        let mut extra = Vec::new();
        extra.extend_from_slice(&UNICODE_PATH_EXTRA.to_le_bytes());
        let data_len = 1 + 4 + payload.len();
        extra.extend_from_slice(&(data_len as u16).to_le_bytes());
        extra.push(1);
        extra.extend_from_slice(&crc.to_le_bytes());
        extra.extend_from_slice(payload);
        extra
    }

    fn build_zip(members: &[Member]) -> (Vec<u8>, Vec<(u64, u64)>) {
        let mut local = Vec::new();
        let mut coords = Vec::new();
        let mut local_offsets = Vec::new();
        for member in members {
            let header_start = local.len() as u64;
            local_offsets.push(header_start);
            let crc = stored_crc(&member.data);
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
            debug_assert_eq!(
                local.len(),
                header_start as usize + LOCAL_LEN + member.name.len() + member.data.len()
            );
        }
        let mut central = Vec::new();
        let cd_start = local.len() as u64;
        for (index, member) in members.iter().enumerate() {
            let central_start = cd_start + central.len() as u64;
            coords.push((local_offsets[index], central_start));
            let crc = stored_crc(&member.data);
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
            central.extend_from_slice(&(local_offsets[index] as u32).to_le_bytes());
            central.extend_from_slice(&member.name);
            central.extend_from_slice(&member.extra);
            debug_assert_eq!(
                central.len(),
                (central_start - cd_start) as usize
                    + CENTRAL_LEN
                    + member.name.len()
                    + member.extra.len()
            );
        }
        let mut bytes = local;
        bytes.extend_from_slice(&central);
        let cd_size = central.len() as u32;
        let count = members.len() as u16;
        bytes.extend_from_slice(b"PK\x05\x06");
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&cd_size.to_le_bytes());
        bytes.extend_from_slice(&(cd_start as u32).to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        (bytes, coords)
    }

    fn prove(members: &[Member]) -> Result<Vec<String>, Zip2NameFailure> {
        let (bytes, coords) = build_zip(members);
        prove_zip2_names(&mut Cursor::new(bytes), &coords)
    }

    fn ascii(name: &str) -> Member {
        Member {
            name: name.as_bytes().to_vec(),
            flags: UTF8_FLAG,
            extra: Vec::new(),
            data: b"ok".to_vec(),
        }
    }

    #[test]
    fn crc32_ieee_123456789() {
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn prove_zip2_names_flagged_utf8() {
        let names = prove(&[ascii("café.txt")]).unwrap();
        assert_eq!(names, ["café.txt"]);
    }

    #[test]
    fn prove_zip2_names_unflagged_cp437() {
        let mut name = b"Cura".to_vec();
        name.push(0x87);
        name.extend_from_slice(b"ao.txt");
        let names = prove(&[Member {
            name,
            flags: 0,
            extra: Vec::new(),
            data: b"x".to_vec(),
        }])
        .unwrap();
        assert_eq!(names, ["Curaçao.txt"]);
        let e_acute = prove(&[Member {
            name: vec![0x82],
            flags: 0,
            extra: Vec::new(),
            data: b"x".to_vec(),
        }])
        .unwrap();
        assert_eq!(e_acute, ["é"]);
    }

    #[test]
    fn prove_zip2_names_unflagged_c3_a9_is_box_drawings() {
        let names = prove(&[Member {
            name: vec![0xC3, 0xA9],
            flags: 0,
            extra: Vec::new(),
            data: b"x".to_vec(),
        }])
        .unwrap();
        assert_eq!(names, ["├⌐"]);
    }

    #[test]
    fn prove_zip2_names_zip9_only_collapse_keeps_two_identities() {
        let names = prove(&[
            Member {
                name: vec![0xC3, 0xA9],
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
        .unwrap();
        assert_eq!(names, ["├⌐", "é"]);
    }

    #[test]
    fn prove_zip2_names_true_identity_collision() {
        let error = prove(&[ascii("same.txt"), ascii("same.txt")]).unwrap_err();
        assert_eq!(
            error,
            Zip2NameFailure::Collision {
                name: "same.txt".to_owned()
            }
        );
    }

    #[test]
    fn prove_zip2_names_valid_0x7075() {
        let raw = b"raw".to_vec();
        let extra = extra_7075(&raw, "unicode.txt".as_bytes(), None);
        let names = prove(&[Member {
            name: raw,
            flags: 0,
            extra,
            data: b"x".to_vec(),
        }])
        .unwrap();
        assert_eq!(names, ["unicode.txt"]);
    }

    #[test]
    fn prove_zip2_names_crc_invalid_0x7075() {
        let raw = b"raw".to_vec();
        let extra = extra_7075(&raw, b"unicode.txt", Some(0));
        let error = prove(&[Member {
            name: raw,
            flags: 0,
            extra,
            data: b"x".to_vec(),
        }])
        .unwrap_err();
        assert!(matches!(error, Zip2NameFailure::Unproved { index: 0, .. }));
    }

    #[test]
    fn prove_zip2_names_utf8_invalid_0x7075() {
        let raw = b"raw".to_vec();
        let extra = extra_7075(&raw, &[0xFF], None);
        let error = prove(&[Member {
            name: raw,
            flags: 0,
            extra,
            data: b"x".to_vec(),
        }])
        .unwrap_err();
        assert!(matches!(error, Zip2NameFailure::Unproved { index: 0, .. }));
    }

    #[test]
    fn prove_zip2_names_truncated_or_bad_signature_is_unreadable() {
        let (mut bytes, coords) = build_zip(&[ascii("ok.txt")]);
        bytes.truncate(10);
        let error = prove_zip2_names(&mut Cursor::new(bytes), &coords).unwrap_err();
        assert!(matches!(error, Zip2NameFailure::Unreadable { .. }));

        let (mut bytes, coords) = build_zip(&[ascii("ok.txt")]);
        bytes[coords[0].1 as usize] = b'X';
        let error = prove_zip2_names(&mut Cursor::new(bytes), &coords).unwrap_err();
        assert!(matches!(error, Zip2NameFailure::Unreadable { .. }));
    }

    #[test]
    fn empty_proved_name_stays_empty() {
        let names = prove(&[Member {
            name: Vec::new(),
            flags: UTF8_FLAG,
            extra: Vec::new(),
            data: Vec::new(),
        }])
        .unwrap();
        assert_eq!(names, [""]);
    }

    #[test]
    fn cp437_high_table_covers_required_scalars() {
        assert_eq!(CP437_HIGH[0x82 - 0x80], 0x00E9);
        assert_eq!(CP437_HIGH[0x87 - 0x80], 0x00E7);
        assert_eq!(CP437_HIGH[0xA9 - 0x80], 0x2310);
        assert_eq!(CP437_HIGH[0xC3 - 0x80], 0x251C);
    }
}
