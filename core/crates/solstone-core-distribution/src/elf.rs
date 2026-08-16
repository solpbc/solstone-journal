// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const ELF64_EHDR: usize = 64;
const ELF64_PHDR: usize = 56;
const ELF64_SHDR: usize = 64;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const DT_NEEDED: i64 = 1;
const DT_STRTAB: i64 = 5;
const DT_NULL: i64 = 0;
const DT_RUNPATH: i64 = 29;
const DT_RPATH: i64 = 15;
const SHT_STRTAB: u32 = 3;
const SHT_DYNAMIC: u32 = 6;
const SHT_GNU_VERNEED: u32 = 0x6fff_fffe;

pub const HELPER_RUNPATH: &str = "$ORIGIN/../lib/solstone-core-speakers-analyze";
pub const HELPER_SONAME: &str = "libonnxruntime.so.1";
pub const GLIBC_CEILING: (u32, u32) = (2, 27);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfInfo {
    pub machine: u16,
    pub interp: Option<String>,
    pub needed: Vec<String>,
    pub runpath: Option<String>,
    pub rpath: Option<String>,
    pub glibc: Option<(u32, u32)>,
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
                DT_RUNPATH => runpath = Some(value),
                DT_RPATH => rpath = Some(value),
                _ => {}
            }
            cursor += 16;
        }
    }

    let dynstr = dynstr_section(bytes, shoff, shentsize, shnum)
        .or_else(|_| dynstr_from_vaddr(bytes, phoff, phentsize, phnum, dynstr_vaddr))?;
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
    let glibc = glibc_from_verneed(bytes, shoff, shentsize, shnum, &dynstr, interp.is_some())?;
    Ok(ElfInfo {
        machine,
        interp,
        needed,
        runpath,
        rpath,
        glibc,
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
        let name = read_u32(bytes, off)?;
        let _ = name;
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
    for index in 0..phnum {
        let off = phoff + index * phentsize;
        if read_u32(bytes, off)? != PT_LOAD {
            continue;
        }
        let p_offset = read_u64(bytes, off + 8)?;
        let p_vaddr = read_u64(bytes, off + 16)?;
        let p_filesz = read_u64(bytes, off + 32)?;
        if vaddr >= p_vaddr && vaddr < p_vaddr + p_filesz {
            let file = (p_offset + (vaddr - p_vaddr)) as usize;
            return Ok(bytes[file..].to_vec());
        }
    }
    Err(ElfError::new("DT_STRTAB is not in a PT_LOAD"))
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

fn glibc_from_verneed(
    bytes: &[u8],
    shoff: usize,
    shentsize: usize,
    shnum: usize,
    dynstr: &[u8],
    dynamic: bool,
) -> Result<Option<(u32, u32)>, ElfError> {
    let mut found = None;
    for index in 0..shnum {
        let off = shoff + index * shentsize;
        if read_u32(bytes, off + 4)? != SHT_GNU_VERNEED {
            continue;
        }
        found = Some(off);
        break;
    }
    let Some(off) = found else {
        if dynamic {
            return Err(ElfError::new("could not read GNU version needs"));
        }
        return Ok(None);
    };
    let offset = read_u64(bytes, off + 24)? as usize;
    let size = read_u64(bytes, off + 32)? as usize;
    if offset + size > bytes.len() {
        return Err(ElfError::new("truncated SHT_GNU_verneed"));
    }
    let table = &bytes[offset..offset + size];
    let mut cursor = 0;
    let mut ceiling: Option<(u32, u32)> = None;
    while cursor + 16 <= table.len() {
        let vn_cnt = read_u16(table, cursor + 2)?;
        let vn_aux = read_u32(table, cursor + 8)? as usize;
        let vn_next = read_u32(table, cursor + 12)? as usize;
        let mut aux = cursor + vn_aux;
        for _ in 0..vn_cnt {
            if aux + 16 > table.len() {
                return Err(ElfError::new("truncated vernaux"));
            }
            let name = dynstr_string(dynstr, read_u32(table, aux + 8)? as usize)?;
            if let Some(version) = parse_glibc_version(&name) {
                ceiling = Some(match ceiling {
                    Some(existing) => existing.max(version),
                    None => version,
                });
            }
            let next = read_u32(table, aux + 12)? as usize;
            if next == 0 {
                break;
            }
            aux += next;
        }
        if vn_next == 0 {
            break;
        }
        cursor += vn_next;
    }
    Ok(ceiling)
}

fn parse_glibc_version(name: &str) -> Option<(u32, u32)> {
    let rest = name.strip_prefix("GLIBC_")?;
    let mut parts = rest.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
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
    match info.glibc {
        Some(version) if version <= GLIBC_CEILING => {}
        Some(version) => unexpected.push(format!("GLIBC_{}.{}", version.0, version.1)),
        None => unexpected.push("missing GLIBC verneed".to_owned()),
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

    let mut dynamic = Vec::new();
    push_dyn(&mut dynamic, DT_STRTAB, 0);
    for off in &needed_off {
        push_dyn(&mut dynamic, DT_NEEDED, *off);
    }
    if let Some(off) = runpath_off {
        push_dyn(&mut dynamic, DT_RUNPATH, off);
    }
    push_dyn(&mut dynamic, DT_NULL, 0);

    let mut verneed = Vec::new();
    if glibc.is_some() {
        // Elf64_Verneed
        verneed.extend_from_slice(&1_u16.to_le_bytes());
        verneed.extend_from_slice(&1_u16.to_le_bytes());
        verneed.extend_from_slice(&libc_off.to_le_bytes());
        verneed.extend_from_slice(&16_u32.to_le_bytes());
        verneed.extend_from_slice(&0_u32.to_le_bytes());
        // Elf64_Vernaux
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
    let dynstr_off = dyn_off + dynamic.len();
    let ver_off = align4(dynstr_off + dynstr.len());
    let shstr_off = ver_off + verneed.len();
    let shoff = align8(shstr_off + shstrtab.len());
    let total = shoff + ELF64_SHDR * shnum as usize;
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

    // Patch DT_STRTAB to the file offset (vaddr == offset for this fixture).
    write_u64(&mut dynamic, 8, dynstr_off as u64);
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
