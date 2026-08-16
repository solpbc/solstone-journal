// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::Path;

use tar::{Builder, EntryType, Header};

use crate::ar::{member, read_archive, write_archive};
use crate::stage::staged_files;
use crate::tar::{gzip_bytes, list_tar_gz_bytes};

const DATA_PREFIX: &str = "usr/";

pub fn write_deb(stage: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let control = gzip_bytes(&control_tar()?)?;
    let data = gzip_bytes(&data_tar(stage)?)?;
    let mut out = fs::File::create(dest)?;
    write_archive(
        &mut out,
        &[
            ("debian-binary", b"2.0\n".as_slice()),
            ("control.tar.gz", control.as_slice()),
            ("data.tar.gz", data.as_slice()),
        ],
    )
}

fn control_tar() -> io::Result<Vec<u8>> {
    let control = b"Package: solstone-journal\nVersion: 0\nArchitecture: amd64\nDescription: solstone-journal\n";
    let mut builder = Builder::new(Vec::new());
    append_regular(&mut builder, "control", control)?;
    builder.finish()?;
    Ok(builder.into_inner()?)
}

fn data_tar(stage: &Path) -> io::Result<Vec<u8>> {
    let mut builder = Builder::new(Vec::new());
    for dest in staged_files(stage)? {
        // Commit #3: the deb writer relocates bin/ under the system prefix
        // and has no share/** mapping yet.
        let Some(name) = dest.strip_prefix("bin/") else {
            continue;
        };
        let bytes = fs::read(stage.join(&dest))?;
        append_regular(&mut builder, &format!("{DATA_PREFIX}bin/{name}"), &bytes)?;
    }
    builder.finish()?;
    Ok(builder.into_inner()?)
}

fn append_regular<W: io::Write>(
    builder: &mut Builder<W>,
    dest: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_path(dest)?;
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    builder.append(&header, bytes)?;
    Ok(())
}

pub fn list_deb(path: &Path) -> io::Result<Vec<String>> {
    let bytes = fs::read(path)?;
    let members = read_archive(&bytes)?;
    let data = member(&members, "data.tar.gz")?;
    let mut dests = Vec::new();
    for member_path in list_tar_gz_bytes(data)? {
        if let Some(rest) = member_path.strip_prefix(DATA_PREFIX) {
            dests.push(rest.to_owned());
        }
    }
    dests.sort();
    dests.dedup();
    Ok(dests)
}
