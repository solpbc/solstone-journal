// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

fn encoded(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([1, 2, 3]));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

#[test]
fn each_accepted_type_is_checked_against_its_own_bytes() {
    let png = encoded(ImageFormat::Png, 3, 5);
    let jpeg = encoded(ImageFormat::Jpeg, 3, 5);
    assert!(check_image("screenshot_screen.png", &png).is_ok());
    assert!(check_image("screenshot_screen.jpg", &jpeg).is_ok());
    assert!(check_image("screenshot_screen.jpeg", &jpeg).is_ok());
    assert!(check_image("screenshot_screen.jpg", &png).is_err());
    assert!(check_image("screenshot_screen.png", &jpeg).is_err());
    assert!(check_image("screenshot_screen.png", b"\x89PNG\r\n\x1a\n").is_err());
}

#[test]
fn a_header_claiming_too_many_pixels_is_refused_before_decoding() {
    let mut png = encoded(ImageFormat::Png, 1, 1);
    // IHDR width and height sit at bytes 16..24.
    png[16..20].copy_from_slice(&20_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&20_000u32.to_be_bytes());
    // Re-seal the IHDR chunk (type and data, bytes 12..29) with its CRC.
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    let error = check_image("screenshot_screen.png", &png).unwrap_err();
    assert!(error.1.contains("over"), "{error:?}");
}

#[test]
fn a_fresh_journal_has_received_nothing_and_a_record_reads_back() {
    let journal = tempfile::tempdir().unwrap();
    let id = "ab".repeat(32);
    assert_eq!(lookup(journal.path(), &id).unwrap(), None);
    let received = Received {
        day: "20261103".to_owned(),
        segment: "143207_1".to_owned(),
        received_at: "2026-11-10T13:00:00Z".to_owned(),
    };
    record(journal.path(), &id, &received).unwrap();
    assert_eq!(lookup(journal.path(), &id).unwrap(), Some(received));
    assert_eq!(lookup(journal.path(), &"cd".repeat(32)).unwrap(), None);
}

#[test]
fn a_record_that_is_not_a_directory_fails_closed() {
    let journal = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(journal.path().join("config/screenshots")).unwrap();
    std::fs::write(journal.path().join(RECEIVED_DIR), b"").unwrap();
    assert!(lookup(journal.path(), &"ab".repeat(32)).is_err());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
