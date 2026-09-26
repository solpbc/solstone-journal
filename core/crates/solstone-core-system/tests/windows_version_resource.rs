// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The Windows version resource names the journal and sol pbc in Windows'
// own prompts (the Windows Firewall prompt reads its publisher from here).
// Parse the generated resource back and check every field.

include!("../../../build-support/windows_version_resource.rs");

use std::collections::BTreeMap;

fn u16_at(bytes: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn align4(at: usize) -> usize {
    (at + 3) & !3
}

fn wide(bytes: &[u8], at: usize) -> (String, usize) {
    let mut units = Vec::new();
    let mut end = at;
    loop {
        let unit = u16_at(bytes, end) as u16;
        end += 2;
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    (String::from_utf16(&units).unwrap(), end)
}

// Returns (key, value bytes, children) for the node at `at`.
fn node(bytes: &[u8], at: usize) -> (String, Vec<u8>, Vec<usize>, usize) {
    let length = u16_at(bytes, at);
    let value_length = u16_at(bytes, at + 2);
    let text = u16_at(bytes, at + 4) == 1;
    let (key, after_key) = wide(bytes, at + 6);
    let value_at = align4(after_key);
    let value_bytes = if text { value_length * 2 } else { value_length };
    let value = bytes[value_at..value_at + value_bytes].to_vec();
    let mut children = Vec::new();
    let mut child = align4(value_at + value_bytes);
    while child < at + length {
        children.push(child);
        child = align4(child + u16_at(bytes, child));
    }
    (key, value, children, length)
}

#[test]
fn the_resource_names_the_journal_and_sol_pbc() {
    let bytes = windows_version_resource_bytes("journal.exe", "2.0.20");
    // Empty leading entry, then RT_VERSION (16), id 1, en-US.
    assert_eq!(&bytes[..8], &[0, 0, 0, 0, 32, 0, 0, 0]);
    let header = &bytes[32..64];
    let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    assert_eq!(&header[8..16], &[0xFF, 0xFF, 16, 0, 0xFF, 0xFF, 1, 0]);
    assert_eq!(u16_at(header, 22), 0x0409);

    let info = &bytes[64..64 + size];
    let (key, fixed, children, length) = node(info, 0);
    assert_eq!((key.as_str(), length), ("VS_VERSION_INFO", size));
    assert_eq!(fixed.len(), 52);
    assert_eq!(&fixed[..4], &0xFEEF_04BD_u32.to_le_bytes());
    assert_eq!(&fixed[8..16], &[0, 0, 2, 0, 0, 0, 20, 0]);

    let (key, _, tables, _) = node(info, children[0]);
    assert_eq!(key, "StringFileInfo");
    let (key, _, strings, _) = node(info, tables[0]);
    assert_eq!(key, "040904B0");
    let values: BTreeMap<String, String> = strings
        .iter()
        .map(|&at| {
            let (key, value, _, _) = node(info, at);
            let units: Vec<u16> = value
                .chunks(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .take_while(|&unit| unit != 0)
                .collect();
            (key, String::from_utf16(&units).unwrap())
        })
        .collect();
    let expected: BTreeMap<String, String> = [
        ("CompanyName", "sol pbc"),
        ("FileDescription", "journal"),
        ("FileVersion", "2.0.20.0"),
        ("InternalName", "journal"),
        ("OriginalFilename", "journal.exe"),
        ("ProductName", "journal"),
        ("ProductVersion", "2.0.20"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(values, expected);

    let (key, _, vars, _) = node(info, children[1]);
    assert_eq!(key, "VarFileInfo");
    let (key, translation, _, _) = node(info, vars[0]);
    assert_eq!(key, "Translation");
    assert_eq!(translation, [0x09, 0x04, 0xB0, 0x04]);
}
