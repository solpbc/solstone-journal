// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal's mark as a Windows icon file. The window draws the mark at
//! each size; this packs those PNG images into one `.ico`, which Windows reads
//! for the window, the taskbar button and the Start-menu entry.

/// The sizes Windows asks an icon for, from the smallest list entry to the
/// largest tile.
pub const ICON_SIDES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// Pack `(side, png)` images into an icon file. Every side must be one of
/// [`ICON_SIDES`] and appear once, and every image must be a PNG of that
/// side; anything else is refused rather than written as a broken icon.
pub fn pack_png_icon(images: &[(u32, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if images.is_empty() {
        return Err("no icon images".to_owned());
    }
    let mut sides = Vec::with_capacity(images.len());
    for (side, png) in images {
        if !ICON_SIDES.contains(side) || sides.contains(side) {
            return Err(format!("unexpected icon size {side}"));
        }
        if png_side(png) != Some((*side, *side)) {
            return Err(format!("the {side} px icon image is not a {side} px PNG"));
        }
        sides.push(*side);
    }
    let count = u16::try_from(images.len()).map_err(|_| "too many icon images".to_owned())?;
    let directory = 6 + 16 * images.len();
    let mut out = Vec::new();
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = directory;
    for (side, png) in images {
        // A 256 px image is recorded as 0 in the one-byte size fields.
        let byte = if *side >= 256 { 0 } else { *side as u8 };
        out.extend_from_slice(&[byte, byte, 0, 0]);
        out.extend_from_slice(&1_u16.to_le_bytes());
        out.extend_from_slice(&32_u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += png.len();
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    Ok(out)
}

/// The width and height a PNG declares in its header.
fn png_side(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || png[..8] != PNG_SIGNATURE || &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(side: u32) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&13_u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&side.to_be_bytes());
        bytes.extend_from_slice(&side.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    #[test]
    fn packs_each_image_where_its_directory_entry_points() {
        let images = vec![(16, png(16)), (256, png(256))];
        let icon = pack_png_icon(&images).unwrap();
        assert_eq!(&icon[..6], &[0, 0, 1, 0, 2, 0]);
        for (index, (side, image)) in images.iter().enumerate() {
            let entry = &icon[6 + 16 * index..6 + 16 * (index + 1)];
            assert_eq!(entry[0], if *side == 256 { 0 } else { *side as u8 });
            let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
            assert_eq!(&icon[offset..offset + size], image.as_slice());
        }
    }

    #[test]
    fn refuses_an_image_that_is_not_the_size_it_claims() {
        assert!(pack_png_icon(&[(32, png(16))]).is_err());
        assert!(pack_png_icon(&[(33, png(33))]).is_err());
        assert!(pack_png_icon(&[(16, png(16)), (16, png(16))]).is_err());
        assert!(pack_png_icon(&[(16, b"not a png".to_vec())]).is_err());
        assert!(pack_png_icon(&[]).is_err());
    }
}
