// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Dependency census for the Windows producer, separate from the signed PeInfo schema.
//! Only file-backed PE32+ AMD64 images are admitted. Every import descriptor and
//! export forwarder is bounded by its directory and the original section bytes.
//! Reference: https://learn.microsoft.com/en-us/windows/win32/debug/pe-format
//! Delay descriptor RVA semantics: https://learn.microsoft.com/en-us/cpp/build/reference/understanding-the-helper-function

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeDependencies {
    pub is_dll: bool,
    pub imports: Vec<String>,
    pub delay_imports: Vec<String>,
    pub forwarders: Vec<String>,
}

struct Section {
    virtual_range: Range<u32>,
    raw: Range<usize>,
}

struct Image<'a> {
    bytes: &'a [u8],
    headers: usize,
    sections: Vec<Section>,
    directories: Vec<(u32, u32)>,
    is_dll: bool,
}

fn range(start: usize, len: usize, bound: usize) -> Result<Range<usize>, String> {
    let end = start.checked_add(len).ok_or("PE range overflow")?;
    if end > bound {
        return Err("PE range exceeds file-backed data".into());
    }
    Ok(start..end)
}

fn word(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let r = range(offset, 2, bytes.len())?;
    Ok(u16::from_le_bytes(bytes[r].try_into().unwrap()))
}

fn dword(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let r = range(offset, 4, bytes.len())?;
    Ok(u32::from_le_bytes(bytes[r].try_into().unwrap()))
}

impl<'a> Image<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.get(..2) != Some(b"MZ") {
            return Err("dependency census requires a PE image".into());
        }
        let pe = dword(bytes, 0x3c)? as usize;
        let coff = pe.checked_add(4).ok_or("PE header overflow")?;
        if bytes.get(pe..coff) != Some(b"PE\0\0") {
            return Err("invalid PE signature".into());
        }
        range(coff, 20, bytes.len())?;
        if word(bytes, coff)? != crate::pe::IMAGE_FILE_MACHINE_AMD64 {
            return Err("Windows payload requires AMD64 PE images".into());
        }
        let count = word(bytes, coff + 2)? as usize;
        if count == 0 || count > 96 {
            return Err("unsupported PE section count".into());
        }
        let characteristics = word(bytes, coff + 18)?;
        if characteristics & 2 == 0 {
            return Err("PE image lacks executable characteristic".into());
        }
        let optional = coff + 20;
        let size = word(bytes, coff + 16)? as usize;
        let optional_range = range(optional, size, bytes.len())?;
        if size < 112 || word(bytes, optional)? != 0x20b {
            return Err("dependency census requires PE32+".into());
        }
        let directory_count = dword(bytes, optional + 108)? as usize;
        if directory_count > (size - 112) / 8 {
            return Err("truncated PE data directory table".into());
        }
        let headers = dword(bytes, optional + 60)? as usize;
        let section_table = range(optional_range.end, count * 40, bytes.len())?;
        if headers < section_table.end || headers > bytes.len() {
            return Err("invalid PE header extent".into());
        }
        let mut sections: Vec<Section> = Vec::with_capacity(count);
        for index in 0..count {
            let off = section_table.start + index * 40;
            let virtual_size = dword(bytes, off + 8)?;
            let address = dword(bytes, off + 12)?;
            let raw_size = dword(bytes, off + 16)?;
            let raw_start = dword(bytes, off + 20)? as usize;
            let end = address
                .checked_add(virtual_size.max(raw_size))
                .ok_or("PE section RVA overflow")?;
            let raw = range(raw_start, raw_size as usize, bytes.len())?;
            if address < headers as u32 || (raw_size != 0 && raw_start < headers) {
                return Err("PE section overlaps headers".into());
            }
            for prior in &sections {
                if address < prior.virtual_range.end && prior.virtual_range.start < end {
                    return Err("overlapping PE virtual sections".into());
                }
                if raw_size != 0
                    && !prior.raw.is_empty()
                    && raw.start < prior.raw.end
                    && prior.raw.start < raw.end
                {
                    return Err("overlapping PE raw sections".into());
                }
            }
            sections.push(Section {
                virtual_range: address..end,
                raw,
            });
        }
        let directories = (0..directory_count)
            .map(|i| {
                Ok((
                    dword(bytes, optional + 112 + i * 8)?,
                    dword(bytes, optional + 116 + i * 8)?,
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            bytes,
            headers,
            sections,
            directories,
            is_dll: characteristics & 0x2000 != 0,
        })
    }

    // The returned tail ends at the raw section boundary, never virtual padding
    // or bytes belonging to an adjacent section/certificate overlay.
    fn tail(&self, rva: u32) -> Result<&'a [u8], String> {
        if rva == 0 {
            return Err("null PE data RVA".into());
        }
        if (rva as usize) < self.headers {
            return Ok(&self.bytes[rva as usize..self.headers]);
        }
        for section in &self.sections {
            if section.virtual_range.contains(&rva) {
                let delta = (rva - section.virtual_range.start) as usize;
                if delta >= section.raw.len() {
                    return Err("PE RVA refers to virtual padding".into());
                }
                return Ok(&self.bytes[section.raw.start + delta..section.raw.end]);
            }
        }
        Err("PE RVA has no file-backed section".into())
    }

    fn data(&self, rva: u32, size: usize) -> Result<&'a [u8], String> {
        self.tail(rva)?
            .get(..size)
            .ok_or_else(|| "PE range crosses file-backed section".into())
    }

    fn directory(&self, index: usize) -> Result<Option<(u32, &'a [u8])>, String> {
        let Some(&(rva, size)) = self.directories.get(index) else {
            return Ok(None);
        };
        if rva == 0 && size == 0 {
            return Ok(None);
        }
        if rva == 0 || size == 0 {
            return Err("incomplete PE directory declaration".into());
        }
        rva.checked_add(size).ok_or("PE directory RVA overflow")?;
        Ok(Some((rva, self.data(rva, size as usize)?)))
    }

    fn string(&self, rva: u32) -> Result<&'a str, String> {
        ascii_string(self.tail(rva)?)
    }

    fn imports(&self, delayed: bool) -> Result<Vec<String>, String> {
        let Some((_, data)) = self.directory(if delayed { 13 } else { 1 })? else {
            return Ok(Vec::new());
        };
        let width = if delayed { 32 } else { 20 };
        let mut names = Vec::new();
        for descriptor in data.chunks_exact(width) {
            if descriptor.iter().all(|b| *b == 0) {
                // The directory may also contain strings/thunks after the
                // descriptor list (for example Go PE output). Only the list
                // terminator must fit; the remaining bytes are not padding.
                return Ok(names);
            }
            // Modern AMD64 descriptors use dlattrRva. Refuse legacy VA form
            // and unknown attribute bits rather than accidentally treating VAs as RVAs.
            if delayed && dword(descriptor, 0)? != 1 {
                return Err("unsupported PE delay-import attributes (requires dlattrRva)".into());
            }
            let name_rva = dword(descriptor, if delayed { 4 } else { 12 })?;
            names.push(dll_name(self.string(name_rva)?)?);
            let iat = dword(descriptor, if delayed { 12 } else { 16 })?;
            self.data(iat, 8)?;
            let lookup = dword(descriptor, if delayed { 16 } else { 0 })?;
            self.thunks(if lookup != 0 { lookup } else { iat })?;
        }
        Err("PE import directory lacks a bounded null terminator".into())
    }

    fn thunks(&self, rva: u32) -> Result<(), String> {
        for entry in self.tail(rva)?.chunks_exact(8) {
            let value = u64::from_le_bytes(entry.try_into().unwrap());
            if value == 0 {
                return Ok(());
            }
            if value & (1 << 63) != 0 {
                if value & 0x7fff_ffff_ffff_0000 != 0 {
                    return Err("invalid PE ordinal import".into());
                }
            } else {
                let name =
                    u32::try_from(value).map_err(|_| "PE import name RVA exceeds 32 bits")?;
                self.data(name, 2)?;
                self.string(name.checked_add(2).ok_or("PE name RVA overflow")?)?;
            }
        }
        Err("PE import lookup lacks a bounded null terminator".into())
    }

    fn forwarders(&self) -> Result<Vec<String>, String> {
        let Some((rva, directory)) = self.directory(0)? else {
            return Ok(Vec::new());
        };
        if directory.len() < 40 {
            return Err("truncated PE export directory".into());
        }
        let count = dword(directory, 20)? as usize;
        if count == 0 {
            return Ok(Vec::new());
        }
        let table_size = count.checked_mul(4).ok_or("PE export table overflow")?;
        let table = self.data(dword(directory, 28)?, table_size)?;
        let end = rva
            .checked_add(directory.len() as u32)
            .ok_or("PE export directory overflow")?;
        let mut names = Vec::new();
        for entry in table.chunks_exact(4) {
            let target = u32::from_le_bytes(entry.try_into().unwrap());
            if target >= rva && target < end {
                // Forwarder string including NUL must fit inside the export directory.
                let text = ascii_string(&directory[(target - rva) as usize..])?;
                let (library, symbol) =
                    text.rsplit_once('.').ok_or("invalid PE export forwarder")?;
                if symbol.is_empty()
                    || symbol
                        .bytes()
                        .any(|b| b <= b' ' || matches!(b, b'/' | b'\\' | b':'))
                {
                    return Err("invalid PE forwarder symbol".into());
                }
                if let Some(ordinal) = symbol.strip_prefix('#')
                    && (ordinal.is_empty()
                        || !ordinal.bytes().all(|b| b.is_ascii_digit())
                        || ordinal.parse::<u16>().is_err())
                {
                    return Err("invalid PE forwarder ordinal".into());
                }
                let name = if library.to_ascii_lowercase().ends_with(".dll") {
                    library.to_owned()
                } else {
                    format!("{library}.dll")
                };
                names.push(dll_name(&name)?);
            }
        }
        Ok(names)
    }
}

fn ascii_string(data: &[u8]) -> Result<&str, String> {
    let limit = data.len().min(4096);
    let end = data[..limit]
        .iter()
        .position(|b| *b == 0)
        .ok_or("PE string lacks a bounded null terminator")?;
    if end == 0 || !data[..end].is_ascii() {
        return Err("PE dependency string must be nonempty ASCII".into());
    }
    std::str::from_utf8(&data[..end]).map_err(|_| "invalid PE ASCII string".into())
}

pub(crate) fn dll_name(name: &str) -> Result<String, String> {
    if !name.is_ascii()
        || name.len() > 255
        || !name.to_ascii_lowercase().ends_with(".dll")
        || name.len() <= 4
        || name
            .bytes()
            .any(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')))
        || name.starts_with('.')
        || name.contains("..")
    {
        return Err(format!("invalid PE library basename: {name:?}"));
    }
    Ok(name.to_ascii_lowercase())
}

pub fn inspect_dependencies(bytes: &[u8]) -> Result<PeDependencies, String> {
    let image = Image::parse(bytes)?;
    Ok(PeDependencies {
        is_dll: image.is_dll,
        imports: image.imports(false)?,
        delay_imports: image.imports(true)?,
        forwarders: image.forwarders()?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const PE: usize = 0x80;
    const OPT: usize = PE + 24;
    const SECTION: usize = OPT + 240;
    const RAW: usize = 0x200;
    const RVA: u32 = 0x1000;

    fn put(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn image() -> Vec<u8> {
        let mut bytes = vec![0; 0x600];
        bytes[..2].copy_from_slice(b"MZ");
        put(&mut bytes, 0x3c, PE as u32);
        bytes[PE..PE + 4].copy_from_slice(b"PE\0\0");
        bytes[PE + 4..PE + 6].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[PE + 6..PE + 8].copy_from_slice(&1u16.to_le_bytes());
        bytes[PE + 20..PE + 22].copy_from_slice(&240u16.to_le_bytes());
        bytes[PE + 22..PE + 24].copy_from_slice(&0x2022u16.to_le_bytes());
        bytes[OPT..OPT + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        put(&mut bytes, OPT + 60, RAW as u32);
        put(&mut bytes, OPT + 108, 16);
        put(&mut bytes, SECTION + 8, 0x400);
        put(&mut bytes, SECTION + 12, RVA);
        put(&mut bytes, SECTION + 16, 0x400);
        put(&mut bytes, SECTION + 20, RAW as u32);
        bytes
    }

    fn directory(bytes: &mut [u8], index: usize, offset: u32, size: u32) {
        put(bytes, OPT + 112 + index * 8, RVA + offset);
        put(bytes, OPT + 116 + index * 8, size);
    }

    fn with_import(delayed: bool) -> Vec<u8> {
        let mut bytes = image();
        directory(
            &mut bytes,
            if delayed { 13 } else { 1 },
            0,
            if delayed { 64 } else { 40 },
        );
        if delayed {
            put(&mut bytes, RAW, 1);
        }
        put(&mut bytes, RAW + if delayed { 4 } else { 12 }, RVA + 0x80);
        put(&mut bytes, RAW + if delayed { 12 } else { 16 }, RVA + 0xc0);
        put(&mut bytes, RAW + if delayed { 16 } else { 0 }, RVA + 0xc0);
        bytes[RAW + 0x80..RAW + 0x8d].copy_from_slice(b"KERNEL32.dll\0");
        // Ordinal import and terminating entry exercise the lookup traversal.
        bytes[RAW + 0xc0..RAW + 0xc8].copy_from_slice(&0x8000_0000_0000_0001u64.to_le_bytes());
        bytes
    }

    fn with_forwarder(text: &[u8]) -> Vec<u8> {
        let mut bytes = image();
        directory(&mut bytes, 0, 0, 0x100);
        put(&mut bytes, RAW + 20, 1);
        put(&mut bytes, RAW + 28, RVA + 0x40);
        put(&mut bytes, RAW + 0x40, RVA + 0x80);
        bytes[RAW + 0x80..RAW + 0x80 + text.len()].copy_from_slice(text);
        bytes
    }

    #[test]
    fn ordinary_delay_and_forwarder_dependencies_are_all_reported() {
        for delayed in [false, true] {
            let result = inspect_dependencies(&with_import(delayed)).unwrap();
            assert!(result.is_dll);
            assert_eq!(
                if delayed {
                    result.delay_imports
                } else {
                    result.imports
                },
                ["kernel32.dll"]
            );
        }
        for text in [b"NTDLL.RtlAllocateHeap\0".as_slice(), b"NTDLL.dll.#42\0"] {
            assert_eq!(
                inspect_dependencies(&with_forwarder(text))
                    .unwrap()
                    .forwarders,
                ["ntdll.dll"]
            );
        }
    }

    #[test]
    fn malformed_imports_never_become_an_empty_census() {
        for delayed in [false, true] {
            let original = with_import(delayed);
            let index = if delayed { 13 } else { 1 };
            let mut bytes = original.clone();
            put(
                &mut bytes,
                OPT + 116 + index * 8,
                if delayed { 32 } else { 20 },
            );
            assert!(
                inspect_dependencies(&bytes)
                    .unwrap_err()
                    .contains("terminator")
            );
            let mut bytes = original.clone();
            put(&mut bytes, RAW + if delayed { 4 } else { 12 }, RVA + 0x500);
            put(&mut bytes, SECTION + 8, 0x800);
            assert!(
                inspect_dependencies(&bytes)
                    .unwrap_err()
                    .contains("virtual padding")
            );
            let mut bytes = original.clone();
            // Valid referenced data can follow the descriptor terminator.
            put(&mut bytes, OPT + 116 + index * 8, 0x100);
            assert!(inspect_dependencies(&bytes).is_ok());
            let mut bytes = original.clone();
            put(&mut bytes, OPT + 116 + index * 8, 0);
            assert!(
                inspect_dependencies(&bytes)
                    .unwrap_err()
                    .contains("incomplete")
            );
            let mut bytes = original;
            put(&mut bytes, RAW + if delayed { 4 } else { 12 }, RVA + 0x3ff);
            bytes[RAW + 0x3ff] = b'A';
            bytes.extend_from_slice(b".dll\0");
            assert!(
                inspect_dependencies(&bytes)
                    .unwrap_err()
                    .contains("terminator")
            );
        }
    }

    #[test]
    fn malformed_header_and_delay_attributes_are_refused() {
        let mut bytes = with_import(true);
        for attr in [0, 2, 3, u32::MAX] {
            put(&mut bytes, RAW, attr);
            assert!(
                inspect_dependencies(&bytes)
                    .unwrap_err()
                    .contains("attributes")
            );
        }
        let mut bytes = image();
        put(&mut bytes, OPT + 108, 17);
        assert!(
            inspect_dependencies(&bytes)
                .unwrap_err()
                .contains("directory table")
        );
        let mut bytes = image();
        put(&mut bytes, SECTION + 16, 0x800);
        assert!(inspect_dependencies(&bytes).is_err());
        let mut bytes = image();
        bytes[PE + 6..PE + 8].copy_from_slice(&2u16.to_le_bytes());
        let first = bytes[SECTION..SECTION + 40].to_vec();
        bytes[SECTION + 40..SECTION + 80].copy_from_slice(&first);
        assert!(
            inspect_dependencies(&bytes)
                .unwrap_err()
                .contains("overlapping")
        );
    }

    #[test]
    fn forwarder_text_must_terminate_inside_the_export_directory() {
        for text in [
            b"missingseparator\0".as_slice(),
            b"../evil.Func\0",
            b"NTDLL.#65536\0",
            b"NTDLL.\0",
            b"NTDLL.#x\0",
        ] {
            assert!(
                inspect_dependencies(&with_forwarder(text)).is_err(),
                "{text:?}"
            );
        }
        let mut bytes = with_forwarder(b"NTDLL.Function\0");
        put(&mut bytes, OPT + 116, 0x80 + 14); // Excludes the NUL itself.
        assert!(
            inspect_dependencies(&bytes)
                .unwrap_err()
                .contains("terminator")
        );
    }

    #[test]
    fn import_thunk_names_ordinals_and_terminators_are_bounded() {
        let mut bytes = with_import(false);
        bytes[RAW + 0xc0..RAW + 0xc8].copy_from_slice(&0x8000_0001_0000_0001u64.to_le_bytes());
        assert!(
            inspect_dependencies(&bytes)
                .unwrap_err()
                .contains("ordinal")
        );
        let mut bytes = with_import(false);
        put(&mut bytes, RAW, RVA + 0x3f8);
        bytes[RAW + 0x3f8..RAW + 0x400].copy_from_slice(&0x8000_0000_0000_0001u64.to_le_bytes());
        assert!(inspect_dependencies(&bytes).unwrap_err().contains("lookup"));
        let mut bytes = with_import(false);
        bytes[RAW + 0xc0..RAW + 0xc8].copy_from_slice(&(u64::from(RVA) + 0x3ff).to_le_bytes());
        assert!(inspect_dependencies(&bytes).is_err());
    }
}
