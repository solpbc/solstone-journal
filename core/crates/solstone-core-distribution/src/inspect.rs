// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::Path;

use crate::digest::sha256_hex;
use crate::inventory::{
    OS_LINUX, OS_MACOS, OS_WINDOWS, artifact_sidecars, checksum_members_for_os,
    manifest_members_for_os,
};

#[derive(Clone, Copy)]
pub struct ArchiveChainDigests<'a> {
    pub prebuild_input_sha256: &'a str,
    pub delivery_contract_sha256: &'a str,
    pub final_invocation_sha256: &'a str,
}

pub struct ReleaseInfo<'a> {
    pub product: &'a str,
    pub version: &'a str,
    pub target: &'a str,
    pub commit: &'a str,
    pub lock_sha256: &'a str,
    pub archive_chain: Option<ArchiveChainDigests<'a>>,
}

pub fn write_sidecars(
    out_dir: &Path,
    os: &str,
    release: &ReleaseInfo<'_>,
    basename: &str,
) -> io::Result<()> {
    match os {
        OS_LINUX => {}
        OS_MACOS => {}
        OS_WINDOWS => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "windows archive/signing is not implemented in this lode",
            ));
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpected os {other}"),
            ));
        }
    }
    let [sha256, manifest_name, release_name] = artifact_sidecars(basename);
    write_sidecar(&out_dir.join(&release_name), render_release(release))?;

    let mut checksums = String::new();
    for name in checksum_members_for_os(os, basename)
        .map_err(|msg| io::Error::new(io::ErrorKind::InvalidInput, msg))?
    {
        let path = out_dir.join(&name);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sidecar-member-not-regular: {name}"),
            ));
        }
        let digest = sha256_hex(&fs::read(&path)?);
        checksums.push_str(&format!("{digest}  {name}\n"));
    }
    write_sidecar(&out_dir.join(&sha256), checksums)?;

    let mut manifest = String::from("{\n");
    manifest.push_str(&format!("  \"product\": {:?},\n", release.product));
    manifest.push_str(&format!("  \"version\": {:?},\n", release.version));
    manifest.push_str(&format!("  \"target\": {:?},\n", release.target));
    manifest.push_str("  \"files\": {\n");
    let members = manifest_members_for_os(os, basename)
        .map_err(|msg| io::Error::new(io::ErrorKind::InvalidInput, msg))?;
    for (index, name) in members.iter().enumerate() {
        let path = out_dir.join(name);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sidecar-member-not-regular: {name}"),
            ));
        }
        let digest = sha256_hex(&fs::read(path)?);
        let comma = if index + 1 == members.len() { "" } else { "," };
        manifest.push_str(&format!("    {name:?}: {digest:?}{comma}\n"));
    }
    manifest.push_str("  }\n}\n");
    write_sidecar(&out_dir.join(manifest_name), manifest)?;
    Ok(())
}

fn write_sidecar(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sidecar-not-regular: {}", path.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::write(path, bytes)
}

#[must_use]
pub fn render_release(release: &ReleaseInfo<'_>) -> String {
    let mut rendered = format!(
        "product={}\nversion={}\ntarget={}\ncommit={}\nlock_sha256={}\n",
        release.product, release.version, release.target, release.commit, release.lock_sha256
    );
    if let Some(chain) = release.archive_chain {
        rendered.push_str(&format!(
            "archive_prebuild_input_sha256={}\narchive_delivery_contract_sha256={}\narchive_final_invocation_sha256={}\n",
            chain.prebuild_input_sha256,
            chain.delivery_contract_sha256,
            chain.final_invocation_sha256,
        ));
    }
    rendered
}

pub fn self_inspect(out_dir: &Path, basename: &str) -> io::Result<Vec<(String, String)>> {
    let release_name = format!("{basename}.release");
    parse_release(&fs::read_to_string(out_dir.join(release_name))?)
}

pub fn parse_release(text: &str) -> io::Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "release-invalid",
            ));
        };
        pairs.push((key.to_owned(), value.to_owned()));
    }
    if !matches!(pairs.len(), 5 | 8) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "release-invalid",
        ));
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::{ArchiveChainDigests, ReleaseInfo, write_sidecars};
    use std::collections::BTreeMap;
    use std::fs;

    use crate::digest::sha256_hex;
    use crate::inventory::{checksum_members_for_os, manifest_members_for_os};

    const CHAIN_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn macos_sidecars_bind_the_signing_receipt_before_manifest_write() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary directory");
        let out = temporary.path();
        let basename = "solstone-journal-1.2.3-darwin-aarch64";
        fs::write(out.join(format!("{basename}.tar.gz")), b"tar bytes").expect("tar");
        fs::write(out.join(format!("{basename}.pkg")), b"pkg bytes").expect("pkg");
        let release = ReleaseInfo {
            product: "solstone-journal",
            version: "1.2.3",
            target: "macos-arm64",
            commit: "commit",
            lock_sha256: "lock",
            archive_chain: Some(ArchiveChainDigests {
                prebuild_input_sha256: CHAIN_DIGEST,
                delivery_contract_sha256: CHAIN_DIGEST,
                final_invocation_sha256: CHAIN_DIGEST,
            }),
        };

        assert!(write_sidecars(out, "macos", &release, basename).is_err());
        fs::write(
            out.join(format!("{basename}.signing.json")),
            b"{\"os\":\"macos\",\"receipt\":\"constructed\"}\n",
        )
        .expect("receipt");
        write_sidecars(out, "macos", &release, basename).expect("sidecars");
        let release_text =
            fs::read_to_string(out.join(format!("{basename}.release"))).expect("release sidecar");
        assert!(release_text.contains("archive_prebuild_input_sha256="));
        assert!(release_text.contains("archive_delivery_contract_sha256="));
        assert!(release_text.contains("archive_final_invocation_sha256="));

        let checksums =
            fs::read_to_string(out.join(format!("{basename}.sha256"))).expect("checksum sidecar");
        let checksum_members = checksums
            .lines()
            .map(|line| {
                let (digest, name) = line.split_once("  ").expect("checksum line");
                (name.to_owned(), digest.to_owned())
            })
            .collect::<BTreeMap<_, _>>();
        let mut expected_checksum = checksum_members_for_os("macos", basename).expect("macos");
        expected_checksum.sort();
        assert_eq!(
            checksum_members.keys().cloned().collect::<Vec<_>>(),
            expected_checksum
        );
        for (name, digest) in &checksum_members {
            assert_eq!(
                digest,
                &sha256_hex(&fs::read(out.join(name)).expect("member"))
            );
        }

        let manifest = fs::read(out.join(format!("{basename}.manifest.json"))).expect("manifest");
        let files = serde_json::from_slice::<serde_json::Value>(&manifest)
            .expect("manifest json")
            .get("files")
            .and_then(serde_json::Value::as_object)
            .expect("files")
            .iter()
            .map(|(name, digest)| (name.clone(), digest.as_str().expect("digest").to_owned()))
            .collect::<BTreeMap<_, _>>();
        let mut expected_manifest = manifest_members_for_os("macos", basename).expect("macos");
        expected_manifest.sort();
        assert_eq!(files.keys().cloned().collect::<Vec<_>>(), expected_manifest);
        for (name, digest) in &files {
            assert_eq!(
                digest,
                &sha256_hex(&fs::read(out.join(name)).expect("member"))
            );
        }
        assert!(!files.contains_key(&format!("{basename}.manifest.json")));
        assert!(!files.contains_key(&format!("{basename}.manifest.json.minisig")));
    }

    #[test]
    fn write_sidecars_refuses_windows() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary directory");
        let out = temporary.path();
        let basename = "solstone-journal-1.0.22-windows-x86_64";
        let release = ReleaseInfo {
            product: "solstone-journal",
            version: "1.0.22",
            target: "windows-x86_64",
            commit: "commit",
            lock_sha256: "lock",
            archive_chain: None,
        };
        let error = write_sidecars(out, "windows", &release, basename).expect_err("windows");
        assert!(
            error
                .to_string()
                .contains("windows archive/signing is not implemented in this lode"),
            "{error}"
        );
        assert!(!out.join(format!("{basename}.release")).exists());
        assert!(!out.join(format!("{basename}.sha256")).exists());
        assert!(!out.join(format!("{basename}.manifest.json")).exists());
    }
}
