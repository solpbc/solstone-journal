// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Mach-O inspection for the macOS distribution tree.
//!
//! The macOS sibling of `elf.rs`, and deliberately not a port of it: the Linux
//! rules do not carry across. `$ORIGIN` has no meaning here — `@loader_path`
//! does; there is no static libSystem, so "no dynamic dependencies at all" is
//! unreachable and the core-family rule becomes "only system dylibs"; and the
//! GLIBC verneed ceiling becomes the `LC_BUILD_VERSION` deployment target.
//!
//! ⚠ `code_signature` reports the presence of an `LC_CODE_SIGNATURE` load
//! command and nothing more. The linker emits an ad-hoc signature for every
//! arm64 binary it produces, so this flag is TRUE on a binary we have never
//! signed. It answers "is there a signature blob", never "is this signed by
//! us" — that question belongs to `apple::verify_signed`, which asserts the
//! authority and team identifier.

const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_CIGAM_64: u32 = 0xcffa_edfe;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_CIGAM: u32 = 0xbeba_feca;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const FAT_CIGAM_64: u32 = 0xbfba_feca;

const HEADER_64: usize = 32;

const LC_REQ_DYLD: u32 = 0x8000_0000;
const LC_LOAD_DYLIB: u32 = 0x0000_000c;
const LC_ID_DYLIB: u32 = 0x0000_000d;
const LC_LOAD_WEAK_DYLIB: u32 = 0x18 | LC_REQ_DYLD;
const LC_REEXPORT_DYLIB: u32 = 0x1f | LC_REQ_DYLD;
const LC_RPATH: u32 = 0x1c | LC_REQ_DYLD;
const LC_CODE_SIGNATURE: u32 = 0x0000_001d;
const LC_BUILD_VERSION: u32 = 0x0000_0032;
const LC_VERSION_MIN_MACOSX: u32 = 0x0000_0024;

const PLATFORM_MACOS: u32 = 1;

pub const MH_EXECUTE: u32 = 2;
pub const MH_DYLIB: u32 = 6;
const MH_PIE: u32 = 0x0020_0000;

/// `@loader_path` is the macOS answer to the Linux `$ORIGIN` rule: the helper
/// finds its bundled runtime relative to its own file, not to the process.
pub const HELPER_RPATH: &str = "@loader_path/../lib/solstone-core-speakers-analyze";
/// The install name the ONNX Runtime dylib is linked against on macOS. Its
/// Linux sibling is a SONAME (`libonnxruntime.so.1`); here it is an `@rpath`
/// install name, and the version is part of it.
pub const HELPER_INSTALL_NAME: &str = "@rpath/libonnxruntime.1.25.0.dylib";

/// Every dependency of a shipped binary must live under one of these. macOS has
/// no static libSystem, so an empty dependency list is not achievable and this
/// prefix rule is what replaces the Linux "no `DT_NEEDED` at all" assertion.
pub const SYSTEM_DYLIB_PREFIXES: &[&str] = &["/usr/lib/", "/System/Library/"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachoInfo {
    pub cputype: u32,
    pub filetype: u32,
    pub install_name: Option<String>,
    pub needed: Vec<String>,
    pub rpaths: Vec<String>,
    pub min_os: Option<(u32, u32)>,
    pub code_signature: bool,
    pub position_independent: bool,
}

#[derive(Debug)]
pub struct MachoError {
    message: String,
}

impl MachoError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for MachoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MachoError {}

#[must_use]
pub const fn cputype_arm64() -> u32 {
    0x0100_000c
}

#[must_use]
pub const fn cputype_x86_64() -> u32 {
    0x0100_0007
}

#[must_use]
pub fn cputype_for_arch(arch: &str) -> Option<u32> {
    match arch {
        "arm64" | "aarch64" => Some(cputype_arm64()),
        "x86_64" => Some(cputype_x86_64()),
        _ => None,
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, MachoError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| MachoError::new("unexpected:\n  truncated mach-o"))
}

fn lc_string(command: &[u8], offset: usize) -> Result<String, MachoError> {
    let start = offset;
    if start >= command.len() {
        return Err(MachoError::new("unexpected:\n  lc_str out of range"));
    }
    let end = command[start..]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(command.len(), |index| start + index);
    String::from_utf8(command[start..end].to_vec())
        .map_err(|_| MachoError::new("unexpected:\n  lc_str not utf-8"))
}

/// Parse a thin little-endian 64-bit Mach-O. A universal ("fat") container is
/// refused by name rather than silently reading its first slice: one target
/// produces one architecture, and a fat file would let a wrong-arch slice ride
/// through every arch assertion below.
pub fn parse_macho(bytes: &[u8]) -> Result<MachoInfo, MachoError> {
    let magic = u32_at(bytes, 0)?;
    let magic_be = u32::from_be_bytes(
        bytes
            .get(0..4)
            .and_then(|slice| slice.try_into().ok())
            .ok_or_else(|| MachoError::new("unexpected:\n  truncated mach-o"))?,
    );
    if [FAT_MAGIC, FAT_CIGAM, FAT_MAGIC_64, FAT_CIGAM_64].contains(&magic)
        || [FAT_MAGIC, FAT_MAGIC_64].contains(&magic_be)
    {
        return Err(MachoError::new("unexpected:\n  universal mach-o"));
    }
    if magic == MH_CIGAM_64 {
        return Err(MachoError::new("unexpected:\n  big-endian mach-o"));
    }
    if magic != MH_MAGIC_64 {
        return Err(MachoError::new("unexpected:\n  not a 64-bit mach-o"));
    }

    let cputype = u32_at(bytes, 4)?;
    let filetype = u32_at(bytes, 12)?;
    let ncmds = u32_at(bytes, 16)?;
    let sizeofcmds = u32_at(bytes, 20)? as usize;
    let flags = u32_at(bytes, 24)?;

    let commands = bytes
        .get(HEADER_64..HEADER_64 + sizeofcmds)
        .ok_or_else(|| MachoError::new("unexpected:\n  truncated load commands"))?;

    let mut info = MachoInfo {
        cputype,
        filetype,
        install_name: None,
        needed: Vec::new(),
        rpaths: Vec::new(),
        min_os: None,
        code_signature: false,
        position_independent: flags & MH_PIE != 0,
    };

    let mut cursor = 0usize;
    for _ in 0..ncmds {
        if cursor + 8 > commands.len() {
            return Err(MachoError::new("unexpected:\n  truncated load command"));
        }
        let cmd = u32_at(commands, cursor)?;
        let cmdsize = u32_at(commands, cursor + 4)? as usize;
        if cmdsize < 8 || cursor + cmdsize > commands.len() {
            return Err(MachoError::new("unexpected:\n  load command size"));
        }
        let command = &commands[cursor..cursor + cmdsize];
        match cmd {
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB => {
                let offset = u32_at(command, 8)? as usize;
                info.needed.push(lc_string(command, offset)?);
            }
            LC_ID_DYLIB => {
                let offset = u32_at(command, 8)? as usize;
                info.install_name = Some(lc_string(command, offset)?);
            }
            LC_RPATH => {
                let offset = u32_at(command, 8)? as usize;
                info.rpaths.push(lc_string(command, offset)?);
            }
            LC_CODE_SIGNATURE => info.code_signature = true,
            LC_BUILD_VERSION => {
                let platform = u32_at(command, 8)?;
                if platform == PLATFORM_MACOS {
                    let minos = u32_at(command, 12)?;
                    info.min_os = Some((minos >> 16, (minos >> 8) & 0xff));
                }
            }
            LC_VERSION_MIN_MACOSX => {
                let version = u32_at(command, 8)?;
                info.min_os = Some((version >> 16, (version >> 8) & 0xff));
            }
            _ => {}
        }
        cursor += cmdsize;
    }
    Ok(info)
}

#[must_use]
pub fn is_system_dylib(path: &str) -> bool {
    SYSTEM_DYLIB_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

#[must_use]
pub fn looks_like_macho(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(0..4).and_then(|slice| slice.try_into().ok()),
        Some(magic)
            if [
                MH_MAGIC_64.to_le_bytes(),
                MH_CIGAM_64.to_le_bytes(),
                FAT_MAGIC.to_be_bytes(),
                FAT_MAGIC_64.to_be_bytes(),
            ]
            .contains(&magic)
    )
}

fn check_common(info: &MachoInfo, cputype: u32, ceiling: (u32, u32), unexpected: &mut Vec<String>) {
    if info.cputype != cputype {
        unexpected.push(format!("cputype {:#x}", info.cputype));
    }
    match info.min_os {
        Some(version) if version <= ceiling => {}
        Some(version) => unexpected.push(format!(
            "LC_BUILD_VERSION minos {}.{}",
            version.0, version.1
        )),
        None => unexpected.push("missing LC_BUILD_VERSION".to_owned()),
    }
    for dependency in &info.needed {
        if !is_system_dylib(dependency) {
            unexpected.push(format!("LC_LOAD_DYLIB {dependency}"));
        }
    }
}

fn finish(mut unexpected: Vec<String>) -> Result<(), MachoError> {
    if unexpected.is_empty() {
        return Ok(());
    }
    unexpected.sort();
    unexpected.dedup();
    Err(MachoError::new(format!(
        "unexpected:\n  {}",
        unexpected.join("\n  ")
    )))
}

/// The macOS core family: an executable whose every dependency is a system
/// dylib and which carries no rpath at all. This is what `crt-static` buys on
/// Linux and it is the strongest statement available here.
pub fn inspect_core_family(
    info: &MachoInfo,
    cputype: u32,
    ceiling: (u32, u32),
) -> Result<(), MachoError> {
    let mut unexpected = Vec::new();
    check_common(info, cputype, ceiling, &mut unexpected);
    if info.filetype != MH_EXECUTE {
        unexpected.push(format!("filetype {}", info.filetype));
    }
    for rpath in &info.rpaths {
        unexpected.push(format!("LC_RPATH {rpath}"));
    }
    finish(unexpected)
}

/// The ONNX-linked helper: system dylibs, plus exactly one `@rpath` dependency
/// resolved by exactly one `@loader_path` rpath. Both halves are asserted —
/// an rpath with no dependency through it, and a dependency with no rpath, are
/// each a broken install that a "runs on the build host" check cannot see.
pub fn inspect_helper(
    info: &MachoInfo,
    cputype: u32,
    ceiling: (u32, u32),
    rpath: &str,
    install_name: &str,
) -> Result<(), MachoError> {
    let mut unexpected = Vec::new();
    if info.filetype != MH_EXECUTE {
        unexpected.push(format!("filetype {}", info.filetype));
    }
    if info.cputype != cputype {
        unexpected.push(format!("cputype {:#x}", info.cputype));
    }
    match info.min_os {
        Some(version) if version <= ceiling => {}
        Some(version) => unexpected.push(format!(
            "LC_BUILD_VERSION minos {}.{}",
            version.0, version.1
        )),
        None => unexpected.push("missing LC_BUILD_VERSION".to_owned()),
    }
    for dependency in &info.needed {
        if dependency == install_name || is_system_dylib(dependency) {
            continue;
        }
        unexpected.push(format!("LC_LOAD_DYLIB {dependency}"));
    }
    if !info.needed.iter().any(|item| item == install_name) {
        unexpected.push(format!("missing LC_LOAD_DYLIB {install_name}"));
    }
    if info.rpaths.iter().all(|item| item != rpath) {
        unexpected.push(format!("missing LC_RPATH {rpath}"));
    }
    for item in &info.rpaths {
        if item != rpath {
            unexpected.push(format!("LC_RPATH {item}"));
        }
    }
    finish(unexpected)
}

/// A shipped payload dylib: the bytes Gatekeeper evaluates on load. Its
/// `LC_ID_DYLIB` install name is what the helper's `LC_LOAD_DYLIB` must name,
/// so the two assertions together prove the pair actually resolves.
pub fn inspect_payload_dylib(
    info: &MachoInfo,
    cputype: u32,
    ceiling: (u32, u32),
    install_name: &str,
) -> Result<(), MachoError> {
    let mut unexpected = Vec::new();
    check_common(info, cputype, ceiling, &mut unexpected);
    if info.filetype != MH_DYLIB {
        unexpected.push(format!("filetype {}", info.filetype));
    }
    match info.install_name.as_deref() {
        Some(name) if name == install_name => {}
        other => unexpected.push(format!("LC_ID_DYLIB {other:?}")),
    }
    finish(unexpected)
}

// ---------------------------------------------------------------------------
// Fixtures. Synthetic Mach-O images with known answers, so a zero from the
// parser can be distinguished from a blind parser.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FixtureSpec<'a> {
    pub cputype: u32,
    pub filetype: u32,
    pub install_name: Option<&'a str>,
    pub needed: &'a [&'a str],
    pub rpaths: &'a [&'a str],
    pub min_os: Option<(u32, u32)>,
    pub code_signature: bool,
    pub position_independent: bool,
}

impl Default for FixtureSpec<'_> {
    fn default() -> Self {
        Self {
            cputype: cputype_arm64(),
            filetype: MH_EXECUTE,
            install_name: None,
            needed: &[],
            rpaths: &[],
            min_os: Some((14, 0)),
            code_signature: false,
            position_independent: true,
        }
    }
}

fn push_lc_str_command(out: &mut Vec<u8>, cmd: u32, value: &str) {
    let header = 12usize;
    let mut body = value.as_bytes().to_vec();
    body.push(0);
    while !(header + body.len()).is_multiple_of(8) {
        body.push(0);
    }
    let size = header + body.len();
    out.extend_from_slice(&cmd.to_le_bytes());
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(header as u32).to_le_bytes());
    out.extend_from_slice(&body);
}

fn push_dylib_command(out: &mut Vec<u8>, cmd: u32, value: &str) {
    let header = 24usize;
    let mut body = value.as_bytes().to_vec();
    body.push(0);
    while !(header + body.len()).is_multiple_of(8) {
        body.push(0);
    }
    let size = header + body.len();
    out.extend_from_slice(&cmd.to_le_bytes());
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(header as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&body);
}

#[must_use]
pub fn fixture(spec: &FixtureSpec<'_>) -> Vec<u8> {
    let mut commands = Vec::new();
    let mut ncmds = 0u32;
    if let Some((major, minor)) = spec.min_os {
        commands.extend_from_slice(&LC_BUILD_VERSION.to_le_bytes());
        commands.extend_from_slice(&24u32.to_le_bytes());
        commands.extend_from_slice(&PLATFORM_MACOS.to_le_bytes());
        commands.extend_from_slice(&((major << 16) | (minor << 8)).to_le_bytes());
        commands.extend_from_slice(&((major << 16) | (minor << 8)).to_le_bytes());
        commands.extend_from_slice(&0u32.to_le_bytes());
        ncmds += 1;
    }
    if let Some(name) = spec.install_name {
        push_dylib_command(&mut commands, LC_ID_DYLIB, name);
        ncmds += 1;
    }
    for dependency in spec.needed {
        push_dylib_command(&mut commands, LC_LOAD_DYLIB, dependency);
        ncmds += 1;
    }
    for rpath in spec.rpaths {
        push_lc_str_command(&mut commands, LC_RPATH, rpath);
        ncmds += 1;
    }
    if spec.code_signature {
        commands.extend_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
        commands.extend_from_slice(&16u32.to_le_bytes());
        commands.extend_from_slice(&0u32.to_le_bytes());
        commands.extend_from_slice(&0u32.to_le_bytes());
        ncmds += 1;
    }

    let mut out = Vec::new();
    out.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
    out.extend_from_slice(&spec.cputype.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&spec.filetype.to_le_bytes());
    out.extend_from_slice(&ncmds.to_le_bytes());
    out.extend_from_slice(&(commands.len() as u32).to_le_bytes());
    let flags = if spec.position_independent { MH_PIE } else { 0 };
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&commands);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM: &[&str] = &[
        "/usr/lib/libSystem.B.dylib",
        "/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation",
    ];

    #[test]
    fn parses_every_load_command_it_asserts_on() {
        let bytes = fixture(&FixtureSpec {
            needed: &[SYSTEM[0], SYSTEM[1], HELPER_INSTALL_NAME],
            rpaths: &[HELPER_RPATH],
            code_signature: true,
            ..FixtureSpec::default()
        });
        let info = parse_macho(&bytes).expect("fixture parses");
        assert_eq!(info.cputype, cputype_arm64());
        assert_eq!(info.filetype, MH_EXECUTE);
        assert_eq!(info.needed.len(), 3);
        assert_eq!(info.rpaths, vec![HELPER_RPATH.to_owned()]);
        assert_eq!(info.min_os, Some((14, 0)));
        assert!(info.code_signature);
        assert!(info.position_independent);
    }

    #[test]
    fn core_family_admits_system_only_and_refuses_each_way_it_can_be_wrong() {
        let ceiling = (14, 0);
        let good = parse_macho(&fixture(&FixtureSpec {
            needed: SYSTEM,
            ..FixtureSpec::default()
        }))
        .unwrap();
        inspect_core_family(&good, cputype_arm64(), ceiling).expect("system-only core");

        // A non-system dependency is the failure the rule exists to catch.
        let bundled = parse_macho(&fixture(&FixtureSpec {
            needed: &["@rpath/libonnxruntime.1.25.0.dylib"],
            ..FixtureSpec::default()
        }))
        .unwrap();
        let error = inspect_core_family(&bundled, cputype_arm64(), ceiling).unwrap_err();
        assert!(error.to_string().contains("LC_LOAD_DYLIB @rpath"));

        // An rpath on a core-family binary means it expects to find something
        // beside itself, which this family never may.
        let with_rpath = parse_macho(&fixture(&FixtureSpec {
            needed: SYSTEM,
            rpaths: &["@loader_path/../lib"],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(
            inspect_core_family(&with_rpath, cputype_arm64(), ceiling)
                .unwrap_err()
                .to_string()
                .contains("LC_RPATH")
        );

        // Wrong architecture.
        assert!(
            inspect_core_family(&good, cputype_x86_64(), ceiling)
                .unwrap_err()
                .to_string()
                .contains("cputype")
        );

        // Deployment target above the ceiling: builds here, refuses to launch
        // on the oldest macOS we claim.
        let newer = parse_macho(&fixture(&FixtureSpec {
            needed: SYSTEM,
            min_os: Some((15, 0)),
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(
            inspect_core_family(&newer, cputype_arm64(), ceiling)
                .unwrap_err()
                .to_string()
                .contains("minos 15.0")
        );
    }

    #[test]
    fn helper_requires_both_halves_of_the_loader_pair() {
        let ceiling = (14, 0);
        let good = parse_macho(&fixture(&FixtureSpec {
            needed: &[SYSTEM[0], HELPER_INSTALL_NAME],
            rpaths: &[HELPER_RPATH],
            ..FixtureSpec::default()
        }))
        .unwrap();
        inspect_helper(
            &good,
            cputype_arm64(),
            ceiling,
            HELPER_RPATH,
            HELPER_INSTALL_NAME,
        )
        .expect("helper with both halves");

        // rpath present, dependency absent — the helper would never load the
        // bundled runtime and nothing about the file looks wrong.
        let no_dependency = parse_macho(&fixture(&FixtureSpec {
            needed: &[SYSTEM[0]],
            rpaths: &[HELPER_RPATH],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(
            inspect_helper(
                &no_dependency,
                cputype_arm64(),
                ceiling,
                HELPER_RPATH,
                HELPER_INSTALL_NAME
            )
            .unwrap_err()
            .to_string()
            .contains("missing LC_LOAD_DYLIB")
        );

        // dependency present, rpath absent — resolves on a host that happens to
        // have the library elsewhere and fails on an owner's machine.
        let no_rpath = parse_macho(&fixture(&FixtureSpec {
            needed: &[SYSTEM[0], HELPER_INSTALL_NAME],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(
            inspect_helper(
                &no_rpath,
                cputype_arm64(),
                ceiling,
                HELPER_RPATH,
                HELPER_INSTALL_NAME
            )
            .unwrap_err()
            .to_string()
            .contains("missing LC_RPATH")
        );

        // An absolute build-host rpath is the classic leak.
        let host_rpath = parse_macho(&fixture(&FixtureSpec {
            needed: &[SYSTEM[0], HELPER_INSTALL_NAME],
            rpaths: &[HELPER_RPATH, "/Users/build/onnx/lib"],
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(
            inspect_helper(
                &host_rpath,
                cputype_arm64(),
                ceiling,
                HELPER_RPATH,
                HELPER_INSTALL_NAME
            )
            .unwrap_err()
            .to_string()
            .contains("/Users/build/onnx/lib")
        );
    }

    #[test]
    fn payload_dylib_install_name_must_match_what_the_helper_loads() {
        let ceiling = (14, 0);
        let good = parse_macho(&fixture(&FixtureSpec {
            filetype: MH_DYLIB,
            install_name: Some(HELPER_INSTALL_NAME),
            needed: SYSTEM,
            ..FixtureSpec::default()
        }))
        .unwrap();
        inspect_payload_dylib(&good, cputype_arm64(), ceiling, HELPER_INSTALL_NAME)
            .expect("payload dylib");

        let wrong = parse_macho(&fixture(&FixtureSpec {
            filetype: MH_DYLIB,
            install_name: Some("/opt/homebrew/lib/libonnxruntime.1.25.0.dylib"),
            needed: SYSTEM,
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(
            inspect_payload_dylib(&wrong, cputype_arm64(), ceiling, HELPER_INSTALL_NAME)
                .unwrap_err()
                .to_string()
                .contains("LC_ID_DYLIB")
        );

        // An executable handed to the dylib rule must not pass it.
        let executable = parse_macho(&fixture(&FixtureSpec {
            needed: SYSTEM,
            ..FixtureSpec::default()
        }))
        .unwrap();
        assert!(
            inspect_payload_dylib(&executable, cputype_arm64(), ceiling, HELPER_INSTALL_NAME)
                .unwrap_err()
                .to_string()
                .contains("filetype")
        );
    }

    #[test]
    fn universal_and_foreign_containers_are_refused_by_name() {
        let mut fat = Vec::new();
        fat.extend_from_slice(&FAT_MAGIC.to_be_bytes());
        fat.extend_from_slice(&2u32.to_be_bytes());
        fat.resize(64, 0);
        assert!(
            parse_macho(&fat)
                .unwrap_err()
                .to_string()
                .contains("universal")
        );

        let elf = b"\x7fELF\x02\x01\x01\x00 and then some padding bytes";
        assert!(
            parse_macho(elf)
                .unwrap_err()
                .to_string()
                .contains("not a 64-bit mach-o")
        );

        assert!(parse_macho(&[]).is_err());
    }

    #[test]
    fn looks_like_macho_separates_the_two_families() {
        let macho = fixture(&FixtureSpec::default());
        assert!(looks_like_macho(&macho));
        assert!(!looks_like_macho(b"#!/bin/sh\nexec solstone-core \"$@\"\n"));
        assert!(!looks_like_macho(b"\x7fELF\x02\x01\x01\x00"));
    }

    #[test]
    fn system_prefix_rule_admits_only_the_two_os_roots() {
        assert!(is_system_dylib("/usr/lib/libSystem.B.dylib"));
        assert!(is_system_dylib(
            "/System/Library/Frameworks/Foundation.framework/Versions/C/Foundation"
        ));
        assert!(!is_system_dylib("/usr/local/lib/libonnxruntime.dylib"));
        assert!(!is_system_dylib("/opt/homebrew/lib/libfoo.dylib"));
        assert!(!is_system_dylib("@rpath/libonnxruntime.1.25.0.dylib"));
        assert!(!is_system_dylib("@loader_path/../lib/libfoo.dylib"));
    }
}
