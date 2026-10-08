// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

const ELF64_EHDR: usize = 64;
const ELF64_PHDR: usize = 56;
const ELF64_SHDR: usize = 64;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;

const DT_NULL: i64 = 0;
const DT_NEEDED: i64 = 1;
const DT_STRTAB: i64 = 5;
const DT_SONAME: i64 = 14;
const DT_RPATH: i64 = 15;
const DT_RUNPATH: i64 = 29;

const DT_VERDEF: i64 = 0x6fff_fffc;
const DT_VERDEFNUM: i64 = 0x6fff_fffd;
const DT_VERNEED: i64 = 0x6fff_fffe;
const DT_VERNEEDNUM: i64 = 0x6fff_ffff;

const DT_AUDIT: i64 = 0x6fff_fefc;
const DT_DEPAUDIT: i64 = 0x6fff_fefb;
const DT_FILTER: i64 = 0x7fff_ffff;
const DT_AUXILIARY: i64 = 0x7fff_fffd;

const SHT_STRTAB: u32 = 3;
const SHT_DYNAMIC: u32 = 6;
const SHT_GNU_VERNEED: u32 = 0x6fff_fffe;

pub const HELPER_RUNPATH: &str = "$ORIGIN/../lib/solstone-core-speakers-analyze";
pub const HELPER_SONAME: &str = "libonnxruntime.so.1";
pub const GLIBC_CEILING: (u32, u32) = (2, 34);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionNeed {
    pub file: String,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionDef {
    pub file: String,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfInfo {
    pub machine: u16,
    pub interp: Option<String>,
    pub soname: Option<String>,
    pub needed: Vec<String>,
    pub runpath: Option<String>,
    pub rpath: Option<String>,
    pub verneed: Vec<VersionNeed>,
    pub verdef: Vec<VersionDef>,
}

#[derive(Debug)]
pub struct ElfError {
    pub message: String,
}

impl ElfError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ElfError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ElfError {}

pub fn parse_elf(bytes: &[u8]) -> Result<ElfInfo, ElfError> {
    if bytes.len() < ELF64_EHDR || &bytes[0..4] != b"\x7fELF" {
        return Err(ElfError::new("not an ELF file"));
    }
    if bytes[4] != 2 || bytes[5] != 1 {
        return Err(ElfError::new("only ELF64 little-endian is supported"));
    }
    let machine = read_u16(bytes, 18)?;
    let phoff = read_u64(bytes, 32)? as usize;
    let shoff = read_u64(bytes, 40)? as usize;
    let phentsize = read_u16(bytes, 54)? as usize;
    let phnum = read_u16(bytes, 56)? as usize;
    let shentsize = read_u16(bytes, 58)? as usize;
    let shnum = read_u16(bytes, 60)? as usize;
    let mut interp = None;
    let mut dynamic = None;
    for index in 0..phnum {
        let off = phoff + index * phentsize;
        let p_type = read_u32(bytes, off)?;
        let p_offset = read_u64(bytes, off + 8)? as usize;
        let p_filesz = read_u64(bytes, off + 32)? as usize;
        if p_type == PT_INTERP {
            let end = p_offset
                .checked_add(p_filesz)
                .ok_or_else(|| ElfError::new("overflow PT_INTERP"))?;
            if end > bytes.len() {
                return Err(ElfError::new("truncated PT_INTERP"));
            }
            let raw = &bytes[p_offset..end];
            let cstr = raw.split(|byte| *byte == 0).next().unwrap_or(raw);
            interp = Some(
                std::str::from_utf8(cstr)
                    .map_err(|_| ElfError::new("PT_INTERP is not UTF-8"))?
                    .to_owned(),
            );
        }
        if p_type == PT_DYNAMIC {
            dynamic = Some((p_offset, p_filesz));
        }
    }

    let mut needed = Vec::new();
    let mut runpath = None;
    let mut rpath = None;
    let mut dynstr_vaddr = None;
    let mut soname_offset = None;
    let mut verneed_vaddr = None;
    let mut verneed_num = None;
    let mut verdef_vaddr = None;
    let mut verdef_num = None;

    if let Some((offset, size)) = dynamic {
        let mut cursor = offset;
        let end = offset + size;
        while cursor + 16 <= end {
            let tag = read_i64(bytes, cursor)?;
            let value = read_u64(bytes, cursor + 8)?;
            match tag {
                DT_NULL => break,
                DT_NEEDED => needed.push(value),
                DT_STRTAB => dynstr_vaddr = Some(value),
                DT_SONAME => soname_offset = Some(value),
                DT_RUNPATH => runpath = Some(value),
                DT_RPATH => rpath = Some(value),
                DT_VERNEED => verneed_vaddr = Some(value),
                DT_VERNEEDNUM => verneed_num = Some(value as usize),
                DT_VERDEF => verdef_vaddr = Some(value),
                DT_VERDEFNUM => verdef_num = Some(value as usize),
                DT_AUDIT => return Err(ElfError::new("forbidden dynamic tag DT_AUDIT")),
                DT_DEPAUDIT => return Err(ElfError::new("forbidden dynamic tag DT_DEPAUDIT")),
                DT_FILTER => return Err(ElfError::new("forbidden dynamic tag DT_FILTER")),
                DT_AUXILIARY => return Err(ElfError::new("forbidden dynamic tag DT_AUXILIARY")),
                _ => {}
            }
            cursor += 16;
        }
    }

    let dynstr = dynstr_from_vaddr(bytes, phoff, phentsize, phnum, dynstr_vaddr)
        .or_else(|_| dynstr_section(bytes, shoff, shentsize, shnum))?;

    let needed = needed
        .into_iter()
        .map(|offset| dynstr_string(&dynstr, offset as usize))
        .collect::<Result<Vec<_>, _>>()?;
    let runpath = runpath
        .map(|offset| dynstr_string(&dynstr, offset as usize))
        .transpose()?;
    let rpath = rpath
        .map(|offset| dynstr_string(&dynstr, offset as usize))
        .transpose()?;
    let soname = soname_offset
        .map(|offset| dynstr_string(&dynstr, offset as usize))
        .transpose()?;

    let verneed = parse_verneed(
        bytes,
        phoff,
        phentsize,
        phnum,
        verneed_vaddr,
        verneed_num,
        &dynstr,
    )?;

    let verdef = parse_verdef(
        bytes,
        phoff,
        phentsize,
        phnum,
        verdef_vaddr,
        verdef_num,
        &dynstr,
    )?;

    Ok(ElfInfo {
        machine,
        interp,
        soname,
        needed,
        runpath,
        rpath,
        verneed,
        verdef,
    })
}

fn dynstr_section(
    bytes: &[u8],
    shoff: usize,
    shentsize: usize,
    shnum: usize,
) -> Result<Vec<u8>, ElfError> {
    for index in 0..shnum {
        let off = shoff + index * shentsize;
        let sh_type = read_u32(bytes, off + 4)?;
        if sh_type != SHT_STRTAB {
            continue;
        }
        let offset = read_u64(bytes, off + 24)? as usize;
        let size = read_u64(bytes, off + 32)? as usize;
        if offset + size <= bytes.len() && looks_like_dynstr(&bytes[offset..offset + size]) {
            return Ok(bytes[offset..offset + size].to_vec());
        }
    }
    Err(ElfError::new("missing .dynstr"))
}

fn looks_like_dynstr(bytes: &[u8]) -> bool {
    bytes.first() == Some(&0) && bytes.len() > 1
}

fn translate_vaddr(
    bytes: &[u8],
    phoff: usize,
    phentsize: usize,
    phnum: usize,
    vaddr: u64,
) -> Result<usize, ElfError> {
    for index in 0..phnum {
        let off = phoff + index * phentsize;
        if read_u32(bytes, off)? != PT_LOAD {
            continue;
        }
        let p_offset = read_u64(bytes, off + 8)?;
        let p_vaddr = read_u64(bytes, off + 16)?;
        let p_filesz = read_u64(bytes, off + 32)?;
        if vaddr >= p_vaddr && vaddr < p_vaddr + p_filesz {
            return Ok((p_offset + (vaddr - p_vaddr)) as usize);
        }
    }
    Err(ElfError::new("virtual address is not in a PT_LOAD segment"))
}

fn dynstr_from_vaddr(
    bytes: &[u8],
    phoff: usize,
    phentsize: usize,
    phnum: usize,
    vaddr: Option<u64>,
) -> Result<Vec<u8>, ElfError> {
    let Some(vaddr) = vaddr else {
        return Err(ElfError::new("missing DT_STRTAB"));
    };
    let file = translate_vaddr(bytes, phoff, phentsize, phnum, vaddr)?;
    Ok(bytes[file..].to_vec())
}

fn dynstr_string(dynstr: &[u8], offset: usize) -> Result<String, ElfError> {
    let rest = dynstr
        .get(offset..)
        .ok_or_else(|| ElfError::new("dynstr offset out of range"))?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| ElfError::new("unterminated dynstr"))?;
    std::str::from_utf8(&rest[..end])
        .map(str::to_owned)
        .map_err(|_| ElfError::new("dynstr is not UTF-8"))
}

fn parse_verneed(
    bytes: &[u8],
    phoff: usize,
    phentsize: usize,
    phnum: usize,
    vaddr: Option<u64>,
    num: Option<usize>,
    dynstr: &[u8],
) -> Result<Vec<VersionNeed>, ElfError> {
    let Some(vaddr) = vaddr else {
        return Ok(Vec::new());
    };
    let start_offset = translate_vaddr(bytes, phoff, phentsize, phnum, vaddr)?;
    let mut cursor = start_offset;
    let mut needs = Vec::new();
    let max_count = num.unwrap_or(usize::MAX);

    for _ in 0..max_count {
        if cursor + 16 > bytes.len() {
            return Err(ElfError::new("truncated Elf64_Verneed"));
        }
        let vn_file = read_u32(bytes, cursor + 4)? as usize;
        let vn_cnt = read_u16(bytes, cursor + 2)? as usize;
        let vn_aux = read_u32(bytes, cursor + 8)? as usize;
        let vn_next = read_u32(bytes, cursor + 12)? as usize;

        let file_name = dynstr_string(dynstr, vn_file)?;
        let mut aux_cursor = cursor + vn_aux;
        let mut names = Vec::new();

        for _ in 0..vn_cnt {
            if aux_cursor + 16 > bytes.len() {
                return Err(ElfError::new("truncated Elf64_Vernaux"));
            }
            let vna_name = read_u32(bytes, aux_cursor + 8)? as usize;
            let vna_next = read_u32(bytes, aux_cursor + 12)? as usize;
            names.push(dynstr_string(dynstr, vna_name)?);
            if vna_next == 0 {
                break;
            }
            aux_cursor += vna_next;
        }

        needs.push(VersionNeed {
            file: file_name,
            names,
        });

        if vn_next == 0 {
            break;
        }
        cursor += vn_next;
    }
    Ok(needs)
}

fn parse_verdef(
    bytes: &[u8],
    phoff: usize,
    phentsize: usize,
    phnum: usize,
    vaddr: Option<u64>,
    num: Option<usize>,
    dynstr: &[u8],
) -> Result<Vec<VersionDef>, ElfError> {
    let Some(vaddr) = vaddr else {
        return Ok(Vec::new());
    };
    let start_offset = translate_vaddr(bytes, phoff, phentsize, phnum, vaddr)?;
    let mut cursor = start_offset;
    let mut defs = Vec::new();
    let max_count = num.unwrap_or(usize::MAX);

    for _ in 0..max_count {
        if cursor + 20 > bytes.len() {
            return Err(ElfError::new("truncated Elf64_Verdef"));
        }
        let vd_flags = read_u16(bytes, cursor + 2)?;
        let vd_cnt = read_u16(bytes, cursor + 6)? as usize;
        let vd_aux = read_u32(bytes, cursor + 12)? as usize;
        let vd_next = read_u32(bytes, cursor + 16)? as usize;

        if (vd_flags & 1) == 0 {
            let mut aux_cursor = cursor + vd_aux;
            let mut names = Vec::new();
            for _ in 0..vd_cnt {
                if aux_cursor + 8 > bytes.len() {
                    return Err(ElfError::new("truncated Elf64_Verdaux"));
                }
                let vda_name = read_u32(bytes, aux_cursor)? as usize;
                let vda_next = read_u32(bytes, aux_cursor + 4)? as usize;
                names.push(dynstr_string(dynstr, vda_name)?);
                if vda_next == 0 {
                    break;
                }
                aux_cursor += vda_next;
            }
            if let Some(first) = names.first() {
                defs.push(VersionDef {
                    file: first.clone(),
                    names,
                });
            }
        }

        if vd_next == 0 {
            break;
        }
        cursor += vd_next;
    }
    Ok(defs)
}

pub fn inspect_musl_static(info: &ElfInfo, machine: u16) -> Result<(), ElfError> {
    let mut missing = Vec::new();
    if info.machine != machine {
        missing.push(format!("e_machine {}", info.machine));
    }
    if info.interp.is_some() {
        missing.push("PT_INTERP present".to_owned());
    }
    if !missing.is_empty() {
        return Err(ElfError::new(format!(
            "missing required:\n  {}",
            missing.join("\n  ")
        )));
    }
    Ok(())
}

pub fn inspect_gnu_helper(
    info: &ElfInfo,
    machine: u16,
    runpath: Option<&str>,
    needed: &[&str],
) -> Result<(), ElfError> {
    let mut unexpected = Vec::new();
    if info.machine != machine {
        unexpected.push(format!("e_machine {}", info.machine));
    }
    match &info.interp {
        Some(interp) if interp.contains("ld-linux") => {}
        other => unexpected.push(format!("PT_INTERP {other:?}")),
    }
    if let Some(expected) = runpath
        && info.runpath.as_deref() != Some(expected)
        && info.rpath.as_deref() != Some(expected)
    {
        unexpected.push(format!("DT_RUNPATH {:?}", info.runpath));
    }
    for name in needed {
        if !info.needed.iter().any(|item| item == name) {
            unexpected.push(format!("DT_NEEDED {name}"));
        }
    }
    if !unexpected.is_empty() {
        unexpected.sort();
        return Err(ElfError::new(format!(
            "unexpected:\n  {}",
            unexpected.join("\n  ")
        )));
    }
    Ok(())
}

pub fn inspect_core_family(info: &ElfInfo, machine: u16) -> Result<(), ElfError> {
    if info.interp.is_some() || !info.needed.is_empty() {
        return Err(ElfError::new(
            "unexpected:\n  dynamic core-family".to_owned(),
        ));
    }
    inspect_musl_static(info, machine)
}

fn parse_numeric_version(name: &str, prefix: &str) -> Option<Vec<u32>> {
    let rest = name.strip_prefix(prefix)?;
    let mut components = Vec::new();
    for part in rest.split('.') {
        components.push(part.parse().ok()?);
    }
    if components.is_empty() {
        None
    } else {
        Some(components)
    }
}

fn glibc_version_exceeds_ceiling(components: &[u32], ceiling: (u32, u32)) -> bool {
    let max_len = components.len().max(2);
    for i in 0..max_len {
        let comp = components.get(i).copied().unwrap_or(0);
        let ceil = match i {
            0 => ceiling.0,
            1 => ceiling.1,
            _ => 0,
        };
        if comp > ceil {
            return true;
        }
        if comp < ceil {
            return false;
        }
    }
    false
}

pub const NON_GLIBC_SYSTEM_NEEDED: &[(&str, &str, &str)] = &[
    (
        "libstdc++.so.6",
        "libstdc++6 (>= 11)",
        "libstdc++.so.6(GLIBCXX_3.4.29)(64bit)",
    ),
    (
        "libgcc_s.so.1",
        "libgcc-s1 (>= 11)",
        "libgcc_s.so.1()(64bit)",
    ),
    ("libgomp.so.1", "libgomp1 (>= 11)", "libgomp"),
];

pub fn admit_elf(
    staged_path: &str,
    bytes: &[u8],
    expected_machine: u16,
    pkg_rel_dir: &str,
    nested_archive: Option<(&str, &str)>,
    staged_files: &[(&str, &[u8])],
) -> Result<(), ElfError> {
    let info = parse_elf(bytes)?;
    let target_prefix = match nested_archive {
        Some((archive, member)) => format!("archive {archive} member {member}"),
        None => staged_path.to_string(),
    };

    if info.machine != expected_machine {
        return Err(ElfError::new(format!(
            "{target_prefix}: unexpected machine {} (expected {expected_machine})",
            info.machine
        )));
    }

    let expected_interp = match expected_machine {
        EM_X86_64 => "/lib64/ld-linux-x86-64.so.2",
        EM_AARCH64 => "/lib/ld-linux-aarch64.so.1",
        _ => {
            return Err(ElfError::new(format!(
                "{target_prefix}: unsupported machine"
            )));
        }
    };

    if let Some(interp) = &info.interp
        && interp != expected_interp
    {
        return Err(ElfError::new(format!(
            "{target_prefix}: unexpected PT_INTERP {interp:?} (expected {expected_interp})"
        )));
    }

    let is_x86 = expected_machine == EM_X86_64;
    let gcc_tables: &[(&str, &[&str])] = if is_x86 {
        &[
            ("libstdc++.so.6", crate::elf_gcc11::X86_64_LIBSTDCXX),
            ("libgcc_s.so.1", crate::elf_gcc11::X86_64_LIBGCC),
            ("libgomp.so.1", crate::elf_gcc11::X86_64_LIBGOMP),
        ]
    } else {
        &[
            ("libstdc++.so.6", crate::elf_gcc11::AARCH64_LIBSTDCXX),
            ("libgcc_s.so.1", crate::elf_gcc11::AARCH64_LIBGCC),
            ("libgomp.so.1", crate::elf_gcc11::AARCH64_LIBGOMP),
        ]
    };

    let loader_so = if is_x86 {
        "ld-linux-x86-64.so.2"
    } else {
        "ld-linux-aarch64.so.1"
    };

    let glibc_family = [
        "libc.so.6",
        "libm.so.6",
        "libpthread.so.0",
        "libdl.so.2",
        "librt.so.1",
        loader_so,
    ];

    let mut system_needed = glibc_family.to_vec();
    for (name, _, _) in NON_GLIBC_SYSTEM_NEEDED {
        system_needed.push(name);
    }

    for need in &info.verneed {
        let file = need.file.as_str();
        if glibc_family.contains(&file) {
            for ver in &need.names {
                if ver.starts_with("GLIBC_PRIVATE") || ver.starts_with("GLIBC_ABI_") {
                    return Err(ElfError::new(format!(
                        "{target_prefix}: forbidden glibc version {ver} in {file}"
                    )));
                }
                let Some(components) = parse_numeric_version(ver, "GLIBC_") else {
                    return Err(ElfError::new(format!(
                        "{target_prefix}: unrecognized glibc version format {ver} in {file}"
                    )));
                };
                if glibc_version_exceeds_ceiling(&components, GLIBC_CEILING) {
                    return Err(ElfError::new(format!(
                        "{target_prefix}: glibc version {ver} exceeds ceiling {}.{} in {file}",
                        GLIBC_CEILING.0, GLIBC_CEILING.1
                    )));
                }
            }
        } else if let Some((_, allowed)) = gcc_tables.iter().find(|(name, _)| *name == file) {
            for ver in &need.names {
                if !allowed.contains(&ver.as_str()) {
                    return Err(ElfError::new(format!(
                        "{target_prefix}: unadmitted symbol version {ver} in {file}"
                    )));
                }
            }
        }
    }

    if let Some((archive, member)) = nested_archive {
        if let Some(rp) = &info.runpath {
            return Err(ElfError::new(format!(
                "archive {archive} member {member}: nested archive binary must not declare RUNPATH ({rp})"
            )));
        }
        if let Some(rp) = &info.rpath {
            return Err(ElfError::new(format!(
                "archive {archive} member {member}: nested archive binary must not declare RPATH ({rp})"
            )));
        }
        for need in &info.needed {
            if !system_needed.contains(&need.as_str()) {
                return Err(ElfError::new(format!(
                    "archive {archive} member {member}: non-system dependency {need}"
                )));
            }
        }
        return Ok(());
    }

    let mut search_paths = Vec::new();
    let mut raw_paths = Vec::new();
    if let Some(rp) = &info.runpath {
        raw_paths.extend(rp.split(':'));
    }
    if let Some(rp) = &info.rpath {
        raw_paths.extend(rp.split(':'));
    }

    for elem in raw_paths {
        if elem.is_empty() {
            return Err(ElfError::new(format!(
                "{staged_path}: empty search path element in RUNPATH/RPATH"
            )));
        }
        if elem.starts_with('/') {
            return Err(ElfError::new(format!(
                "{staged_path}: absolute search path element {elem} in RUNPATH/RPATH"
            )));
        }
        if elem.contains("${ORIGIN}") || elem.contains("$LIB") || elem.contains("$PLATFORM") {
            return Err(ElfError::new(format!(
                "{staged_path}: forbidden dynamic string token in {elem}"
            )));
        }
        if !elem.starts_with("$ORIGIN") {
            return Err(ElfError::new(format!(
                "{staged_path}: search path element must start with $ORIGIN: {elem}"
            )));
        }

        let subpath = elem.strip_prefix("$ORIGIN").unwrap();
        let subpath = subpath.strip_prefix('/').unwrap_or(subpath);

        let mut current_components = if pkg_rel_dir.is_empty() || pkg_rel_dir == "." {
            Vec::new()
        } else {
            pkg_rel_dir.split('/').map(String::from).collect::<Vec<_>>()
        };

        if !subpath.is_empty() {
            for part in subpath.split('/') {
                if part.is_empty() || part == "." {
                    continue;
                }
                if part == ".." {
                    if current_components.is_empty() {
                        return Err(ElfError::new(format!(
                            "{staged_path}: search path element {elem} escapes package root"
                        )));
                    }
                    if let Some(last) = current_components.last()
                        && (last.starts_with("solstone-") || *last == "solstone_journal_models")
                        && current_components.len() == 2
                        && current_components[0] == "lib"
                    {
                        return Err(ElfError::new(format!(
                            "{staged_path}: search path element {elem} escapes private namespace"
                        )));
                    }
                    current_components.pop();
                } else {
                    current_components.push(part.to_owned());
                }
            }
        }

        let resolved_dir = current_components.join("/");
        if staged_path.starts_with("bin/") {
            let is_private = resolved_dir.starts_with("lib/solstone-")
                || resolved_dir.starts_with("lib/solstone_journal_models");
            if !is_private {
                return Err(ElfError::new(format!(
                    "{staged_path}: bin binary search path {elem} must point at private lib directory"
                )));
            }
            let has_regular = staged_files.iter().any(|(f, _)| {
                if let Some(rest) = f.strip_prefix(&resolved_dir) {
                    rest.starts_with('/') && !rest[1..].contains('/')
                } else {
                    false
                }
            });
            if !has_regular {
                return Err(ElfError::new(format!(
                    "{staged_path}: search directory {resolved_dir} contains no staged regular files"
                )));
            }
        }
        search_paths.push(resolved_dir);
    }

    let staged_file_map: BTreeMap<&str, &[u8]> = staged_files.iter().copied().collect();

    for need in &info.needed {
        if need.contains('/') {
            return Err(ElfError::new(format!(
                "{staged_path}: NEEDED entry {need} contains slash"
            )));
        }

        let is_sys = system_needed.contains(&need.as_str());
        if is_sys {
            for dir in &search_paths {
                let candidate = if dir.is_empty() {
                    need.clone()
                } else {
                    format!("{dir}/{need}")
                };
                if staged_file_map.contains_key(candidate.as_str()) {
                    return Err(ElfError::new(format!(
                        "{staged_path}: search path shadows system library {need} at {candidate}"
                    )));
                }
            }
            continue;
        }

        let mut hit: Option<(&str, &[u8])> = None;
        for dir in &search_paths {
            let candidate = if dir.is_empty() {
                need.clone()
            } else {
                format!("{dir}/{need}")
            };
            if let Some((path, dep_bytes)) =
                staged_files.iter().find(|(p, _)| *p == candidate.as_str())
            {
                hit = Some((*path, *dep_bytes));
                break;
            }
        }

        let Some((hit_path, dep_bytes)) = hit else {
            return Err(ElfError::new(format!(
                "{staged_path}: unsatisfied NEEDED dependency {need}"
            )));
        };

        let dep_info = parse_elf(dep_bytes).map_err(|e| {
            ElfError::new(format!(
                "{staged_path}: dependency {need} at {hit_path} could not be parsed: {e}"
            ))
        })?;

        if dep_info.soname.as_deref() != Some(need.as_str()) {
            return Err(ElfError::new(format!(
                "{staged_path}: dependency {need} at {hit_path} has SONAME {:?}, expected {need}",
                dep_info.soname
            )));
        }

        for ver_need in &info.verneed {
            if ver_need.file == *need {
                for ver in &ver_need.names {
                    let found = dep_info
                        .verdef
                        .iter()
                        .any(|vd| vd.names.iter().any(|n| n == ver));
                    if !found {
                        return Err(ElfError::new(format!(
                            "{staged_path}: dependency {need} at {hit_path} missing symbol version {ver}"
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

pub fn committed_gnu_dynamic() -> &'static [u8] {
    include_bytes!("../fixtures/gnu-dynamic.elf")
}

pub fn committed_static_musl() -> &'static [u8] {
    include_bytes!("../fixtures/static-musl.elf")
}

pub const fn machine_x86_64() -> u16 {
    EM_X86_64
}

pub const fn machine_aarch64() -> u16 {
    EM_AARCH64
}

pub fn fixture_gnu_dynamic(
    machine: u16,
    interp: &str,
    needed: &[&str],
    runpath: Option<&str>,
    glibc: (u32, u32),
) -> Vec<u8> {
    build_gnu(machine, Some(interp), needed, runpath, Some(glibc))
}

pub fn fixture_static_musl(machine: u16) -> Vec<u8> {
    build_gnu(machine, None, &[], None, None)
}

fn build_gnu(
    machine: u16,
    interp: Option<&str>,
    needed: &[&str],
    runpath: Option<&str>,
    glibc: Option<(u32, u32)>,
) -> Vec<u8> {
    let mut dynstr = vec![0_u8];
    let mut needed_off = Vec::new();
    for name in needed {
        needed_off.push(dynstr.len() as u64);
        dynstr.extend_from_slice(name.as_bytes());
        dynstr.push(0);
    }
    let libc_off = dynstr.len() as u32;
    dynstr.extend_from_slice(b"libc.so.6\0");
    let glibc_off = dynstr.len() as u32;
    let glibc_name = glibc.map(|(major, minor)| format!("GLIBC_{major}.{minor}"));
    if let Some(name) = &glibc_name {
        dynstr.extend_from_slice(name.as_bytes());
        dynstr.push(0);
    }
    let runpath_off = runpath.map(|value| {
        let off = dynstr.len() as u64;
        dynstr.extend_from_slice(value.as_bytes());
        dynstr.push(0);
        off
    });

    let mut verneed = Vec::new();
    if glibc.is_some() {
        verneed.extend_from_slice(&1_u16.to_le_bytes());
        verneed.extend_from_slice(&1_u16.to_le_bytes());
        verneed.extend_from_slice(&libc_off.to_le_bytes());
        verneed.extend_from_slice(&16_u32.to_le_bytes());
        verneed.extend_from_slice(&0_u32.to_le_bytes());
        let hash = elf_hash(glibc_name.as_deref().unwrap_or(""));
        verneed.extend_from_slice(&hash.to_le_bytes());
        verneed.extend_from_slice(&0_u16.to_le_bytes());
        verneed.extend_from_slice(&2_u16.to_le_bytes());
        verneed.extend_from_slice(&glibc_off.to_le_bytes());
        verneed.extend_from_slice(&0_u32.to_le_bytes());
    }

    let interp_bytes = interp.map(|value| {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        bytes
    });
    let shstrtab = b"\0.interp\0.dynstr\0.dynamic\0.gnu.version_r\0.shstrtab\0";

    let phnum = 2 + u16::from(interp.is_some());
    let shnum = 5 + u16::from(glibc.is_some()) + u16::from(interp.is_some());
    let phoff = ELF64_EHDR;
    let after_ph = phoff + ELF64_PHDR * phnum as usize;
    let interp_off = after_ph;
    let interp_len = interp_bytes.as_ref().map(Vec::len).unwrap_or(0);
    let dyn_off = align8(interp_off + interp_len);

    let num_dyn_entries = 2
        + needed_off.len()
        + usize::from(runpath_off.is_some())
        + if glibc.is_some() { 2 } else { 0 };
    let dyn_size = num_dyn_entries * 16;
    let dynstr_off = dyn_off + dyn_size;
    let ver_off = align4(dynstr_off + dynstr.len());
    let shstr_off = ver_off + verneed.len();
    let shoff = align8(shstr_off + shstrtab.len());
    let total = shoff + ELF64_SHDR * shnum as usize;

    let mut dynamic = Vec::new();
    push_dyn(&mut dynamic, DT_STRTAB, dynstr_off as u64);
    for off in &needed_off {
        push_dyn(&mut dynamic, DT_NEEDED, *off);
    }
    if let Some(off) = runpath_off {
        push_dyn(&mut dynamic, DT_RUNPATH, off);
    }
    if glibc.is_some() {
        push_dyn(&mut dynamic, DT_VERNEED, ver_off as u64);
        push_dyn(&mut dynamic, DT_VERNEEDNUM, 1);
    }
    push_dyn(&mut dynamic, DT_NULL, 0);

    let mut bytes = vec![0_u8; total];

    bytes[0..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    write_u16(&mut bytes, 16, ET_DYN);
    write_u16(&mut bytes, 18, machine);
    write_u32(&mut bytes, 20, 1);
    write_u64(&mut bytes, 32, phoff as u64);
    write_u64(&mut bytes, 40, shoff as u64);
    write_u16(&mut bytes, 52, ELF64_EHDR as u16);
    write_u16(&mut bytes, 54, ELF64_PHDR as u16);
    write_u16(&mut bytes, 56, phnum);
    write_u16(&mut bytes, 58, ELF64_SHDR as u16);
    write_u16(&mut bytes, 60, shnum);
    write_u16(&mut bytes, 62, shnum - 1);

    let mut ph = 0;
    write_phdr(
        &mut bytes,
        phoff,
        ph,
        PhdrSpec {
            p_type: PT_LOAD,
            flags: 5,
            offset: 0,
            filesz: total as u64,
            memsz: total as u64,
        },
    );
    ph += 1;
    if let Some(interp_bytes) = &interp_bytes {
        write_phdr(
            &mut bytes,
            phoff,
            ph,
            PhdrSpec {
                p_type: PT_INTERP,
                flags: 4,
                offset: interp_off as u64,
                filesz: interp_bytes.len() as u64,
                memsz: interp_bytes.len() as u64,
            },
        );
        bytes[interp_off..interp_off + interp_bytes.len()].copy_from_slice(interp_bytes);
        ph += 1;
    }
    write_phdr(
        &mut bytes,
        phoff,
        ph,
        PhdrSpec {
            p_type: PT_DYNAMIC,
            flags: 6,
            offset: dyn_off as u64,
            filesz: dynamic.len() as u64,
            memsz: dynamic.len() as u64,
        },
    );

    bytes[dyn_off..dyn_off + dynamic.len()].copy_from_slice(&dynamic);
    bytes[dynstr_off..dynstr_off + dynstr.len()].copy_from_slice(&dynstr);
    if !verneed.is_empty() {
        bytes[ver_off..ver_off + verneed.len()].copy_from_slice(&verneed);
    }
    bytes[shstr_off..shstr_off + shstrtab.len()].copy_from_slice(shstrtab);

    let mut shndx = 0;
    write_shdr(&mut bytes, shoff, shndx, ShdrSpec::default());
    shndx += 1;
    let mut name = 1;
    if interp.is_some() {
        write_shdr(
            &mut bytes,
            shoff,
            shndx,
            ShdrSpec {
                name,
                sh_type: 1,
                offset: interp_off as u64,
                size: interp_len as u64,
                entsize: 1,
                ..ShdrSpec::default()
            },
        );
        shndx += 1;
        name += ".interp".len() as u32 + 1;
    }
    let dynstr_ndx = shndx;
    write_shdr(
        &mut bytes,
        shoff,
        shndx,
        ShdrSpec {
            name,
            sh_type: SHT_STRTAB,
            offset: dynstr_off as u64,
            size: dynstr.len() as u64,
            entsize: 1,
            ..ShdrSpec::default()
        },
    );
    shndx += 1;
    name += ".dynstr".len() as u32 + 1;
    write_shdr(
        &mut bytes,
        shoff,
        shndx,
        ShdrSpec {
            name,
            sh_type: SHT_DYNAMIC,
            offset: dyn_off as u64,
            size: dynamic.len() as u64,
            link: dynstr_ndx as u32,
            entsize: 16,
            ..ShdrSpec::default()
        },
    );
    shndx += 1;
    name += ".dynamic".len() as u32 + 1;
    if !verneed.is_empty() {
        write_shdr(
            &mut bytes,
            shoff,
            shndx,
            ShdrSpec {
                name,
                sh_type: SHT_GNU_VERNEED,
                offset: ver_off as u64,
                size: verneed.len() as u64,
                link: dynstr_ndx as u32,
                info: 1,
                ..ShdrSpec::default()
            },
        );
        shndx += 1;
        name += ".gnu.version_r".len() as u32 + 1;
    }
    write_shdr(
        &mut bytes,
        shoff,
        shndx,
        ShdrSpec {
            name,
            sh_type: SHT_STRTAB,
            offset: shstr_off as u64,
            size: shstrtab.len() as u64,
            entsize: 1,
            ..ShdrSpec::default()
        },
    );
    bytes
}

fn push_dyn(out: &mut Vec<u8>, tag: i64, value: u64) {
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&value.to_le_bytes());
}

fn elf_hash(name: &str) -> u32 {
    let mut hash = 0_u32;
    for byte in name.bytes() {
        hash = (hash << 4).wrapping_add(u32::from(byte));
        let high = hash & 0xf000_0000;
        if high != 0 {
            hash ^= high >> 24;
        }
        hash &= !high;
    }
    hash
}

struct PhdrSpec {
    p_type: u32,
    flags: u32,
    offset: u64,
    filesz: u64,
    memsz: u64,
}

fn write_phdr(bytes: &mut [u8], phoff: usize, index: usize, spec: PhdrSpec) {
    let off = phoff + index * ELF64_PHDR;
    write_u32(bytes, off, spec.p_type);
    write_u32(bytes, off + 4, spec.flags);
    write_u64(bytes, off + 8, spec.offset);
    write_u64(bytes, off + 16, spec.offset);
    write_u64(bytes, off + 24, spec.offset);
    write_u64(bytes, off + 32, spec.filesz);
    write_u64(bytes, off + 40, spec.memsz);
    write_u64(bytes, off + 48, 8);
}

#[derive(Default)]
struct ShdrSpec {
    name: u32,
    sh_type: u32,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
    entsize: u64,
}

fn write_shdr(bytes: &mut [u8], shoff: usize, index: usize, spec: ShdrSpec) {
    let off = shoff + index * ELF64_SHDR;
    write_u32(bytes, off, spec.name);
    write_u32(bytes, off + 4, spec.sh_type);
    write_u64(bytes, off + 16, spec.offset);
    write_u64(bytes, off + 24, spec.offset);
    write_u64(bytes, off + 32, spec.size);
    write_u32(bytes, off + 40, spec.link);
    write_u32(bytes, off + 44, spec.info);
    write_u64(bytes, off + 56, spec.entsize);
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn align8(value: usize) -> usize {
    (value + 7) & !7
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ElfError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| ElfError::new("truncated ELF field"))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ElfError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| ElfError::new("truncated ELF field"))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ElfError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| ElfError::new("truncated ELF field"))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, ElfError> {
    Ok(read_u64(bytes, offset)? as i64)
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    pub fn build_complex_elf(
        machine: u16,
        interp: Option<&str>,
        soname: Option<&str>,
        needed: &[&str],
        runpath: Option<&str>,
        rpath: Option<&str>,
        verneeds: &[(&str, &[&str])],
        verdefs: &[(&str, &[&str])],
        extra_tags: &[(i64, u64)],
    ) -> Vec<u8> {
        let mut dynstr = vec![0_u8];
        let mut needed_offs = Vec::new();
        for name in needed {
            needed_offs.push(dynstr.len() as u64);
            dynstr.extend_from_slice(name.as_bytes());
            dynstr.push(0);
        }
        let soname_off = soname.map(|s| {
            let off = dynstr.len() as u64;
            dynstr.extend_from_slice(s.as_bytes());
            dynstr.push(0);
            off
        });
        let runpath_off = runpath.map(|r| {
            let off = dynstr.len() as u64;
            dynstr.extend_from_slice(r.as_bytes());
            dynstr.push(0);
            off
        });
        let rpath_off = rpath.map(|r| {
            let off = dynstr.len() as u64;
            dynstr.extend_from_slice(r.as_bytes());
            dynstr.push(0);
            off
        });

        let mut verneed_bytes = Vec::new();
        for (i, (file, names)) in verneeds.iter().enumerate() {
            let file_off = dynstr.len() as u32;
            dynstr.extend_from_slice(file.as_bytes());
            dynstr.push(0);

            let vn_cnt = names.len() as u16;
            let vn_aux = 16_u32;
            let is_last_vn = i + 1 == verneeds.len();
            let vn_next = if is_last_vn {
                0
            } else {
                16 + (names.len() as u32 * 16)
            };

            verneed_bytes.extend_from_slice(&1_u16.to_le_bytes());
            verneed_bytes.extend_from_slice(&vn_cnt.to_le_bytes());
            verneed_bytes.extend_from_slice(&file_off.to_le_bytes());
            verneed_bytes.extend_from_slice(&vn_aux.to_le_bytes());
            verneed_bytes.extend_from_slice(&vn_next.to_le_bytes());

            for (j, name) in names.iter().enumerate() {
                let name_off = dynstr.len() as u32;
                dynstr.extend_from_slice(name.as_bytes());
                dynstr.push(0);
                let hash = elf_hash(name);
                let is_last_vna = j + 1 == names.len();
                let vna_next = if is_last_vna { 0_u32 } else { 16_u32 };

                verneed_bytes.extend_from_slice(&hash.to_le_bytes());
                verneed_bytes.extend_from_slice(&0_u16.to_le_bytes());
                verneed_bytes.extend_from_slice(&2_u16.to_le_bytes());
                verneed_bytes.extend_from_slice(&name_off.to_le_bytes());
                verneed_bytes.extend_from_slice(&vna_next.to_le_bytes());
            }
        }

        let mut verdef_bytes = Vec::new();
        for (i, (_file, names)) in verdefs.iter().enumerate() {
            let is_last_vd = i + 1 == verdefs.len();
            let vd_cnt = names.len() as u16;
            let vd_aux = 20_u32;
            let vd_next = if is_last_vd {
                0
            } else {
                20 + (names.len() as u32 * 8)
            };

            verdef_bytes.extend_from_slice(&1_u16.to_le_bytes());
            verdef_bytes.extend_from_slice(&0_u16.to_le_bytes()); // vd_flags
            verdef_bytes.extend_from_slice(&1_u16.to_le_bytes()); // vd_ndx
            verdef_bytes.extend_from_slice(&vd_cnt.to_le_bytes());
            verdef_bytes.extend_from_slice(&0_u32.to_le_bytes()); // vd_hash
            verdef_bytes.extend_from_slice(&vd_aux.to_le_bytes());
            verdef_bytes.extend_from_slice(&vd_next.to_le_bytes());

            for (j, name) in names.iter().enumerate() {
                let name_off = dynstr.len() as u32;
                dynstr.extend_from_slice(name.as_bytes());
                dynstr.push(0);
                let is_last_vda = j + 1 == names.len();
                let vda_next = if is_last_vda { 0_u32 } else { 8_u32 };
                verdef_bytes.extend_from_slice(&name_off.to_le_bytes());
                verdef_bytes.extend_from_slice(&vda_next.to_le_bytes());
            }
        }

        let interp_bytes = interp.map(|value| {
            let mut bytes = value.as_bytes().to_vec();
            bytes.push(0);
            bytes
        });

        let phnum = 2 + u16::from(interp.is_some());
        let phoff = ELF64_EHDR;
        let after_ph = phoff + ELF64_PHDR * phnum as usize;
        let interp_off = after_ph;
        let interp_len = interp_bytes.as_ref().map(Vec::len).unwrap_or(0);
        let dyn_off = align8(interp_off + interp_len);

        let mut num_dyn = 1 + needed_offs.len();
        if soname_off.is_some() {
            num_dyn += 1;
        }
        if runpath_off.is_some() {
            num_dyn += 1;
        }
        if rpath_off.is_some() {
            num_dyn += 1;
        }
        if !verneed_bytes.is_empty() {
            num_dyn += 2;
        }
        if !verdef_bytes.is_empty() {
            num_dyn += 2;
        }
        num_dyn += extra_tags.len() + 1; // DT_STRTAB and DT_NULL

        let dyn_size = num_dyn * 16;
        let dynstr_off = dyn_off + dyn_size;
        let verneed_off = align4(dynstr_off + dynstr.len());
        let verdef_off = align4(verneed_off + verneed_bytes.len());
        let total = align8(verdef_off + verdef_bytes.len());

        let mut dynamic = Vec::new();
        push_dyn(&mut dynamic, DT_STRTAB, dynstr_off as u64);
        for off in needed_offs {
            push_dyn(&mut dynamic, DT_NEEDED, off);
        }
        if let Some(off) = soname_off {
            push_dyn(&mut dynamic, DT_SONAME, off);
        }
        if let Some(off) = runpath_off {
            push_dyn(&mut dynamic, DT_RUNPATH, off);
        }
        if let Some(off) = rpath_off {
            push_dyn(&mut dynamic, DT_RPATH, off);
        }
        if !verneed_bytes.is_empty() {
            push_dyn(&mut dynamic, DT_VERNEED, verneed_off as u64);
            push_dyn(&mut dynamic, DT_VERNEEDNUM, verneeds.len() as u64);
        }
        if !verdef_bytes.is_empty() {
            push_dyn(&mut dynamic, DT_VERDEF, verdef_off as u64);
            push_dyn(&mut dynamic, DT_VERDEFNUM, verdefs.len() as u64);
        }
        for (tag, val) in extra_tags {
            push_dyn(&mut dynamic, *tag, *val);
        }
        push_dyn(&mut dynamic, DT_NULL, 0);

        let mut bytes = vec![0_u8; total];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        write_u16(&mut bytes, 16, ET_DYN);
        write_u16(&mut bytes, 18, machine);
        write_u32(&mut bytes, 20, 1);
        write_u64(&mut bytes, 32, phoff as u64);
        write_u64(&mut bytes, 40, 0); // e_shoff = 0
        write_u16(&mut bytes, 52, ELF64_EHDR as u16);
        write_u16(&mut bytes, 54, ELF64_PHDR as u16);
        write_u16(&mut bytes, 56, phnum);
        write_u16(&mut bytes, 58, ELF64_SHDR as u16);
        write_u16(&mut bytes, 60, 0); // e_shnum = 0
        write_u16(&mut bytes, 62, 0);

        let mut ph = 0;
        write_phdr(
            &mut bytes,
            phoff,
            ph,
            PhdrSpec {
                p_type: PT_LOAD,
                flags: 5,
                offset: 0,
                filesz: total as u64,
                memsz: total as u64,
            },
        );
        ph += 1;
        if let Some(interp_bytes) = &interp_bytes {
            write_phdr(
                &mut bytes,
                phoff,
                ph,
                PhdrSpec {
                    p_type: PT_INTERP,
                    flags: 4,
                    offset: interp_off as u64,
                    filesz: interp_bytes.len() as u64,
                    memsz: interp_bytes.len() as u64,
                },
            );
            bytes[interp_off..interp_off + interp_bytes.len()].copy_from_slice(interp_bytes);
            ph += 1;
        }
        write_phdr(
            &mut bytes,
            phoff,
            ph,
            PhdrSpec {
                p_type: PT_DYNAMIC,
                flags: 6,
                offset: dyn_off as u64,
                filesz: dynamic.len() as u64,
                memsz: dynamic.len() as u64,
            },
        );

        bytes[dyn_off..dyn_off + dynamic.len()].copy_from_slice(&dynamic);
        bytes[dynstr_off..dynstr_off + dynstr.len()].copy_from_slice(&dynstr);
        if !verneed_bytes.is_empty() {
            bytes[verneed_off..verneed_off + verneed_bytes.len()].copy_from_slice(&verneed_bytes);
        }
        if !verdef_bytes.is_empty() {
            bytes[verdef_off..verdef_off + verdef_bytes.len()].copy_from_slice(&verdef_bytes);
        }

        bytes
    }

    #[test]
    fn elf_gcc11_tables_match_literal_copies_and_contain_no_sonames() {
        for (table, name) in [
            (crate::elf_gcc11::X86_64_LIBSTDCXX, "X86_64_LIBSTDCXX"),
            (crate::elf_gcc11::X86_64_LIBGCC, "X86_64_LIBGCC"),
            (crate::elf_gcc11::X86_64_LIBGOMP, "X86_64_LIBGOMP"),
            (crate::elf_gcc11::AARCH64_LIBSTDCXX, "AARCH64_LIBSTDCXX"),
            (crate::elf_gcc11::AARCH64_LIBGCC, "AARCH64_LIBGCC"),
            (crate::elf_gcc11::AARCH64_LIBGOMP, "AARCH64_LIBGOMP"),
        ] {
            for entry in table {
                assert_ne!(*entry, "libstdc++.so.6", "{name} must not contain soname");
                assert_ne!(*entry, "libgcc_s.so.1", "{name} must not contain soname");
                assert_ne!(*entry, "libgomp.so.1", "{name} must not contain soname");
            }
        }
    }

    #[test]
    fn admit_elf_handles_corrupt_and_wrong_machine() {
        let bad = b"not an elf file";
        assert!(admit_elf("bin/foo", bad, EM_X86_64, "bin", None, &[]).is_err());

        let truncated = b"\x7fELF";
        assert!(admit_elf("bin/foo", truncated, EM_X86_64, "bin", None, &[]).is_err());

        let mut wrong_class = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        wrong_class[4] = 1; // ELFCLASS32
        assert!(admit_elf("bin/foo", &wrong_class, EM_X86_64, "bin", None, &[]).is_err());

        let good_x86 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &good_x86, EM_X86_64, "bin", None, &[]).is_ok());
        let err = admit_elf("bin/foo", &good_x86, EM_AARCH64, "bin", None, &[]).unwrap_err();
        assert!(err.to_string().contains("unexpected machine"));
    }

    #[test]
    fn admit_elf_interp_checks() {
        // Valid x86_64
        let x86 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &x86, EM_X86_64, "bin", None, &[]).is_ok());

        // Musl loader refuses
        let musl = build_complex_elf(
            EM_X86_64,
            Some("/lib/ld-musl-x86_64.so.1"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &musl, EM_X86_64, "bin", None, &[]).is_err());

        // Aarch64 loader check
        let arm = build_complex_elf(
            EM_AARCH64,
            Some("/lib/ld-linux-aarch64.so.1"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &arm, EM_AARCH64, "bin", None, &[]).is_ok());

        let arm_wrong = build_complex_elf(
            EM_AARCH64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &arm_wrong, EM_AARCH64, "bin", None, &[]).is_err());
    }

    #[test]
    fn admit_elf_glibc_versions() {
        // GLIBC_2.2.5 passes
        let v225 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.2.5"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &v225, EM_X86_64, "bin", None, &[]).is_ok());

        // GLIBC_2.34 passes
        let v234 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &v234, EM_X86_64, "bin", None, &[]).is_ok());

        // GLIBC_2.35 fails
        let v235 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.35"])],
            &[],
            &[],
        );
        let err = admit_elf("bin/foo", &v235, EM_X86_64, "bin", None, &[]).unwrap_err();
        assert!(err.to_string().contains("exceeds ceiling"));

        // GLIBC_2.34.1 fails
        let v2341 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34.1"])],
            &[],
            &[],
        );
        let err = admit_elf("bin/foo", &v2341, EM_X86_64, "bin", None, &[]).unwrap_err();
        assert!(err.to_string().contains("exceeds ceiling"));

        // GLIBC_PRIVATE fails
        let vpriv = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_PRIVATE"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &vpriv, EM_X86_64, "bin", None, &[]).is_err());

        // GLIBC_ABI_DT_RELR fails
        let v_relr = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_ABI_DT_RELR"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &v_relr, EM_X86_64, "bin", None, &[]).is_err());
    }

    #[test]
    fn admit_elf_cxx_runtime_versions() {
        // x86_64 libstdc++ valid version
        let stdc_ok = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libstdc++.so.6", "libc.so.6"],
            None,
            None,
            &[
                ("libstdc++.so.6", &["GLIBCXX_3.4.29"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &stdc_ok, EM_X86_64, "bin", None, &[]).is_ok());

        // x86_64 libstdc++ too high version
        let stdc_bad = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libstdc++.so.6", "libc.so.6"],
            None,
            None,
            &[
                ("libstdc++.so.6", &["GLIBCXX_3.4.30"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        let err = admit_elf("bin/foo", &stdc_bad, EM_X86_64, "bin", None, &[]).unwrap_err();
        assert!(err.to_string().contains("unadmitted symbol version"));

        // CXXABI_1.3.14 fails
        let stdc_bad_cxxabi = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libstdc++.so.6", "libc.so.6"],
            None,
            None,
            &[
                ("libstdc++.so.6", &["CXXABI_1.3.14"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &stdc_bad_cxxabi, EM_X86_64, "bin", None, &[]).is_err());

        // Unknown namespace NOTA_1 on libstdc++.so.6 fails
        let stdc_nota = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libstdc++.so.6", "libc.so.6"],
            None,
            None,
            &[
                ("libstdc++.so.6", &["NOTA_1"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &stdc_nota, EM_X86_64, "bin", None, &[]).is_err());

        // GOMP_4.5 and OMP_1.0 pass
        let gomp_ok = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libgomp.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgomp.so.1", &["GOMP_4.5", "OMP_1.0"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gomp_ok, EM_X86_64, "bin", None, &[]).is_ok());

        // GOMP_5.1 fails
        let gomp_bad1 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libgomp.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgomp.so.1", &["GOMP_5.1"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gomp_bad1, EM_X86_64, "bin", None, &[]).is_err());

        // OMP_5.1 fails
        let gomp_bad2 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libgomp.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgomp.so.1", &["OMP_5.1"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gomp_bad2, EM_X86_64, "bin", None, &[]).is_err());

        // aarch64 libgcc GLIBC_2.0 is admitted via libgcc table
        let gcc_arm_ok = build_complex_elf(
            EM_AARCH64,
            Some("/lib/ld-linux-aarch64.so.1"),
            None,
            &["libgcc_s.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgcc_s.so.1", &["GLIBC_2.0"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gcc_arm_ok, EM_AARCH64, "bin", None, &[]).is_ok());

        // x86_64 libgcc does NOT have GLIBC_2.0
        let gcc_x86_bad = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libgcc_s.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgcc_s.so.1", &["GLIBC_2.0"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gcc_x86_bad, EM_X86_64, "bin", None, &[]).is_err());

        // x86_64 GCC_7.0.0 passes
        let gcc_x86_ok_7 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libgcc_s.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgcc_s.so.1", &["GCC_7.0.0"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gcc_x86_ok_7, EM_X86_64, "bin", None, &[]).is_ok());

        // aarch64 GCC_11.0 passes
        let gcc_arm_ok_11 = build_complex_elf(
            EM_AARCH64,
            Some("/lib/ld-linux-aarch64.so.1"),
            None,
            &["libgcc_s.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgcc_s.so.1", &["GCC_11.0"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gcc_arm_ok_11, EM_AARCH64, "bin", None, &[]).is_ok());

        // x86_64 GCC_4.5.0 fails
        let gcc_x86_bad_45 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libgcc_s.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgcc_s.so.1", &["GCC_4.5.0"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gcc_x86_bad_45, EM_X86_64, "bin", None, &[]).is_err());

        // x86_64 GCC_12.0.0 fails
        let gcc_x86_bad_12 = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libgcc_s.so.1", "libc.so.6"],
            None,
            None,
            &[
                ("libgcc_s.so.1", &["GCC_12.0.0"]),
                ("libc.so.6", &["GLIBC_2.34"]),
            ],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &gcc_x86_bad_12, EM_X86_64, "bin", None, &[]).is_err());
    }

    #[test]
    fn admit_elf_nested_archive_checks() {
        // Nested with system dep only, no runpath, no rpath -> ok
        let nested_ok = build_complex_elf(
            EM_X86_64,
            None,
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(
            admit_elf(
                "lib/rfdetr.tar.gz",
                &nested_ok,
                EM_X86_64,
                "rfdetr",
                Some(("rfdetr.tar.gz", "rfdetr-cli")),
                &[]
            )
            .is_ok()
        );

        // Nested with runpath -> refuses
        let nested_rp = build_complex_elf(
            EM_X86_64,
            None,
            None,
            &["libc.so.6"],
            Some("$ORIGIN/lib"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        let err_rp = admit_elf(
            "lib/rfdetr.tar.gz",
            &nested_rp,
            EM_X86_64,
            "rfdetr",
            Some(("rfdetr.tar.gz", "rfdetr-cli")),
            &[],
        )
        .unwrap_err();
        assert!(err_rp.to_string().contains("RUNPATH"));

        // Nested with rpath -> refuses the same way
        let nested_rpath = build_complex_elf(
            EM_X86_64,
            None,
            None,
            &["libc.so.6"],
            None,
            Some("$ORIGIN/lib"),
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        let err_rpath = admit_elf(
            "lib/rfdetr.tar.gz",
            &nested_rpath,
            EM_X86_64,
            "rfdetr",
            Some(("rfdetr.tar.gz", "rfdetr-cli")),
            &[],
        )
        .unwrap_err();
        assert!(err_rpath.to_string().contains("RPATH"));

        // Nested with non-system dep -> refuses
        let nested_nonsys = build_complex_elf(
            EM_X86_64,
            None,
            None,
            &["libcustom.so.1"],
            None,
            None,
            &[],
            &[],
            &[],
        );
        let err_nonsys = admit_elf(
            "lib/rfdetr.tar.gz",
            &nested_nonsys,
            EM_X86_64,
            "rfdetr",
            Some(("rfdetr.tar.gz", "rfdetr-cli")),
            &[],
        )
        .unwrap_err();
        let err_nonsys_str = err_nonsys.to_string();
        assert!(err_nonsys_str.contains("rfdetr.tar.gz"));
        assert!(err_nonsys_str.contains("rfdetr-cli"));
        assert!(err_nonsys_str.contains("libcustom.so.1"));

        // Nested GLIBC_2.35 refusal contains archive string, member string, and GLIBC_2.35
        let nested_v235 = build_complex_elf(
            EM_X86_64,
            None,
            None,
            &["libc.so.6"],
            None,
            None,
            &[("libc.so.6", &["GLIBC_2.35"])],
            &[],
            &[],
        );
        let err_v235 = admit_elf(
            "lib/rfdetr.tar.gz",
            &nested_v235,
            EM_X86_64,
            "rfdetr",
            Some(("rfdetr.tar.gz", "rfdetr-cli")),
            &[],
        )
        .unwrap_err();
        let err_v235_str = err_v235.to_string();
        assert!(err_v235_str.contains("rfdetr.tar.gz"));
        assert!(err_v235_str.contains("rfdetr-cli"));
        assert!(err_v235_str.contains("GLIBC_2.35"));
    }

    #[test]
    fn admit_elf_search_paths_and_speakers_fixture() {
        let onnx_bytes = build_complex_elf(
            EM_X86_64,
            None,
            Some("libonnxruntime.so.1"),
            &[],
            None,
            None,
            &[],
            &[("libonnxruntime.so.1", &["VERS_1.25.0"])],
            &[],
        );
        let staged_files = vec![(
            "lib/solstone-core-speakers-analyze/libonnxruntime.so.1",
            onnx_bytes.as_slice(),
        )];

        // Valid speakers helper
        let helper_ok = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libonnxruntime.so.1", "libc.so.6"],
            Some("$ORIGIN/../lib/solstone-core-speakers-analyze"),
            None,
            &[
                ("libc.so.6", &["GLIBC_2.34"]),
                ("libonnxruntime.so.1", &["VERS_1.25.0"]),
            ],
            &[],
            &[],
        );
        admit_elf(
            "bin/solstone-core-speakers-analyze",
            &helper_ok,
            EM_X86_64,
            "bin",
            None,
            &staged_files,
        )
        .unwrap();

        // Twin whose verdef does not define VERS_1.25.0 fails, naming file, libonnxruntime.so.1, and VERS_1.25.0
        let onnx_bad_verdef = build_complex_elf(
            EM_X86_64,
            None,
            Some("libonnxruntime.so.1"),
            &[],
            None,
            None,
            &[],
            &[("libonnxruntime.so.1", &["VERS_1.24.0"])],
            &[],
        );
        let staged_bad_verdef = vec![(
            "lib/solstone-core-speakers-analyze/libonnxruntime.so.1",
            onnx_bad_verdef.as_slice(),
        )];
        let err = admit_elf(
            "bin/solstone-core-speakers-analyze",
            &helper_ok,
            EM_X86_64,
            "bin",
            None,
            &staged_bad_verdef,
        )
        .unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("lib/solstone-core-speakers-analyze/libonnxruntime.so.1"));
        assert!(err_msg.contains("libonnxruntime.so.1"));
        assert!(err_msg.contains("VERS_1.25.0"));

        // Shipped file outside resolved search directories fails
        let dummy_so = build_complex_elf(
            EM_X86_64,
            None,
            Some("dummy.so"),
            &[],
            None,
            None,
            &[],
            &[],
            &[],
        );
        let staged_outside = vec![
            (
                "lib/solstone-core-speakers-analyze/dummy.so",
                dummy_so.as_slice(),
            ),
            ("lib/other-dir/libonnxruntime.so.1", onnx_bytes.as_slice()),
        ];
        let err_outside = admit_elf(
            "bin/solstone-core-speakers-analyze",
            &helper_ok,
            EM_X86_64,
            "bin",
            None,
            &staged_outside,
        )
        .unwrap_err();
        assert!(
            err_outside
                .to_string()
                .contains("unsatisfied NEEDED dependency libonnxruntime.so.1")
        );

        // NEEDED libcustom.so outside search directory fails
        let helper_custom = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libcustom.so", "libc.so.6"],
            Some("$ORIGIN/../lib/solstone-core-speakers-analyze"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        let err_custom = admit_elf(
            "bin/solstone-core-speakers-analyze",
            &helper_custom,
            EM_X86_64,
            "bin",
            None,
            &staged_files,
        )
        .unwrap_err();
        assert!(
            err_custom
                .to_string()
                .contains("unsatisfied NEEDED dependency libcustom.so")
        );

        // NEEDED name containing slash fails
        let helper_slash = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["sub/libcustom.so", "libc.so.6"],
            Some("$ORIGIN/../lib/solstone-core-speakers-analyze"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        let err_slash = admit_elf(
            "bin/solstone-core-speakers-analyze",
            &helper_slash,
            EM_X86_64,
            "bin",
            None,
            &staged_files,
        )
        .unwrap_err();
        assert!(err_slash.to_string().contains("contains slash"));

        // Staged libgomp.so.1 inside reached search directory fails as shadowing
        let gomp_bytes = build_complex_elf(
            EM_X86_64,
            None,
            Some("libgomp.so.1"),
            &[],
            None,
            None,
            &[],
            &[],
            &[],
        );
        let staged_with_shadow = vec![
            (
                "lib/solstone-core-speakers-analyze/libonnxruntime.so.1",
                onnx_bytes.as_slice(),
            ),
            (
                "lib/solstone-core-speakers-analyze/libgomp.so.1",
                gomp_bytes.as_slice(),
            ),
        ];
        let helper_gomp = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libonnxruntime.so.1", "libgomp.so.1", "libc.so.6"],
            Some("$ORIGIN/../lib/solstone-core-speakers-analyze"),
            None,
            &[
                ("libc.so.6", &["GLIBC_2.34"]),
                ("libonnxruntime.so.1", &["VERS_1.25.0"]),
            ],
            &[],
            &[],
        );
        assert!(
            admit_elf(
                "bin/solstone-core-speakers-analyze",
                &helper_gomp,
                EM_X86_64,
                "bin",
                None,
                &staged_with_shadow,
            )
            .unwrap_err()
            .to_string()
            .contains("shadows system library libgomp.so.1")
        );

        // inspect_gnu_helper still errors when speakers runpath is wrong
        let helper_info = parse_elf(&helper_ok).unwrap();
        inspect_gnu_helper(
            &helper_info,
            EM_X86_64,
            Some(HELPER_RUNPATH),
            &[HELPER_SONAME],
        )
        .unwrap();

        let err_rp = inspect_gnu_helper(
            &helper_info,
            EM_X86_64,
            Some("$ORIGIN/../other"),
            &[HELPER_SONAME],
        )
        .unwrap_err();
        assert!(err_rp.to_string().contains("DT_RUNPATH"));

        // inspect_gnu_helper still errors when libonnxruntime.so.1 is required and absent
        let helper_no_onnx = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some(HELPER_RUNPATH),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        let no_onnx_info = parse_elf(&helper_no_onnx).unwrap();
        let err_needed = inspect_gnu_helper(
            &no_onnx_info,
            EM_X86_64,
            Some(HELPER_RUNPATH),
            &[HELPER_SONAME],
        )
        .unwrap_err();
        assert!(
            err_needed
                .to_string()
                .contains("DT_NEEDED libonnxruntime.so.1")
        );
    }

    #[test]
    fn admit_elf_search_paths_rules() {
        let dummy = build_complex_elf(
            EM_X86_64,
            None,
            Some("libx.so"),
            &[],
            None,
            None,
            &[],
            &[],
            &[],
        );

        // $ORIGIN on bin/ fails
        let bin_origin = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("$ORIGIN"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &bin_origin, EM_X86_64, "bin", None, &[]).is_err());

        // $ORIGIN on lib/solstone-foo/libx.so passes
        let lib_origin = build_complex_elf(
            EM_X86_64,
            None,
            Some("libx.so"),
            &["libc.so.6"],
            Some("$ORIGIN"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(
            admit_elf(
                "lib/solstone-foo/libx.so",
                &lib_origin,
                EM_X86_64,
                "lib/solstone-foo",
                None,
                &[],
            )
            .is_ok()
        );

        // $ORIGIN/../lib on bin/ file fails
        let bin_dotdot_lib = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("$ORIGIN/../lib"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &bin_dotdot_lib, EM_X86_64, "bin", None, &[]).is_err());

        // $ORIGIN/../lib/solstone-other fails when directory is absent
        let bin_other = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("$ORIGIN/../lib/solstone-other"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &bin_other, EM_X86_64, "bin", None, &[]).is_err());

        // $ORIGIN/../lib/solstone-other fails when directory exists with no regular file (nested subdir)
        let staged_nested = vec![("lib/solstone-other/sub/nested.so", dummy.as_slice())];
        assert!(
            admit_elf(
                "bin/foo",
                &bin_other,
                EM_X86_64,
                "bin",
                None,
                &staged_nested
            )
            .is_err()
        );

        // Passing twin: $ORIGIN/../lib/solstone-other with regular file in that directory passes
        let staged_regular = vec![("lib/solstone-other/libx.so", dummy.as_slice())];
        assert!(
            admit_elf(
                "bin/foo",
                &bin_other,
                EM_X86_64,
                "bin",
                None,
                &staged_regular
            )
            .is_ok()
        );

        // Empty element fails
        let empty_elem = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("$ORIGIN/../lib/solstone-other:"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(
            admit_elf(
                "bin/foo",
                &empty_elem,
                EM_X86_64,
                "bin",
                None,
                &staged_regular
            )
            .is_err()
        );

        // Relative element fails
        let rel_elem = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("relative/lib"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &rel_elem, EM_X86_64, "bin", None, &[]).is_err());

        // Absolute element fails
        let abs_elem = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("/usr/lib"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &abs_elem, EM_X86_64, "bin", None, &[]).is_err());

        // ${ORIGIN} fails
        let tok_origin = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("${ORIGIN}/../lib/solstone-other"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(
            admit_elf(
                "bin/foo",
                &tok_origin,
                EM_X86_64,
                "bin",
                None,
                &staged_regular
            )
            .is_err()
        );

        // $LIB fails
        let tok_lib = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("$ORIGIN/../lib/$LIB"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &tok_lib, EM_X86_64, "bin", None, &[]).is_err());

        // Fails when forbidden element is only in DT_RPATH
        let rpath_bad = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            None,
            Some("$ORIGIN/../lib/$LIB"),
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(admit_elf("bin/foo", &rpath_bad, EM_X86_64, "bin", None, &[]).is_err());

        // Escaping package root
        let escape_root = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("$ORIGIN/../../lib"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(
            admit_elf(
                "bin/foo",
                &escape_root,
                EM_X86_64,
                "bin",
                None,
                &staged_regular
            )
            .is_err()
        );

        // Escaping private namespace
        let escape_ns = build_complex_elf(
            EM_X86_64,
            Some("/lib64/ld-linux-x86-64.so.2"),
            None,
            &["libc.so.6"],
            Some("$ORIGIN/../other"),
            None,
            &[("libc.so.6", &["GLIBC_2.34"])],
            &[],
            &[],
        );
        assert!(
            admit_elf(
                "lib/solstone-core-speakers-analyze/sub",
                &escape_ns,
                EM_X86_64,
                "lib/solstone-core-speakers-analyze",
                None,
                &staged_regular,
            )
            .is_err()
        );
    }

    #[test]
    fn parse_elf_refuses_forbidden_tags() {
        for (tag, name) in [
            (DT_AUDIT, "DT_AUDIT"),
            (DT_DEPAUDIT, "DT_DEPAUDIT"),
            (DT_FILTER, "DT_FILTER"),
            (DT_AUXILIARY, "DT_AUXILIARY"),
        ] {
            let elf = build_complex_elf(
                EM_X86_64,
                None,
                None,
                &[],
                None,
                None,
                &[],
                &[],
                &[(tag, 0)],
            );
            let err = parse_elf(&elf).unwrap_err();
            assert!(err.to_string().contains(name));
        }
    }
}
