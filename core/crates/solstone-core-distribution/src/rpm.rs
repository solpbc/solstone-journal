// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::stage::staged_files;

const LEAD_LEN: usize = 96;

pub fn write_rpm(stage: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = gzip_cpio(stage)?;
    let mut lead = [0_u8; LEAD_LEN];
    lead[0..4].copy_from_slice(&0xedabeedb_u32.to_be_bytes());
    lead[4] = 3;
    lead[5] = 0;
    let name = b"solstone-journal";
    lead[10..10 + name.len()].copy_from_slice(name);
    lead[76..78].copy_from_slice(&1_u16.to_be_bytes());
    fs::write(dest, [lead.as_slice(), payload.as_slice()].concat())
}

fn gzip_cpio(stage: &Path) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut ino = 1_u32;
    for dest in staged_files(stage)? {
        let bytes = fs::read(stage.join(&dest))?;
        write_cpio_member(&mut encoder, ino, &dest, &bytes)?;
        ino += 1;
    }
    write_cpio_member(&mut encoder, ino, "TRAILER!!!", &[])?;
    encoder.finish()
}

fn write_cpio_member<W: Write>(out: &mut W, ino: u32, name: &str, bytes: &[u8]) -> io::Result<()> {
    let name_bytes = format!("{name}\0");
    let namesize = name_bytes.len() as u32;
    let mut header = String::from("070701");
    header.push_str(&format!("{ino:08x}"));
    header.push_str(&format!("{:08x}", 0o100644_u32));
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000001");
    header.push_str("00000000");
    header.push_str(&format!("{:08x}", bytes.len() as u32));
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str(&format!("{namesize:08x}"));
    header.push_str("00000000");
    out.write_all(header.as_bytes())?;
    out.write_all(name_bytes.as_bytes())?;
    pad4(out, 6 + 13 * 8 + name_bytes.len())?;
    out.write_all(bytes)?;
    pad4(out, bytes.len())?;
    Ok(())
}

fn pad4<W: Write>(out: &mut W, used: usize) -> io::Result<()> {
    let pad = (4 - (used % 4)) % 4;
    if pad > 0 {
        out.write_all(&[0_u8; 4][..pad])?;
    }
    Ok(())
}
