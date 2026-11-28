// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded container sniffing for ZIP/OOXML and MS-CFB (OLE2) containers.
//!
//! Inspects archive directory entries without decompressing payload streams so
//! misnamed files, unhinted byte buffers, `.xlsb` binary workbooks, and
//! password-protected OOXML packages route deterministically before parsing.

use std::collections::HashSet;
use std::io::Cursor;

use crate::{DocumentFormat, ExtractError, ExtractLimits};

const ZIP_LOCAL_MAGIC: &[u8; 4] = b"PK\x03\x04";
const ZIP_EMPTY_MAGIC: &[u8; 4] = b"PK\x05\x06";
const ZIP_SPANNED_MAGIC: &[u8; 4] = b"PK\x07\x08";
const CFB_MAGIC: &[u8; 8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

const CFB_ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const CFB_FREESECT: u32 = 0xFFFF_FFFF;
const CFB_FATSECT: u32 = 0xFFFF_FFFD;
const CFB_DIFSECT: u32 = 0xFFFF_FFFC;
const CFB_MAX_REG_SECT: u32 = 0xFFFF_FFFA;
const CFB_DIR_ENTRY_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SniffedContainer {
    Ooxml(DocumentFormat),
    EncryptedOoxml,
    LegacyCfb(DocumentFormat),
    NonOfficeZip,
    NonOfficeCfb,
    Unknown,
}

pub(crate) fn sniff_container(
    bytes: &[u8],
    limits: &ExtractLimits,
) -> Result<SniffedContainer, ExtractError> {
    if is_zip_magic(bytes) {
        return sniff_zip(bytes, limits);
    }
    if bytes.starts_with(CFB_MAGIC) {
        return sniff_cfb(bytes, limits);
    }
    Ok(SniffedContainer::Unknown)
}

fn is_zip_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(ZIP_LOCAL_MAGIC)
        || bytes.starts_with(ZIP_EMPTY_MAGIC)
        || bytes.starts_with(ZIP_SPANNED_MAGIC)
}

fn sniff_zip(bytes: &[u8], limits: &ExtractLimits) -> Result<SniffedContainer, ExtractError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| ExtractError::Invalid(format!("invalid ZIP container: {err}")))?;

    let entry_count = archive.len();
    if entry_count > limits.max_package_entries {
        return Err(ExtractError::LimitExceeded(format!(
            "ZIP archive contains {entry_count} entries, exceeding limit of {}",
            limits.max_package_entries
        )));
    }

    let mut has_word = false;
    let mut has_xlsx = false;
    let mut has_xlsb = false;
    let mut has_pptx = false;

    for idx in 0..entry_count {
        let Some(Ok(name)) = archive.name_for_index(idx) else {
            continue;
        };
        let normalized = name
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_ascii_lowercase();
        match normalized.as_str() {
            "word/document.xml" => has_word = true,
            "xl/workbook.bin" => has_xlsb = true,
            "xl/workbook.xml" => has_xlsx = true,
            "ppt/presentation.xml" => has_pptx = true,
            _ => {}
        }
    }

    let detected = if has_word {
        Some(DocumentFormat::Docx)
    } else if has_xlsb {
        Some(DocumentFormat::Xlsb)
    } else if has_xlsx {
        Some(DocumentFormat::Xlsx)
    } else if has_pptx {
        Some(DocumentFormat::Pptx)
    } else {
        None
    };

    let Some(format) = detected else {
        return Ok(SniffedContainer::NonOfficeZip);
    };

    // Inspect declared uncompressed sizes from central-directory headers so
    // obvious zip bombs fail fast before entering any format reader.
    let mut total_declared: u64 = 0;
    for idx in 0..entry_count {
        if let Ok(entry) = archive.by_index_raw(idx) {
            let uncompressed = entry.size();
            if uncompressed > limits.max_part_bytes {
                return Err(ExtractError::LimitExceeded(format!(
                    "archive entry expands to {uncompressed} bytes, exceeding per-part limit of {}",
                    limits.max_part_bytes
                )));
            }
            total_declared = total_declared.saturating_add(uncompressed);
            if total_declared > limits.max_uncompressed_bytes {
                return Err(ExtractError::LimitExceeded(format!(
                    "archive uncompressed total exceeds limit of {} bytes",
                    limits.max_uncompressed_bytes
                )));
            }
        }
    }

    Ok(SniffedContainer::Ooxml(format))
}

/// Inspect MS-CFB (v3 / v4) directory entries safely without external crates
/// or recursion.
fn sniff_cfb(bytes: &[u8], limits: &ExtractLimits) -> Result<SniffedContainer, ExtractError> {
    if bytes.len() < 512 {
        return Err(ExtractError::Invalid(
            "compound file is shorter than 512-byte header".to_owned(),
        ));
    }

    let major_version = read_u16_le(bytes, 0x1A)?;
    let sector_shift = read_u16_le(bytes, 0x1E)?;
    let sector_size: usize = match (major_version, sector_shift) {
        (3, 9) => 512,
        (4, 12) => 4096,
        _ => {
            return Err(ExtractError::Invalid(format!(
                "unsupported compound file version {major_version} / sector shift {sector_shift}"
            )));
        }
    };
    if bytes.len() < sector_size {
        return Err(ExtractError::Invalid(
            "compound file is truncated inside header sector".to_owned(),
        ));
    }

    let first_dir_sector = read_u32_le(bytes, 0x30)?;
    let first_difat_sector = read_u32_le(bytes, 0x44)?;
    let num_difat_sectors = read_u32_le(bytes, 0x48)?;
    let num_fat_sectors = read_u32_le(bytes, 0x2C)?;

    let fat_sectors = collect_fat_sectors(
        bytes,
        sector_size,
        num_fat_sectors,
        first_difat_sector,
        num_difat_sectors,
    )?;

    let names =
        read_cfb_directory_names(bytes, sector_size, first_dir_sector, &fat_sectors, limits)?;

    let mut has_encryption_info = false;
    let mut has_encrypted_package = false;
    let mut has_word = false;
    let mut has_workbook = false;
    let mut has_ppt = false;

    for name in &names {
        if name.eq_ignore_ascii_case("EncryptionInfo") {
            has_encryption_info = true;
        } else if name.eq_ignore_ascii_case("EncryptedPackage") {
            has_encrypted_package = true;
        } else if name.eq_ignore_ascii_case("WordDocument") {
            has_word = true;
        } else if name.eq_ignore_ascii_case("Workbook") || name.eq_ignore_ascii_case("Book") {
            has_workbook = true;
        } else if name.eq_ignore_ascii_case("PowerPoint Document") {
            has_ppt = true;
        }
    }

    if has_encrypted_package || (has_encryption_info && !has_word && !has_workbook && !has_ppt) {
        Ok(SniffedContainer::EncryptedOoxml)
    } else if has_word {
        Ok(SniffedContainer::LegacyCfb(DocumentFormat::Doc))
    } else if has_workbook {
        Ok(SniffedContainer::LegacyCfb(DocumentFormat::Xls))
    } else if has_ppt {
        Ok(SniffedContainer::LegacyCfb(DocumentFormat::Ppt))
    } else {
        Ok(SniffedContainer::NonOfficeCfb)
    }
}

fn sector_offset(sector: u32, sector_size: usize) -> Option<usize> {
    let idx = usize::try_from(sector).ok()?;
    idx.checked_add(1)?.checked_mul(sector_size)
}

fn sector_slice(bytes: &[u8], sector: u32, sector_size: usize) -> Option<&[u8]> {
    let start = sector_offset(sector, sector_size)?;
    let end = start.checked_add(sector_size)?;
    bytes.get(start..end)
}

fn collect_fat_sectors(
    bytes: &[u8],
    sector_size: usize,
    num_fat_sectors: u32,
    first_difat_sector: u32,
    num_difat_sectors: u32,
) -> Result<Vec<u32>, ExtractError> {
    let max_possible_sectors = bytes.len() / sector_size;
    let target = usize::try_from(num_fat_sectors)
        .unwrap_or(usize::MAX)
        .min(max_possible_sectors);
    let mut fat_sectors = Vec::with_capacity(target.min(109));

    // First 109 DIFAT entries live inside the 512-byte header at 0x4C..0x200.
    for i in 0..109 {
        let sector = read_u32_le(bytes, 0x4C + i * 4)?;
        if sector <= CFB_MAX_REG_SECT {
            fat_sectors.push(sector);
            if fat_sectors.len() >= target {
                return Ok(fat_sectors);
            }
        }
    }

    // Follow optional DIFAT chain, bounded and cycle-checked.
    let entries_per_difat = (sector_size / 4).saturating_sub(1);
    let mut current = first_difat_sector;
    let mut visited = HashSet::new();
    let max_hops = usize::try_from(num_difat_sectors)
        .unwrap_or(0)
        .min(max_possible_sectors);

    for _ in 0..max_hops {
        if current > CFB_MAX_REG_SECT || !visited.insert(current) {
            break;
        }
        let Some(sec) = sector_slice(bytes, current, sector_size) else {
            return Err(ExtractError::Invalid(
                "compound file DIFAT sector out of bounds".to_owned(),
            ));
        };
        for i in 0..entries_per_difat {
            let fat_sec = read_u32_le(sec, i * 4)?;
            if fat_sec <= CFB_MAX_REG_SECT {
                fat_sectors.push(fat_sec);
                if fat_sectors.len() >= target {
                    return Ok(fat_sectors);
                }
            }
        }
        current = read_u32_le(sec, entries_per_difat * 4)?;
    }

    Ok(fat_sectors)
}

fn fat_next_sector(
    bytes: &[u8],
    sector_size: usize,
    fat_sectors: &[u32],
    sector: u32,
) -> Option<u32> {
    let entries_per_fat = sector_size / 4;
    let idx = usize::try_from(sector).ok()?;
    let fat_index = idx / entries_per_fat;
    let entry_index = idx % entries_per_fat;
    let fat_sector = *fat_sectors.get(fat_index)?;
    let sec = sector_slice(bytes, fat_sector, sector_size)?;
    read_u32_le(sec, entry_index * 4).ok()
}

struct CfbDirEntry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
}

fn read_cfb_directory_names(
    bytes: &[u8],
    sector_size: usize,
    first_dir_sector: u32,
    fat_sectors: &[u32],
    limits: &ExtractLimits,
) -> Result<Vec<String>, ExtractError> {
    if first_dir_sector > CFB_MAX_REG_SECT {
        return Err(ExtractError::Invalid(
            "compound file has invalid first directory sector".to_owned(),
        ));
    }

    let entries_per_sector = sector_size / CFB_DIR_ENTRY_BYTES;
    let mut visited = HashSet::new();
    let mut current = first_dir_sector;
    let mut entries: Vec<Option<CfbDirEntry>> = Vec::new();
    let mut inspected_entries: usize = 0;

    while current <= CFB_MAX_REG_SECT {
        if !visited.insert(current) {
            return Err(ExtractError::Invalid(
                "compound file directory chain contains a cycle".to_owned(),
            ));
        }
        let Some(sec) = sector_slice(bytes, current, sector_size) else {
            return Err(ExtractError::Invalid(
                "compound file directory sector out of bounds".to_owned(),
            ));
        };

        for slot in 0..entries_per_sector {
            let offset = slot * CFB_DIR_ENTRY_BYTES;
            let entry = &sec[offset..offset + CFB_DIR_ENTRY_BYTES];
            let obj_type = entry[0x42];
            if obj_type == 0 {
                entries.push(None);
                continue;
            }
            inspected_entries += 1;
            if inspected_entries > limits.max_package_entries {
                return Err(ExtractError::LimitExceeded(format!(
                    "compound file directory exceeds entry limit of {}",
                    limits.max_package_entries
                )));
            }
            let name_len = usize::from(read_u16_le(entry, 0x40)?);
            let mut decoded_name = String::new();
            if (2..=64).contains(&name_len) && name_len % 2 == 0 {
                let code_units = (name_len / 2).saturating_sub(1);
                let mut utf16 = Vec::with_capacity(code_units);
                for u in 0..code_units {
                    utf16.push(read_u16_le(entry, u * 2)?);
                }
                if let Ok(decoded) = String::from_utf16(&utf16) {
                    decoded_name = decoded;
                }
            }
            entries.push(Some(CfbDirEntry {
                name: decoded_name,
                kind: obj_type,
                left: read_u32_le(entry, 0x44)?,
                right: read_u32_le(entry, 0x48)?,
                child: read_u32_le(entry, 0x4C)?,
            }));
        }

        match fat_next_sector(bytes, sector_size, fat_sectors, current) {
            Some(CFB_ENDOFCHAIN | CFB_FREESECT) | None => break,
            Some(CFB_FATSECT | CFB_DIFSECT) => {
                return Err(ExtractError::Invalid(
                    "compound file directory chain points to reserved sector".to_owned(),
                ));
            }
            Some(next) => current = next,
        }
    }

    // Prefer walking only Root Entry's top-level red-black sibling tree so a
    // nested OLE object (e.g., an embedded Word storage inside a PowerPoint
    // deck) cannot shadow the outer document's top-level streams.
    if let Some(Some(root)) = entries.first()
        && root.kind == 5
        && root.child != CFB_FREESECT
    {
        let mut top_level = Vec::new();
        let mut stack = vec![root.child];
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if id == CFB_FREESECT || !seen.insert(id) {
                continue;
            }
            let Ok(idx) = usize::try_from(id) else {
                continue;
            };
            let Some(Some(node)) = entries.get(idx) else {
                continue;
            };
            if !node.name.is_empty() {
                top_level.push(node.name.clone());
            }
            if node.left != CFB_FREESECT {
                stack.push(node.left);
            }
            if node.right != CFB_FREESECT {
                stack.push(node.right);
            }
        }
        if !top_level.is_empty() {
            return Ok(top_level);
        }
    }

    // Fallback to flat directory scan if Root Entry's child tree is missing.
    Ok(entries
        .into_iter()
        .flatten()
        .filter(|e| !e.name.is_empty())
        .map(|e| e.name)
        .collect())
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, ExtractError> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| ExtractError::Invalid("unexpected end of compound file".to_owned()))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, ExtractError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ExtractError::Invalid("unexpected end of compound file".to_owned()))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
