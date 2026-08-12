// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Deliberately duplicated from `solstone-core-body-source/src/whitespace.rs`.
/// This small table is kept local so steward pruning stays independent from the
/// unrelated body-import domain.
pub(crate) const fn is_python_whitespace(code_point: u32) -> bool {
    matches!(
        code_point,
        0x0009..=0x000d
            | 0x001c..=0x001f
            | 0x0020
            | 0x0085
            | 0x00a0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
    )
}

// Unicode 15.1.0 UnicodeData.txt, General_Category=Nd. Every listed range is
// ten consecutive code points with digit values zero through nine. Verify a
// regenerated table with `python3 -c 'import unicodedata; ...unicodedata.decimal(...)'`.
const ND_ZEROES: [u32; 68] = [
    0x0030, 0x0660, 0x06f0, 0x07c0, 0x0966, 0x09e6, 0x0a66, 0x0ae6, 0x0b66, 0x0be6, 0x0c66, 0x0ce6,
    0x0d66, 0x0de6, 0x0e50, 0x0ed0, 0x0f20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80,
    0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
    0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0,
    0x11650, 0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60,
    0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0,
    0x1e950, 0x1fbf0,
];

pub(crate) fn nd_digit(code_point: u32) -> Option<u8> {
    let index = ND_ZEROES.partition_point(|start| *start <= code_point);
    let start = *ND_ZEROES.get(index.checked_sub(1)?)?;
    (code_point <= start + 9).then_some((code_point - start) as u8)
}
