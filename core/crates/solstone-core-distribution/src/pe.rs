// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! PE32+ inspection for Windows distribution artifacts.
//!
//! Thin little-endian PE32+ only. PE32 is refused by name. The parser records
//! machine, imports, exports, and the first debug-directory entry; it does not
//! inspect Authenticode signatures.

use std::collections::BTreeMap;

pub const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
pub const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;

const IMAGE_FILE_EXECUTABLE_IMAGE: u16 = 0x0002;
const IMAGE_FILE_DLL: u16 = 0x2000;
const PE32_MAGIC: u16 = 0x10b;
const PE32PLUS_MAGIC: u16 = 0x20b;
const DOS_E_LFANEW: usize = 0x3c;
const PE_SIGNATURE: &[u8; 4] = b"PE\0\0";
const COFF_LEN: usize = 20;
const OPT_STANDARD_LEN: usize = 112;
const DATA_DIRECTORY_ENTRY_LEN: usize = 8;
const DIR_EXPORT: u32 = 0;
const DIR_IMPORT: u32 = 1;
const DIR_DEBUG: u32 = 6;
const SECTION_HEADER_LEN: usize = 40;
const IMPORT_DESCRIPTOR_LEN: usize = 20;
const EXPORT_DIRECTORY_LEN: usize = 40;
const DEBUG_DIRECTORY_LEN: usize = 28;
const THUNK_LEN: usize = 8;
const ORDINAL_FLAG64: u64 = 0x8000_0000_0000_0000;
const E_LFANEW_VALUE: u32 = 0x40;
const FIXTURE_OPT_SIZE: u16 = 240;
const FIXTURE_DIR_COUNT: u32 = 16;
const FIXTURE_RVA_BASE: u32 = 0x1000;
const FIXTURE_COFF_OFF: usize = E_LFANEW_VALUE as usize + 4;
const FIXTURE_OPT_OFF: usize = FIXTURE_COFF_OFF + COFF_LEN;
const FIXTURE_DATA_DIR_OFF: usize = FIXTURE_OPT_OFF + OPT_STANDARD_LEN;
const FIXTURE_SECTION_OFF: usize = FIXTURE_OPT_OFF + FIXTURE_OPT_SIZE as usize;
const FIXTURE_SECTION_RAW: u32 = (FIXTURE_SECTION_OFF + SECTION_HEADER_LEN) as u32;

const NOT_A_PE: &str = "unexpected:\n  not a pe file";
const TRUNCATED_PE: &str = "unexpected:\n  truncated pe";
const PE32: &str = "unexpected:\n  pe32";
const OOB_IMPORT: &str = "unexpected:\n  out-of-bounds import directory";
const OOB_EXPORT: &str = "unexpected:\n  out-of-bounds export directory";
const OOB_DEBUG: &str = "unexpected:\n  out-of-bounds debug directory";
const IMPORT_NOT_UTF8: &str = "unexpected:\n  import name not utf-8";
const EXPORT_NOT_UTF8: &str = "unexpected:\n  export name not utf-8";
const EXPORT_ORDINAL_OVERFLOW: &str = "unexpected:\n  export ordinal overflow";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeInfo {
    pub machine: u16,
    pub imports: Vec<ImportedLibrary>,
    pub exports: Vec<PeSymbol>,
    pub debug: Option<DebugInfoKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedLibrary {
    pub name: String,
    pub symbols: Vec<PeSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeSymbol {
    Named(String),
    Ordinal(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugInfoKind {
    Coff,
    CodeView,
    Repro,
    Other(u32),
}

#[derive(Debug)]
pub struct PeError {
    message: String,
}

impl PeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PeError {}

#[must_use]
pub const fn machine_amd64() -> u16 {
    IMAGE_FILE_MACHINE_AMD64
}

#[must_use]
pub const fn machine_arm64() -> u16 {
    IMAGE_FILE_MACHINE_ARM64
}

struct Section {
    virtual_size: u32,
    virtual_address: u32,
    size_of_raw_data: u32,
    pointer_to_raw_data: u32,
}

pub fn parse_pe(bytes: &[u8]) -> Result<PeInfo, PeError> {
    if bytes.get(0..2) != Some(&b"MZ"[..]) {
        return Err(PeError::new(NOT_A_PE));
    }
    let e_lfanew = match bytes.get(DOS_E_LFANEW..DOS_E_LFANEW + 4) {
        Some(slice) => {
            u32::from_le_bytes(slice.try_into().map_err(|_| PeError::new(NOT_A_PE))?) as usize
        }
        None => return Err(PeError::new(NOT_A_PE)),
    };
    match bytes.get(e_lfanew..e_lfanew + 4) {
        Some(signature) if signature == PE_SIGNATURE => {}
        _ => return Err(PeError::new(NOT_A_PE)),
    }

    let coff_off = e_lfanew + 4;
    let machine = read_u16(bytes, coff_off)?;
    let number_of_sections = read_u16(bytes, coff_off + 2)?;
    let size_of_optional_header = read_u16(bytes, coff_off + 16)?;
    let opt_off = coff_off
        .checked_add(COFF_LEN)
        .ok_or_else(|| PeError::new(TRUNCATED_PE))?;
    if size_of_optional_header < 2 {
        return Err(PeError::new(TRUNCATED_PE));
    }
    let magic = read_u16(bytes, opt_off)?;
    if magic != PE32PLUS_MAGIC {
        return Err(PeError::new(PE32));
    }
    if size_of_optional_header < OPT_STANDARD_LEN as u16 {
        return Err(PeError::new(TRUNCATED_PE));
    }
    let opt_end = opt_off
        .checked_add(size_of_optional_header as usize)
        .ok_or_else(|| PeError::new(TRUNCATED_PE))?;
    if opt_end > bytes.len() {
        return Err(PeError::new(TRUNCATED_PE));
    }
    let number_of_rva_and_sizes = read_u32(bytes, opt_off + 108)?;

    let section_off = opt_end;
    let section_bytes = (number_of_sections as usize)
        .checked_mul(SECTION_HEADER_LEN)
        .ok_or_else(|| PeError::new(TRUNCATED_PE))?;
    let section_end = section_off
        .checked_add(section_bytes)
        .ok_or_else(|| PeError::new(TRUNCATED_PE))?;
    if section_end > bytes.len() {
        return Err(PeError::new(TRUNCATED_PE));
    }
    let mut sections = Vec::with_capacity(number_of_sections as usize);
    for index in 0..number_of_sections as usize {
        let off = section_off + index * SECTION_HEADER_LEN;
        sections.push(Section {
            virtual_size: read_u32(bytes, off + 8)?,
            virtual_address: read_u32(bytes, off + 12)?,
            size_of_raw_data: read_u32(bytes, off + 16)?,
            pointer_to_raw_data: read_u32(bytes, off + 20)?,
        });
    }

    let imports = parse_imports(
        bytes,
        &sections,
        data_directory(
            bytes,
            opt_off,
            size_of_optional_header,
            number_of_rva_and_sizes,
            DIR_IMPORT,
        )?,
    )?;
    let exports = parse_exports(
        bytes,
        &sections,
        data_directory(
            bytes,
            opt_off,
            size_of_optional_header,
            number_of_rva_and_sizes,
            DIR_EXPORT,
        )?,
    )?;
    let debug = parse_debug(
        bytes,
        &sections,
        data_directory(
            bytes,
            opt_off,
            size_of_optional_header,
            number_of_rva_and_sizes,
            DIR_DEBUG,
        )?,
    )?;
    Ok(PeInfo {
        machine,
        imports,
        exports,
        debug,
    })
}

fn data_directory(
    bytes: &[u8],
    opt_off: usize,
    size_of_optional_header: u16,
    number_of_rva_and_sizes: u32,
    index: u32,
) -> Result<Option<(u32, u32)>, PeError> {
    if index >= number_of_rva_and_sizes {
        return Ok(None);
    }
    let slot = OPT_STANDARD_LEN + index as usize * DATA_DIRECTORY_ENTRY_LEN;
    let slot_end = slot + DATA_DIRECTORY_ENTRY_LEN;
    if slot_end > size_of_optional_header as usize {
        return Ok(None);
    }
    let off = opt_off + slot;
    Ok(Some((read_u32(bytes, off)?, read_u32(bytes, off + 4)?)))
}

fn parse_imports(
    bytes: &[u8],
    sections: &[Section],
    directory: Option<(u32, u32)>,
) -> Result<Vec<ImportedLibrary>, PeError> {
    let Some((virtual_address, size)) = directory else {
        return Ok(Vec::new());
    };
    if virtual_address == 0 && size == 0 {
        return Ok(Vec::new());
    }
    let mut imports = Vec::new();
    let mut descriptor_rva = virtual_address;
    for _ in 0..=bytes.len() {
        let original_first_thunk = read_u32_at_rva(bytes, sections, descriptor_rva, OOB_IMPORT)?;
        let name_rva = read_u32_at_rva(bytes, sections, descriptor_rva + 12, OOB_IMPORT)?;
        let first_thunk = read_u32_at_rva(bytes, sections, descriptor_rva + 16, OOB_IMPORT)?;
        let time_date_stamp = read_u32_at_rva(bytes, sections, descriptor_rva + 4, OOB_IMPORT)?;
        let forwarder = read_u32_at_rva(bytes, sections, descriptor_rva + 8, OOB_IMPORT)?;
        if original_first_thunk == 0
            && time_date_stamp == 0
            && forwarder == 0
            && name_rva == 0
            && first_thunk == 0
        {
            break;
        }
        let name = read_cstr_at_rva(bytes, sections, name_rva, OOB_IMPORT, IMPORT_NOT_UTF8)?;
        let thunk_rva = if original_first_thunk != 0 {
            original_first_thunk
        } else {
            first_thunk
        };
        let symbols = parse_thunks(bytes, sections, thunk_rva)?;
        imports.push(ImportedLibrary { name, symbols });
        descriptor_rva = descriptor_rva
            .checked_add(IMPORT_DESCRIPTOR_LEN as u32)
            .ok_or_else(|| PeError::new(OOB_IMPORT))?;
    }
    Ok(imports)
}

fn parse_thunks(
    bytes: &[u8],
    sections: &[Section],
    mut thunk_rva: u32,
) -> Result<Vec<PeSymbol>, PeError> {
    let mut symbols = Vec::new();
    for _ in 0..=bytes.len() {
        let entry = read_u64_at_rva(bytes, sections, thunk_rva, OOB_IMPORT)?;
        if entry == 0 {
            break;
        }
        if entry & ORDINAL_FLAG64 != 0 {
            symbols.push(PeSymbol::Ordinal((entry & 0xffff) as u16));
        } else {
            let hint_name_rva = entry as u32;
            let name = read_cstr_at_rva(
                bytes,
                sections,
                hint_name_rva
                    .checked_add(2)
                    .ok_or_else(|| PeError::new(OOB_IMPORT))?,
                OOB_IMPORT,
                IMPORT_NOT_UTF8,
            )?;
            symbols.push(PeSymbol::Named(name));
        }
        thunk_rva = thunk_rva
            .checked_add(THUNK_LEN as u32)
            .ok_or_else(|| PeError::new(OOB_IMPORT))?;
    }
    Ok(symbols)
}

fn parse_exports(
    bytes: &[u8],
    sections: &[Section],
    directory: Option<(u32, u32)>,
) -> Result<Vec<PeSymbol>, PeError> {
    let Some((virtual_address, size)) = directory else {
        return Ok(Vec::new());
    };
    if virtual_address == 0 && size == 0 {
        return Ok(Vec::new());
    }
    let base = read_u32_at_rva(bytes, sections, virtual_address + 16, OOB_EXPORT)?;
    let number_of_functions = read_u32_at_rva(bytes, sections, virtual_address + 20, OOB_EXPORT)?;
    let number_of_names = read_u32_at_rva(bytes, sections, virtual_address + 24, OOB_EXPORT)?;
    let address_of_functions = read_u32_at_rva(bytes, sections, virtual_address + 28, OOB_EXPORT)?;
    let address_of_names = read_u32_at_rva(bytes, sections, virtual_address + 32, OOB_EXPORT)?;
    let address_of_name_ordinals =
        read_u32_at_rva(bytes, sections, virtual_address + 36, OOB_EXPORT)?;
    if number_of_functions as usize > bytes.len() || number_of_names as usize > bytes.len() {
        return Err(PeError::new(OOB_EXPORT));
    }

    let mut names_by_index = BTreeMap::new();
    for i in 0..number_of_names {
        let ordinal_rva = address_of_name_ordinals
            .checked_add(i.checked_mul(2).ok_or_else(|| PeError::new(OOB_EXPORT))?)
            .ok_or_else(|| PeError::new(OOB_EXPORT))?;
        let name_ptr_rva = address_of_names
            .checked_add(i.checked_mul(4).ok_or_else(|| PeError::new(OOB_EXPORT))?)
            .ok_or_else(|| PeError::new(OOB_EXPORT))?;
        let index = u32::from(read_u16_at_rva(bytes, sections, ordinal_rva, OOB_EXPORT)?);
        let name_rva = read_u32_at_rva(bytes, sections, name_ptr_rva, OOB_EXPORT)?;
        let name = read_cstr_at_rva(bytes, sections, name_rva, OOB_EXPORT, EXPORT_NOT_UTF8)?;
        names_by_index.insert(index, name);
    }

    let mut exports = Vec::new();
    for j in 0..number_of_functions {
        let function_rva = address_of_functions
            .checked_add(j.checked_mul(4).ok_or_else(|| PeError::new(OOB_EXPORT))?)
            .ok_or_else(|| PeError::new(OOB_EXPORT))?;
        let function = read_u32_at_rva(bytes, sections, function_rva, OOB_EXPORT)?;
        if function == 0 {
            continue;
        }
        if let Some(name) = names_by_index.get(&j) {
            exports.push(PeSymbol::Named(name.clone()));
            continue;
        }
        let ordinal = base
            .checked_add(j)
            .ok_or_else(|| PeError::new(EXPORT_ORDINAL_OVERFLOW))?;
        if ordinal > u32::from(u16::MAX) {
            return Err(PeError::new(EXPORT_ORDINAL_OVERFLOW));
        }
        exports.push(PeSymbol::Ordinal(ordinal as u16));
    }
    Ok(exports)
}

fn parse_debug(
    bytes: &[u8],
    sections: &[Section],
    directory: Option<(u32, u32)>,
) -> Result<Option<DebugInfoKind>, PeError> {
    let Some((virtual_address, size)) = directory else {
        return Ok(None);
    };
    if virtual_address == 0 && size == 0 {
        return Ok(None);
    }
    if size < DEBUG_DIRECTORY_LEN as u32 {
        return Err(PeError::new(OOB_DEBUG));
    }
    let kind = read_u32_at_rva(bytes, sections, virtual_address + 12, OOB_DEBUG)?;
    Ok(Some(match kind {
        1 => DebugInfoKind::Coff,
        2 => DebugInfoKind::CodeView,
        16 => DebugInfoKind::Repro,
        other => DebugInfoKind::Other(other),
    }))
}

fn rva_to_offset(sections: &[Section], rva: u32) -> Option<usize> {
    for section in sections {
        let span = section.virtual_size.max(section.size_of_raw_data);
        if span == 0 {
            continue;
        }
        let end = section.virtual_address.checked_add(span)?;
        if rva >= section.virtual_address && rva < end {
            let delta = rva - section.virtual_address;
            return section
                .pointer_to_raw_data
                .checked_add(delta)
                .map(|offset| offset as usize);
        }
    }
    None
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, PeError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| PeError::new(TRUNCATED_PE))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PeError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| PeError::new(TRUNCATED_PE))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, PeError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| PeError::new(TRUNCATED_PE))
}

fn read_u16_at_rva(
    bytes: &[u8],
    sections: &[Section],
    rva: u32,
    oob: &str,
) -> Result<u16, PeError> {
    let offset = rva_to_offset(sections, rva).ok_or_else(|| PeError::new(oob))?;
    read_u16(bytes, offset).map_err(|_| PeError::new(oob))
}

fn read_u32_at_rva(
    bytes: &[u8],
    sections: &[Section],
    rva: u32,
    oob: &str,
) -> Result<u32, PeError> {
    let offset = rva_to_offset(sections, rva).ok_or_else(|| PeError::new(oob))?;
    read_u32(bytes, offset).map_err(|_| PeError::new(oob))
}

fn read_u64_at_rva(
    bytes: &[u8],
    sections: &[Section],
    rva: u32,
    oob: &str,
) -> Result<u64, PeError> {
    let offset = rva_to_offset(sections, rva).ok_or_else(|| PeError::new(oob))?;
    read_u64(bytes, offset).map_err(|_| PeError::new(oob))
}

fn read_cstr_at_rva(
    bytes: &[u8],
    sections: &[Section],
    rva: u32,
    oob: &str,
    utf8: &str,
) -> Result<String, PeError> {
    let offset = rva_to_offset(sections, rva).ok_or_else(|| PeError::new(oob))?;
    let rest = bytes.get(offset..).ok_or_else(|| PeError::new(oob))?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| PeError::new(oob))?;
    std::str::from_utf8(&rest[..end])
        .map(str::to_owned)
        .map_err(|_| PeError::new(utf8))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeSymbolSpec<'a> {
    Named(&'a str),
    Ordinal(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSpec<'a> {
    pub name: &'a str,
    pub symbols: &'a [PeSymbolSpec<'a>],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureSpec<'a> {
    pub machine: u16,
    pub dll: bool,
    pub imports: &'a [ImportSpec<'a>],
    pub exports: &'a [PeSymbolSpec<'a>],
    pub debug: Option<DebugInfoKind>,
}

impl Default for FixtureSpec<'_> {
    fn default() -> Self {
        Self {
            machine: machine_amd64(),
            dll: false,
            imports: &[],
            exports: &[],
            debug: None,
        }
    }
}

#[must_use]
pub fn fixture(spec: &FixtureSpec<'_>) -> Vec<u8> {
    let mut payload = Vec::new();
    let mut export_dir = (0, 0);
    let mut import_dir = (0, 0);
    let mut debug_dir = (0, 0);
    if !spec.exports.is_empty() {
        export_dir = write_exports(&mut payload, spec.exports);
    }
    if !spec.imports.is_empty() {
        import_dir = write_imports(&mut payload, spec.imports);
    }
    if let Some(kind) = spec.debug {
        debug_dir = write_debug(&mut payload, kind);
    }
    let n_sections = u16::from(!payload.is_empty());
    let mut bytes = write_headers(spec.machine, spec.dll, n_sections);
    if n_sections == 1 {
        write_section_header(&mut bytes, payload.len() as u32);
        bytes.extend_from_slice(&payload);
    }
    patch_data_directory(&mut bytes, DIR_EXPORT as usize, export_dir);
    patch_data_directory(&mut bytes, DIR_IMPORT as usize, import_dir);
    patch_data_directory(&mut bytes, DIR_DEBUG as usize, debug_dir);
    bytes
}

#[must_use]
pub fn fixture_pe32() -> Vec<u8> {
    let mut bytes = vec![0_u8; FIXTURE_OPT_OFF + 2];
    bytes[0..2].copy_from_slice(b"MZ");
    bytes[DOS_E_LFANEW..DOS_E_LFANEW + 4].copy_from_slice(&E_LFANEW_VALUE.to_le_bytes());
    bytes[E_LFANEW_VALUE as usize..E_LFANEW_VALUE as usize + 4].copy_from_slice(PE_SIGNATURE);
    bytes[FIXTURE_COFF_OFF..FIXTURE_COFF_OFF + 2]
        .copy_from_slice(&IMAGE_FILE_MACHINE_AMD64.to_le_bytes());
    bytes[FIXTURE_COFF_OFF + 16..FIXTURE_COFF_OFF + 18].copy_from_slice(&2u16.to_le_bytes());
    bytes[FIXTURE_OPT_OFF..FIXTURE_OPT_OFF + 2].copy_from_slice(&PE32_MAGIC.to_le_bytes());
    bytes
}

#[must_use]
pub fn fixture_oob_import() -> Vec<u8> {
    fixture_oob_directory(DIR_IMPORT as usize, 20)
}

#[must_use]
pub fn fixture_oob_export() -> Vec<u8> {
    fixture_oob_directory(DIR_EXPORT as usize, 40)
}

#[must_use]
pub fn fixture_oob_debug() -> Vec<u8> {
    fixture_oob_directory(DIR_DEBUG as usize, 28)
}

fn fixture_oob_directory(index: usize, size: u32) -> Vec<u8> {
    let mut bytes = write_headers(machine_amd64(), false, 0);
    patch_data_directory(&mut bytes, index, (0x00ff_0000, size));
    bytes
}

fn write_headers(machine: u16, dll: bool, n_sections: u16) -> Vec<u8> {
    let mut bytes = vec![0_u8; FIXTURE_SECTION_OFF + n_sections as usize * SECTION_HEADER_LEN];
    bytes[0..2].copy_from_slice(b"MZ");
    bytes[DOS_E_LFANEW..DOS_E_LFANEW + 4].copy_from_slice(&E_LFANEW_VALUE.to_le_bytes());
    bytes[E_LFANEW_VALUE as usize..E_LFANEW_VALUE as usize + 4].copy_from_slice(PE_SIGNATURE);
    bytes[FIXTURE_COFF_OFF..FIXTURE_COFF_OFF + 2].copy_from_slice(&machine.to_le_bytes());
    bytes[FIXTURE_COFF_OFF + 2..FIXTURE_COFF_OFF + 4].copy_from_slice(&n_sections.to_le_bytes());
    bytes[FIXTURE_COFF_OFF + 16..FIXTURE_COFF_OFF + 18]
        .copy_from_slice(&FIXTURE_OPT_SIZE.to_le_bytes());
    let mut characteristics = IMAGE_FILE_EXECUTABLE_IMAGE;
    if dll {
        characteristics |= IMAGE_FILE_DLL;
    }
    bytes[FIXTURE_COFF_OFF + 18..FIXTURE_COFF_OFF + 20]
        .copy_from_slice(&characteristics.to_le_bytes());
    bytes[FIXTURE_OPT_OFF..FIXTURE_OPT_OFF + 2].copy_from_slice(&PE32PLUS_MAGIC.to_le_bytes());
    bytes[FIXTURE_OPT_OFF + 108..FIXTURE_OPT_OFF + 112]
        .copy_from_slice(&FIXTURE_DIR_COUNT.to_le_bytes());
    bytes
}

fn write_section_header(bytes: &mut [u8], payload_len: u32) {
    let off = FIXTURE_SECTION_OFF;
    bytes[off..off + 8].copy_from_slice(b".rdata\0\0");
    bytes[off + 8..off + 12].copy_from_slice(&payload_len.to_le_bytes());
    bytes[off + 12..off + 16].copy_from_slice(&FIXTURE_RVA_BASE.to_le_bytes());
    bytes[off + 16..off + 20].copy_from_slice(&payload_len.to_le_bytes());
    bytes[off + 20..off + 24].copy_from_slice(&FIXTURE_SECTION_RAW.to_le_bytes());
}

fn patch_data_directory(bytes: &mut [u8], index: usize, dir: (u32, u32)) {
    let off = FIXTURE_DATA_DIR_OFF + index * DATA_DIRECTORY_ENTRY_LEN;
    bytes[off..off + 4].copy_from_slice(&dir.0.to_le_bytes());
    bytes[off + 4..off + 8].copy_from_slice(&dir.1.to_le_bytes());
}

fn current_rva(payload: &[u8]) -> u32 {
    FIXTURE_RVA_BASE + payload.len() as u32
}

fn pad_to(payload: &mut Vec<u8>, align: usize) {
    while !payload.len().is_multiple_of(align) {
        payload.push(0);
    }
}

fn append_cstr(payload: &mut Vec<u8>, value: &str) -> u32 {
    let rva = current_rva(payload);
    payload.extend_from_slice(value.as_bytes());
    payload.push(0);
    rva
}

fn write_imports(payload: &mut Vec<u8>, imports: &[ImportSpec<'_>]) -> (u32, u32) {
    let mut names = Vec::new();
    let mut symbol_name_rvas = Vec::new();
    for library in imports {
        names.push(append_cstr(payload, library.name));
        let mut rvas = Vec::new();
        for symbol in library.symbols {
            match symbol {
                PeSymbolSpec::Named(name) => {
                    pad_to(payload, 2);
                    let rva = current_rva(payload);
                    payload.extend_from_slice(&0u16.to_le_bytes());
                    payload.extend_from_slice(name.as_bytes());
                    payload.push(0);
                    rvas.push(rva);
                }
                PeSymbolSpec::Ordinal(_) => rvas.push(0),
            }
        }
        symbol_name_rvas.push(rvas);
    }

    let mut thunk_rvas = Vec::new();
    for (library, name_rvas) in imports.iter().zip(&symbol_name_rvas) {
        pad_to(payload, 8);
        thunk_rvas.push(current_rva(payload));
        for (symbol, name_rva) in library.symbols.iter().zip(name_rvas) {
            let entry = match symbol {
                PeSymbolSpec::Ordinal(ordinal) => ORDINAL_FLAG64 | u64::from(*ordinal),
                PeSymbolSpec::Named(_) => u64::from(*name_rva),
            };
            payload.extend_from_slice(&entry.to_le_bytes());
        }
        payload.extend_from_slice(&0u64.to_le_bytes());
    }

    pad_to(payload, 4);
    let descriptor_rva = current_rva(payload);
    for (name_rva, thunk_rva) in names.iter().zip(&thunk_rvas) {
        payload.extend_from_slice(&thunk_rva.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&name_rva.to_le_bytes());
        payload.extend_from_slice(&thunk_rva.to_le_bytes());
    }
    payload.extend_from_slice(&[0_u8; IMPORT_DESCRIPTOR_LEN]);
    (
        descriptor_rva,
        ((imports.len() + 1) * IMPORT_DESCRIPTOR_LEN) as u32,
    )
}

fn write_exports(payload: &mut Vec<u8>, exports: &[PeSymbolSpec<'_>]) -> (u32, u32) {
    pad_to(payload, 4);
    let dir_rva = current_rva(payload);
    payload.extend_from_slice(&[0_u8; EXPORT_DIRECTORY_LEN]);
    let dummy_rva = current_rva(payload);
    payload.push(1);

    let base = 1u32;
    let mut named = Vec::new();
    let mut ordinal_only = Vec::new();
    for export in exports {
        match *export {
            PeSymbolSpec::Named(name) => named.push(name),
            PeSymbolSpec::Ordinal(ordinal) => ordinal_only.push(ordinal),
        }
    }
    let mut n_functions = named.len() as u32;
    for ordinal in &ordinal_only {
        let index = u32::from(*ordinal).saturating_sub(base);
        n_functions = n_functions.max(index + 1);
    }
    let mut occupied = vec![false; n_functions as usize];
    for slot in occupied.iter_mut().take(named.len()) {
        *slot = true;
    }
    for ordinal in &ordinal_only {
        let index = u32::from(*ordinal).saturating_sub(base) as usize;
        if index < occupied.len() {
            occupied[index] = true;
        }
    }

    pad_to(payload, 4);
    let functions_rva = current_rva(payload);
    for occupied_slot in &occupied {
        let rva = if *occupied_slot { dummy_rva } else { 0 };
        payload.extend_from_slice(&rva.to_le_bytes());
    }

    let mut name_rvas = Vec::new();
    for name in &named {
        name_rvas.push(append_cstr(payload, name));
    }
    pad_to(payload, 4);
    let names_rva = current_rva(payload);
    for name_rva in &name_rvas {
        payload.extend_from_slice(&name_rva.to_le_bytes());
    }
    pad_to(payload, 2);
    let ordinals_rva = current_rva(payload);
    for index in 0..named.len() {
        payload.extend_from_slice(&(index as u16).to_le_bytes());
    }

    let off = (dir_rva - FIXTURE_RVA_BASE) as usize;
    payload[off + 16..off + 20].copy_from_slice(&base.to_le_bytes());
    payload[off + 20..off + 24].copy_from_slice(&n_functions.to_le_bytes());
    payload[off + 24..off + 28].copy_from_slice(&(named.len() as u32).to_le_bytes());
    payload[off + 28..off + 32].copy_from_slice(&functions_rva.to_le_bytes());
    payload[off + 32..off + 36].copy_from_slice(&names_rva.to_le_bytes());
    payload[off + 36..off + 40].copy_from_slice(&ordinals_rva.to_le_bytes());
    (dir_rva, EXPORT_DIRECTORY_LEN as u32)
}

fn write_debug(payload: &mut Vec<u8>, kind: DebugInfoKind) -> (u32, u32) {
    pad_to(payload, 4);
    let rva = current_rva(payload);
    let debug_type = match kind {
        DebugInfoKind::Coff => 1,
        DebugInfoKind::CodeView => 2,
        DebugInfoKind::Repro => 16,
        DebugInfoKind::Other(value) => value,
    };
    let mut entry = vec![0_u8; DEBUG_DIRECTORY_LEN];
    entry[12..16].copy_from_slice(&debug_type.to_le_bytes());
    payload.extend_from_slice(&entry);
    (rva, DEBUG_DIRECTORY_LEN as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_import_export_and_debug_it_asserts_on() {
        let bytes = fixture(&FixtureSpec {
            dll: true,
            imports: &[
                ImportSpec {
                    name: "one.dll",
                    symbols: &[PeSymbolSpec::Named("CreateThing"), PeSymbolSpec::Ordinal(7)],
                },
                ImportSpec {
                    name: "empty.dll",
                    symbols: &[],
                },
            ],
            exports: &[
                PeSymbolSpec::Named("ExportA"),
                PeSymbolSpec::Named("ExportB"),
                PeSymbolSpec::Ordinal(9),
            ],
            debug: Some(DebugInfoKind::CodeView),
            ..FixtureSpec::default()
        });
        let info = parse_pe(&bytes).expect("fixture parses");
        assert_eq!(info.machine, machine_amd64());
        assert_eq!(
            info.imports,
            vec![
                ImportedLibrary {
                    name: "one.dll".into(),
                    symbols: vec![PeSymbol::Named("CreateThing".into()), PeSymbol::Ordinal(7),],
                },
                ImportedLibrary {
                    name: "empty.dll".into(),
                    symbols: vec![],
                },
            ]
        );
        assert_eq!(
            info.exports,
            vec![
                PeSymbol::Named("ExportA".into()),
                PeSymbol::Named("ExportB".into()),
                PeSymbol::Ordinal(9),
            ]
        );
        assert_eq!(info.debug, Some(DebugInfoKind::CodeView));
    }

    #[test]
    fn absent_empty_and_populated_import_libraries_are_distinct() {
        let info = parse_pe(&fixture(&FixtureSpec {
            imports: &[
                ImportSpec {
                    name: "present.dll",
                    symbols: &[PeSymbolSpec::Named("DoWork")],
                },
                ImportSpec {
                    name: "empty.dll",
                    symbols: &[],
                },
            ],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(
            !info
                .imports
                .iter()
                .any(|library| library.name == "absent.dll")
        );
        let empty = info
            .imports
            .iter()
            .find(|library| library.name == "empty.dll")
            .expect("empty.dll present");
        assert!(empty.symbols.is_empty());
        let present = info
            .imports
            .iter()
            .find(|library| library.name == "present.dll")
            .expect("present.dll present");
        assert_eq!(present.symbols, vec![PeSymbol::Named("DoWork".into())]);
    }

    #[test]
    fn ordinal_only_import_is_recorded() {
        let info = parse_pe(&fixture(&FixtureSpec {
            imports: &[ImportSpec {
                name: "ord.dll",
                symbols: &[PeSymbolSpec::Ordinal(3)],
            }],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert_eq!(
            info.imports,
            vec![ImportedLibrary {
                name: "ord.dll".into(),
                symbols: vec![PeSymbol::Ordinal(3)],
            }]
        );
    }

    #[test]
    fn ordinal_only_export_is_recorded() {
        let info = parse_pe(&fixture(&FixtureSpec {
            dll: true,
            exports: &[PeSymbolSpec::Ordinal(5)],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert_eq!(info.exports, vec![PeSymbol::Ordinal(5)]);
    }

    #[test]
    fn debug_presence_and_kind_vs_absence() {
        let cases = [
            (None, None),
            (Some(DebugInfoKind::Coff), Some(DebugInfoKind::Coff)),
            (Some(DebugInfoKind::CodeView), Some(DebugInfoKind::CodeView)),
            (Some(DebugInfoKind::Repro), Some(DebugInfoKind::Repro)),
            (
                Some(DebugInfoKind::Other(99)),
                Some(DebugInfoKind::Other(99)),
            ),
        ];
        for (debug, expected) in cases {
            let info = parse_pe(&fixture(&FixtureSpec {
                debug,
                ..FixtureSpec::default()
            }))
            .unwrap();
            assert_eq!(info.debug, expected);
        }
    }

    #[test]
    fn executable_fixture_has_empty_exports_and_dll_fixture_keeps_them() {
        let executable = parse_pe(&fixture(&FixtureSpec {
            dll: false,
            exports: &[],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(executable.exports.is_empty());
        let dynamic = parse_pe(&fixture(&FixtureSpec {
            dll: true,
            exports: &[PeSymbolSpec::Named("LibEntry")],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert_eq!(dynamic.exports, vec![PeSymbol::Named("LibEntry".into())]);
    }

    #[test]
    fn fixture_machine_is_the_coff_machine_not_a_hardcoded_value() {
        let info = parse_pe(&fixture(&FixtureSpec {
            machine: machine_arm64(),
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert_eq!(info.machine, machine_arm64());
        assert_ne!(info.machine, machine_amd64());
    }

    #[test]
    fn pe32_truncated_and_foreign_containers_are_refused_by_name() {
        assert!(
            parse_pe(&fixture_pe32())
                .unwrap_err()
                .to_string()
                .contains("pe32")
        );
        let full = fixture(&FixtureSpec::default());
        let truncated = &full[..FIXTURE_COFF_OFF + 10];
        assert!(
            parse_pe(truncated)
                .unwrap_err()
                .to_string()
                .contains("truncated pe")
        );
        assert!(
            parse_pe(b"\x7fELF\x02\x01\x01\x00")
                .unwrap_err()
                .to_string()
                .contains("not a pe file")
        );
        assert!(
            parse_pe(&[])
                .unwrap_err()
                .to_string()
                .contains("not a pe file")
        );
    }

    #[test]
    fn out_of_bounds_directories_are_named_errors() {
        let import = parse_pe(&fixture_oob_import()).unwrap_err().to_string();
        assert!(
            import.contains("out-of-bounds import directory"),
            "{import}"
        );
        let export = parse_pe(&fixture_oob_export()).unwrap_err().to_string();
        assert!(
            export.contains("out-of-bounds export directory"),
            "{export}"
        );
        let debug = parse_pe(&fixture_oob_debug()).unwrap_err().to_string();
        assert!(debug.contains("out-of-bounds debug directory"), "{debug}");
    }
}
