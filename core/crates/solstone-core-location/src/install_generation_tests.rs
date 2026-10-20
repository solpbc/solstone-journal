// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::accuracy::{asset_root, read_installed_strict};

fn offer(bytes: &[u8]) -> Offered {
    Offered {
        version: "fixture-v1".into(),
        files: vec![OfferedFile {
            file: "venues.slpk".into(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size_bytes: bytes.len() as u64,
            origin_key: "assets/fixture/venues.slpk",
        }],
    }
}

#[test]
fn completed_generation_is_reused_before_disk_check_and_remove_revokes_it() {
    let journal = tempfile::tempdir().unwrap();
    let offer = offer(b"model");
    let mut fetches = 0;
    let mut fetch = |_: &OfferedFile, destination: &Path, _: &mut dyn FnMut(u64)| {
        fetches += 1;
        fs::write(destination, b"model").map_err(InstallError::Write)
    };
    let installed = install_offered(
        journal.path(),
        &offer,
        JobSource::Explicit,
        &mut |_| Ok(u64::MAX),
        |_, _| {},
        &mut fetch,
    )
    .unwrap()
    .unwrap();
    assert!(installed.cache_generation.is_some());
    let directory = installed.directory(journal.path());
    // A frozen release may drop the new optional key when rewriting the shared record.
    let mut frozen: serde_json::Value = serde_json::from_slice(
        &fs::read(accuracy_root(journal.path()).join(INSTALLED_RECORD)).unwrap(),
    )
    .unwrap();
    frozen.as_object_mut().unwrap().remove("cache_generation");
    frozen["files"][0]["sha256"] = serde_json::json!("0".repeat(64));
    fs::write(
        accuracy_root(journal.path()).join(INSTALLED_RECORD),
        serde_json::to_vec(&frozen).unwrap(),
    )
    .unwrap();
    refresh_installed_with_fetch(
        journal.path(),
        &offer,
        &mut |_| panic!("verified generation needs no new disk allocation"),
        &mut fetch,
    )
    .unwrap();
    let reused = read_installed(journal.path()).unwrap();
    assert_eq!(reused.directory(journal.path()), directory);
    assert_eq!(fetches, 1);
    remove(journal.path()).unwrap();
    assert!(read_installed_strict(journal.path()).unwrap().is_none());
    assert!(!directory.exists());
    // Revocation remains durable; startup maintenance cannot reinstall.
    refresh_installed_with_fetch(
        journal.path(),
        &offer,
        &mut |_| Ok(u64::MAX),
        &mut |_, _, _| panic!("removed packs cannot fetch automatically"),
    )
    .unwrap();
}

#[test]
fn corrupted_control_unknown_entries_and_active_readers_prevent_collection() {
    use solstone_core_artifact_download::generation::hold_use;
    let journal = tempfile::tempdir().unwrap();
    let offer = offer(b"model");
    let inventory = generation(&offer).unwrap();
    let directory = inventory.directory(&asset_root(journal.path()).join("generations"));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("venues.slpk"), b"model").unwrap();
    inventory.seal(&directory).unwrap();
    fs::create_dir_all(accuracy_root(journal.path())).unwrap();
    let record = accuracy_root(journal.path()).join(INSTALLED_RECORD);
    fs::write(&record, b"{").unwrap();
    assert!(maintain(journal.path(), Some(&offer)).is_err());
    assert!(directory.exists());
    fs::remove_file(&record).unwrap();
    let hold = hold_use(&directory).unwrap();
    maintain(journal.path(), Some(&offer)).unwrap();
    assert!(directory.exists());
    drop(hold);
    fs::write(directory.join("owner-file"), b"preserve").unwrap();
    maintain(journal.path(), Some(&offer)).unwrap();
    assert!(directory.exists());
    fs::remove_file(directory.join("owner-file")).unwrap();
    maintain(journal.path(), Some(&offer)).unwrap();
    assert!(!directory.exists());
}

#[cfg(all(test, feature = "full-tests"))]
#[test]
fn interrupted_installer_reuses_the_same_partial_and_credits_remaining_disk() {
    use solstone_core_artifact_download::{RuntimeFetchLoopback, with_runtime_fetch_loopback};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    let server = thread::spawn(move || {
        for response in [b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nab".as_slice(),b"HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nContent-Range: bytes 2-3/4\r\nConnection: close\r\n\r\ncd".as_slice()] {
            let deadline = Instant::now()+Duration::from_secs(5);
            let mut connection = loop { match listener.accept() { Ok((c,_))=>break c, Err(e) if e.kind()==io::ErrorKind::WouldBlock && Instant::now()<deadline => thread::sleep(Duration::from_millis(10)), e=>panic!("expected request: {e:?}") } };
            connection.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut bytes = Vec::new(); while !bytes.ends_with(b"\r\n\r\n") { let mut one=[0]; connection.read_exact(&mut one).unwrap();bytes.push(one[0]);assert!(bytes.len()<16384); }
            seen.lock().unwrap().push(String::from_utf8(bytes).unwrap().to_ascii_lowercase()); connection.write_all(response).unwrap();
        }
    });
    let journal = tempfile::tempdir().unwrap();
    let offer = offer(b"abcd");
    let sha: &'static str = Box::leak(offer.files[0].sha256.clone().into_boxed_str());
    with_runtime_fetch_loopback(
        RuntimeFetchLoopback {
            base_url: base,
            allowed_hosts: vec!["127.0.0.1".into()],
            allow_http: true,
        },
        || {
            let mut fetch = |_: &OfferedFile, path: &Path, progress: &mut dyn FnMut(u64)| {
                let handle = solstone_core_assets::runtime_fetch_handle_fixture(
                    UNIT,
                    "assets/fixture/venues.slpk",
                    sha,
                    4,
                );
                solstone_core_artifact_download::download_runtime_fetch(&handle, path, |n, _| {
                    progress(n)
                })
                .map(|_| ())
                .map_err(|e| InstallError::Download {
                    file: "venues.slpk".into(),
                    detail: e.to_string(),
                })
            };
            assert!(
                install_offered(
                    journal.path(),
                    &offer,
                    JobSource::Explicit,
                    &mut |_| Ok(u64::MAX),
                    |_, _| {},
                    &mut fetch
                )
                .is_err()
            );
            // Exactly two bytes remain; charging the full download would refuse.
            let installed = install_offered(
                journal.path(),
                &offer,
                JobSource::Explicit,
                &mut |_| Ok(HEADROOM_BYTES + 2),
                |_, _| {},
                &mut fetch,
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                fs::read(installed.directory(journal.path()).join("venues.slpk")).unwrap(),
                b"abcd"
            );
        },
    );
    server.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("range: bytes=2-\r\n"));
}

#[test]
fn frozen_predecessor_refresh_and_sweep_preserve_current_backing_for_reupgrade() {
    use solstone_core_assets::windows_predecessor::{
        windows_predecessor, with_windows_predecessor_fixture,
    };
    // Frozen record shape and directory/sweep rules from the supported release.
    // Its serde reader ignores the newer cache_generation field.
    #[derive(Serialize, Deserialize)]
    struct Frozen {
        version: String,
        #[serde(default)]
        folder: String,
        files: Vec<InstalledFile>,
    }
    impl Frozen {
        fn directory(&self, journal: &Path) -> std::path::PathBuf {
            accuracy_root(journal).join(if self.folder.is_empty() {
                &self.version
            } else {
                &self.folder
            })
        }
    }
    let frozen_refresh = |journal: &Path, offered: &Offered, bytes: &[(String, Vec<u8>)]| {
        let record_path = accuracy_root(journal).join(INSTALLED_RECORD);
        let old: Frozen = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        let live = old.directory(journal);
        let matching = offered.files.iter().all(|f| {
            fs::metadata(live.join(&f.file)).is_ok_and(|m| m.is_file() && m.len() == f.size_bytes)
                && fs::read(live.join(&f.file))
                    .is_ok_and(|b| format!("{:x}", Sha256::digest(&b)) == f.sha256)
        });
        let folder = if matching {
            live.file_name().unwrap().to_string_lossy().into_owned()
        } else {
            "frozen-supported.42".into()
        };
        let directory = accuracy_root(journal).join(&folder);
        if !matching {
            fs::create_dir_all(&directory).unwrap();
            for (name, body) in bytes {
                fs::write(directory.join(name), body).unwrap();
            }
        }
        let installed = Frozen {
            version: offered.version.clone(),
            folder: folder.clone(),
            files: offered
                .files
                .iter()
                .map(|f| InstalledFile {
                    file: f.file.clone(),
                    sha256: f.sha256.clone(),
                    size_bytes: f.size_bytes,
                })
                .collect(),
        };
        fs::write(record_path, serde_json::to_vec(&installed).unwrap()).unwrap();
        for entry in fs::read_dir(accuracy_root(journal)).unwrap().flatten() {
            if entry.path().is_dir() && entry.file_name() != folder.as_str() {
                fs::remove_dir_all(entry.path()).unwrap();
            }
        }
        !matching
    };
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy");
    let prior_bytes: Vec<_> = ["containment.slpk", "venues.slpk"]
        .into_iter()
        .map(|name| (name.to_owned(), fs::read(fixtures.join(name)).unwrap()))
        .collect();
    let current_bytes: Vec<_> = prior_bytes
        .iter()
        .map(|(name, body)| {
            let mut body = body.clone();
            body.push(b'c');
            (name.clone(), body)
        })
        .collect();
    let make_offer = |version: &str, bytes: &[(String, Vec<u8>)]| Offered {
        version: version.into(),
        files: bytes
            .iter()
            .map(|(name, body)| OfferedFile {
                file: name.clone(),
                sha256: format!("{:x}", Sha256::digest(body)),
                size_bytes: body.len() as u64,
                origin_key: if name == "venues.slpk" {
                    "assets/fixture/venues.slpk"
                } else {
                    "assets/fixture/containment.slpk"
                },
            })
            .collect(),
    };
    let prior = make_offer("supported-b", &prior_bytes);
    let current = make_offer("current-c", &current_bytes);
    let mut predecessor = windows_predecessor().unwrap().clone();
    for asset in &mut predecessor
        .predecessor
        .groups
        .iter_mut()
        .find(|g| g.unit == UNIT)
        .unwrap()
        .assets
    {
        let member = prior
            .files
            .iter()
            .find(|m| asset.origin_key.ends_with(&format!("/{}", m.file)))
            .unwrap();
        asset.sha256 = member.sha256.clone();
        asset.size_bytes = member.size_bytes;
    }
    with_windows_predecessor_fixture(predecessor, || {
        let journal = tempfile::tempdir().unwrap();
        let legacy = accuracy_root(journal.path()).join("legacy-current");
        fs::create_dir_all(&legacy).unwrap();
        for (name, body) in &current_bytes {
            fs::write(legacy.join(name), body).unwrap();
        }
        let record = Installed {
            version: current.version.clone(),
            folder: "legacy-current".into(),
            cache_generation: None,
            files: current
                .files
                .iter()
                .map(|f| InstalledFile {
                    file: f.file.clone(),
                    sha256: f.sha256.clone(),
                    size_bytes: f.size_bytes,
                })
                .collect(),
        };
        publish_installed_record(journal.path(), &record).unwrap();
        refresh_installed_with_fetch(
            journal.path(),
            &current,
            &mut |_| panic!("matching legacy uses no additional model allocation"),
            &mut |_, _, _| panic!("matching legacy must not fetch"),
        )
        .unwrap();
        let backing = read_installed(journal.path())
            .unwrap()
            .directory(journal.path());
        assert_ne!(backing, legacy);
        assert!(frozen_refresh(journal.path(), &prior, &prior_bytes));
        assert!(!legacy.exists());
        assert!(backing.exists());
        refresh_installed_with_fetch(
            journal.path(),
            &current,
            &mut |_| panic!("retained current generation is already allocated"),
            &mut |_, _, _| panic!("reupgrade must reuse current backing"),
        )
        .unwrap();
        assert_eq!(
            read_installed(journal.path())
                .unwrap()
                .directory(journal.path()),
            backing
        );
        assert!(!frozen_refresh(journal.path(), &prior, &prior_bytes));
        assert!(backing.exists());
    });
}

#[test]
fn legacy_remove_keeps_control_truth_if_cleanup_carrier_cannot_be_written() {
    let journal = tempfile::tempdir().unwrap();
    let directory = accuracy_root(journal.path()).join("legacy");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("venues.slpk"), b"model").unwrap();
    let offered = offer(b"model");
    let installed = Installed {
        version: "legacy".into(),
        folder: String::new(),
        cache_generation: None,
        files: offered
            .files
            .iter()
            .map(|f| InstalledFile {
                file: f.file.clone(),
                sha256: f.sha256.clone(),
                size_bytes: f.size_bytes,
            })
            .collect(),
    };
    publish_installed_record(journal.path(), &installed).unwrap();
    fs::create_dir_all(asset_root(journal.path())).unwrap();
    fs::write(asset_root(journal.path()).join("legacy"), b"owner-file").unwrap();
    assert!(remove(journal.path()).is_err());
    assert_eq!(read_installed(journal.path()), Some(installed));
    assert_eq!(fs::read(directory.join("venues.slpk")).unwrap(), b"model");
}
