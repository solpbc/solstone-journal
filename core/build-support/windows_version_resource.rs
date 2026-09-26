// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Shared by the build scripts of every Rust program in the Windows payload
// (`include!`d, so it adds no crate and no lockfile entry).
//
// Windows reads a program's name and publisher from its version resource, not
// from its signature. Without one, the Windows Firewall prompt for the journal
// named the file and showed "Publisher: Unknown" on a binary validly signed by
// sol pbc. This writes a VS_VERSIONINFO resource file and links it into the
// named binary, on Windows MSVC targets only. `bin` is the Cargo target;
// `file_name` is what the payload installs it as.

#[allow(dead_code)]
fn windows_version_resource(bin: &str, file_name: &str) {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../build-support/windows_version_resource.rs");
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let path = out_dir.join(format!("{bin}-version.res"));
    let bytes = windows_version_resource_bytes(file_name, &version);
    std::fs::write(&path, bytes).expect("write the Windows version resource");
    println!("cargo:rustc-link-arg-bin={bin}={}", path.display());
}

#[allow(dead_code)]
fn windows_version_resource_bytes(file_name: &str, version: &str) -> Vec<u8> {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    let version_ms = (u32::from(major) << 16) | u32::from(minor);
    let version_ls = u32::from(patch) << 16;
    let dotted = format!("{major}.{minor}.{patch}.0");

    let mut fixed = Vec::with_capacity(52);
    for value in [
        0xFEEF_04BD_u32, // signature
        0x0001_0000,     // structure version
        version_ms,      // file version
        version_ls,
        version_ms, // product version
        version_ls,
        0x3F,        // flags mask
        0,           // flags
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,           // subtype
        0,           // date
        0,
    ] {
        fixed.extend_from_slice(&value.to_le_bytes());
    }

    let strings: Vec<Vec<u8>> = [
        ("CompanyName", "sol pbc"),
        ("FileDescription", "journal"),
        ("FileVersion", dotted.as_str()),
        ("InternalName", file_name.trim_end_matches(".exe")),
        ("OriginalFilename", file_name),
        ("ProductName", "journal"),
        ("ProductVersion", version),
    ]
    .iter()
    .map(|(key, value)| {
        let text = utf16z(value);
        let length_in_words = (text.len() / 2) as u16;
        version_node(key, &text, length_in_words, 1, &[])
    })
    .collect();
    let table = version_node("040904B0", &[], 0, 1, &strings);
    let string_file_info = version_node("StringFileInfo", &[], 0, 1, &[table]);
    let mut translation = Vec::new();
    translation.extend_from_slice(&0x0409_u16.to_le_bytes()); // en-US
    translation.extend_from_slice(&1200_u16.to_le_bytes()); // UTF-16
    let var = version_node("Translation", &translation, 4, 0, &[]);
    let var_file_info = version_node("VarFileInfo", &[], 0, 1, &[var]);
    let info = version_node(
        "VS_VERSION_INFO",
        &fixed,
        52,
        0,
        &[string_file_info, var_file_info],
    );

    // A .res file opens with an empty entry, then one entry per resource.
    let mut res = Vec::new();
    res.extend_from_slice(&resource_header(0, 0, 0, 0, 0));
    res.extend_from_slice(&resource_header(info.len() as u32, 16, 1, 0x0030, 0x0409));
    res.extend_from_slice(&info);
    pad4(&mut res);
    res
}

#[allow(dead_code)]
fn resource_header(size: u32, kind: u16, name: u16, flags: u16, language: u16) -> Vec<u8> {
    let mut header = Vec::with_capacity(32);
    header.extend_from_slice(&size.to_le_bytes());
    header.extend_from_slice(&32_u32.to_le_bytes());
    for word in [0xFFFF_u16, kind, 0xFFFF, name] {
        header.extend_from_slice(&word.to_le_bytes());
    }
    header.extend_from_slice(&0_u32.to_le_bytes()); // data version
    header.extend_from_slice(&flags.to_le_bytes());
    header.extend_from_slice(&language.to_le_bytes());
    header.extend_from_slice(&0_u32.to_le_bytes()); // version
    header.extend_from_slice(&0_u32.to_le_bytes()); // characteristics
    header
}

// One VS_VERSIONINFO node: length, value length, type, key, value, children.
// Each node starts on a 4-byte boundary, and its length excludes trailing padding.
#[allow(dead_code)]
fn version_node(
    key: &str,
    value: &[u8],
    value_length: u16,
    kind: u16,
    children: &[Vec<u8>],
) -> Vec<u8> {
    let mut node = vec![0, 0];
    node.extend_from_slice(&value_length.to_le_bytes());
    node.extend_from_slice(&kind.to_le_bytes());
    node.extend_from_slice(&utf16z(key));
    pad4(&mut node);
    node.extend_from_slice(value);
    for child in children {
        pad4(&mut node);
        node.extend_from_slice(child);
    }
    let length = node.len() as u16;
    node[..2].copy_from_slice(&length.to_le_bytes());
    node
}

#[allow(dead_code)]
fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[allow(dead_code)]
fn pad4(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}
