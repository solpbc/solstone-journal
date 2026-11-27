// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Photos Library source (phase 1): metadata-only holding area, deterministic
//! Rules v2 selection, lease-based wanted list, Keep & Read pipeline into Part A
//! `photo_observation.json`, and durable owner-trashed suppression.

pub mod geo;
pub mod holding;
pub mod keep;
pub mod received;
pub mod selection;
pub mod types;

pub use holding::{
    DEVICE_CONNECTED_WINDOW_SECS, HOLDING_DIR_REL, can_run_others_face_check, capable_device_kinds,
    choose_range, choose_range_with_live_seen, dashboard_state, dashboard_state_at,
    dashboard_state_with_live_seen, holding_root, lease_wanted, list_landed_photos,
    mark_wanted_non_copy, photos_library_enabled, photos_library_visible, photos_needs_attention,
    rebuild_from_plain_json, rebuild_sqlite_index, record_capable_device, remove_capable_device,
    scan_batch, scan_changes, scan_finish, scan_start, set_paused, set_paused_with_live_seen,
    sqlite_cache_path, turn_off, turn_off_with_live_seen, update_device_status, widen_range,
    widen_range_with_live_seen,
};
pub use keep::{
    CopyIngestOutcome, CopyMetadataOverride, TrashPhotosResult, TrashSegmentRef, ingest_photo_copy,
    ingest_photo_copy_with, trash_chosen_photos,
};
pub use received::{
    PhotoReceipt, ReceivedLedgerSnapshot, is_segment_live, lookup_received, record_received,
    record_suppression_for_segment,
};
pub use selection::{SmartAlbumContractError, run_selection, validate_smart_album_contract};

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;

    use image::{DynamicImage, ImageBuffer, Rgb, codecs::jpeg::JpegEncoder};
    use solstone_core_entity::faceprints::{
        self, Encoder, Face, FaceKey, Metadata, Source, photo_match,
    };
    use solstone_core_format::content::{ContentResolution, classify};
    use solstone_core_generate::{
        ClientError, GenerateRequest, GenerateResponse, GeneratedResponse,
    };
    use solstone_core_import::publish::NativePublicationOperations;
    use solstone_core_import_sources::image::WireClient;
    use solstone_core_import_sources::photo_faces::{DetectedPhotoFaces, PhotoFaceDetector};
    use solstone_core_segment::photo_observation::{
        BelongingClass, PhotoObservationResolution, PhotoObservationState, read_photo_observation,
        resolve_photo_observation,
    };
    use solstone_core_segment::{PHOTOS_SOURCE, is_syncing_source_stream};

    use super::*;
    use crate::geo::GeoResolver;
    use crate::types::{
        ALBUM_CLOUD_SHARED, ALBUM_REGULAR, AssetAlbumRef, AssetExtended, AssetLocation,
        DecisionKind, OwnerCorrections, RangeSpec, SMART_CAPTURED_BY_ME, SMART_HIDDEN,
        SMART_RECENTLY_SAVED, SMART_RECENTS, SMART_SCREENSHOTS, ScanAssetRow, SelectionConfig,
        SmartAlbumCatalogEntry,
    };

    fn std_catalog() -> Vec<SmartAlbumCatalogEntry> {
        vec![
            SmartAlbumCatalogEntry {
                id: "sa-cbm".to_owned(),
                subtype: SMART_CAPTURED_BY_ME,
                title: Some("Captured by Me".to_owned()),
                count: 10,
            },
            SmartAlbumCatalogEntry {
                id: "sa-saved".to_owned(),
                subtype: SMART_RECENTLY_SAVED,
                title: Some("Recently Saved".to_owned()),
                count: 3,
            },
            SmartAlbumCatalogEntry {
                id: "sa-recents".to_owned(),
                subtype: SMART_RECENTS,
                title: Some("Recents".to_owned()),
                count: 15,
            },
        ]
    }

    fn make_row(id: &str, captured_at: &str, smart_subtypes: &[i64]) -> ScanAssetRow {
        ScanAssetRow {
            id: id.to_owned(),
            captured_at: Some(captured_at.to_owned()),
            media_type: "photo".to_owned(),
            albums: smart_subtypes
                .iter()
                .map(|&st| AssetAlbumRef {
                    id: format!("sa-{st}"),
                    kind: "smart".to_owned(),
                    subtype: st,
                    title: None,
                })
                .collect(),
            ..ScanAssetRow::default()
        }
    }

    fn distinct_jpeg(seed: u8) -> Vec<u8> {
        let pixels = ImageBuffer::from_fn(64, 48, |x, y| {
            Rgb([
                (x as u8).wrapping_add(seed),
                (y as u8).wrapping_mul(3).wrapping_add(seed),
                seed.wrapping_mul(17),
            ])
        });
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(&DynamicImage::ImageRgb8(pixels))
            .unwrap();
        out
    }

    fn generated(text: &str) -> GenerateResponse {
        GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test".to_owned(),
            usage: serde_json::json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))
    }

    struct SeqWire(Mutex<Vec<Result<GenerateResponse, ClientError>>>);

    impl SeqWire {
        fn new(responses: Vec<Result<GenerateResponse, ClientError>>) -> Self {
            Self(Mutex::new(responses))
        }
    }

    impl WireClient for SeqWire {
        fn execute(&self, _request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            let mut guard = self.0.lock().unwrap();
            if guard.is_empty() {
                Ok(generated(
                    r#"{"description":"A sunny trail photo.","visible_text":null,"belonging_class":"lived_moment"}"#,
                ))
            } else {
                guard.remove(0)
            }
        }
    }

    struct StubDetector {
        encoder: Encoder,
        faces: Vec<photo_match::PhotoFaceCandidate>,
    }

    impl PhotoFaceDetector for StubDetector {
        fn detect_photo(
            &self,
            _image_path: &Path,
            _journal: Option<&Path>,
            _with_crops: bool,
        ) -> Result<DetectedPhotoFaces, String> {
            Ok(DetectedPhotoFaces {
                encoder: self.encoder.clone(),
                width: 512,
                height: 384,
                faces: self.faces.clone(),
            })
        }
    }

    fn unit_vec(dim: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; faceprints::WIDTH];
        v[dim] = 1.0;
        v
    }

    fn seed_meeting_person(journal: &Path, entity_id: &str, name: &str, dim: usize) -> Encoder {
        let ent_dir = journal.join("entities").join(entity_id);
        fs::create_dir_all(&ent_dir).unwrap();
        fs::write(
            ent_dir.join("entity.json"),
            serde_json::to_vec(&serde_json::json!({
                "id": entity_id,
                "name": name,
                "type": "Person",
            }))
            .unwrap(),
        )
        .unwrap();

        let encoder = Encoder {
            id: "opencv-sface-2021dec".to_owned(),
            sha256: "test-sface-sha256".to_owned(),
            width: faceprints::WIDTH,
        };
        let mut faces = Vec::new();
        for i in 0..3 {
            let mut emb = unit_vec(dim);
            emb[20 + i] = 0.42;
            let norm = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            for x in &mut emb {
                *x /= norm;
            }
            let day = format!("202611{:02}", 10 + i);
            let seg_dir = journal
                .join("chronicle")
                .join(&day)
                .join("screen")
                .join("100000_300");
            fs::create_dir_all(&seg_dir).unwrap();
            let prefix = faceprints::accumulation::source_prefix("screen.webm");
            let crops_dir = seg_dir.join(format!("{prefix}-crops"));
            fs::create_dir_all(&crops_dir).unwrap();
            let crop_rel = format!("{prefix}-crops/0-{i}.jpg");
            fs::write(seg_dir.join(&crop_rel), distinct_jpeg(40 + i as u8)).unwrap();
            faces.push(Face {
                metadata: Metadata {
                    schema_version: 1,
                    key: FaceKey {
                        day: day.clone(),
                        stream: "screen".to_owned(),
                        stream_layout: "named".to_owned(),
                        segment_key: "100000_300".to_owned(),
                        source_name: "screen.webm".to_owned(),
                        frame_id: i as u64,
                        face_idx: 0,
                    },
                    activities: std::collections::BTreeSet::from([format!(
                        "[\"work\",\"{day}\",\"act-{i}\"]"
                    )]),
                    source: Source::Anchored,
                    label_text: Some(name.to_owned()),
                    quality: 0.90,
                    det_score: 0.95,
                    face_w: 80.0,
                    category: "meeting".to_owned(),
                    crop: Some(crop_rel),
                    added_at: "2026-11-15T00:00:00Z".to_owned(),
                    rejected: false,
                },
                embedding: emb,
            });
        }
        let mut desired = std::collections::BTreeMap::new();
        desired.insert(entity_id.to_owned(), faces);
        faceprints::reconcile(
            journal,
            "[\"work\",\"20261110\",\"act-0\"]",
            &encoder,
            &desired,
        )
        .unwrap();
        encoder
    }

    #[test]
    fn feature_flag_is_on_by_default_and_honors_explicit_false() {
        let dir = tempfile::tempdir().unwrap();
        assert!(photos_library_enabled(dir.path()));
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(dir.path().join("config/journal.json"), r#"{}"#).unwrap();
        assert!(photos_library_enabled(dir.path()));
        fs::write(
            dir.path().join("config/journal.json"),
            r#"{"photos_library":{"enabled":false}}"#,
        )
        .unwrap();
        assert!(!photos_library_enabled(dir.path()));
        fs::write(
            dir.path().join("config/journal.json"),
            r#"{"photos_library":{"enabled":"true"}}"#,
        )
        .unwrap();
        assert!(!photos_library_enabled(dir.path()));
        fs::write(
            dir.path().join("config/journal.json"),
            r#"{"photos_library":{"enabled":true}}"#,
        )
        .unwrap();
        assert!(photos_library_enabled(dir.path()));
    }

    #[test]
    fn smart_album_contract_fails_loudly_on_subtype_drift() {
        let mut bad_catalog = std_catalog();
        bad_catalog.retain(|c| c.subtype != SMART_CAPTURED_BY_ME);
        let rows = vec![make_row("p1", "2026-11-19T12:00:00Z", &[SMART_RECENTS])];
        let err = validate_smart_album_contract(&rows, Some(&bad_catalog)).unwrap_err();
        assert!(err.0.contains("1000000220"));

        // Title drift under wrong subtype number also fails loudly.
        let mut drifted = make_row("p2", "2026-11-19T12:01:00Z", &[]);
        drifted.albums.push(AssetAlbumRef {
            id: "sa-999".to_owned(),
            kind: "smart".to_owned(),
            subtype: 1_000_000_999,
            title: Some("Captured by Me".to_owned()),
        });
        assert!(validate_smart_album_contract(&[drifted], None).is_err());
    }

    #[test]
    fn rules_v2_owner_marks_hidden_collapse_moments_and_others_rung() {
        let mut rows = Vec::new();
        // Hidden photo with every owner mark MUST stay out.
        let mut hidden = make_row(
            "hidden-1",
            "2026-11-19T10:00:00Z",
            &[SMART_CAPTURED_BY_ME, SMART_HIDDEN],
        );
        hidden.hidden = true;
        hidden.favorite = true;
        hidden.edited = true;
        rows.push(hidden);

        // Screenshot stays out even if favorited.
        let mut ss = make_row(
            "ss-1",
            "2026-11-19T10:01:00Z",
            &[SMART_CAPTURED_BY_ME, SMART_SCREENSHOTS],
        );
        ss.screenshot = true;
        ss.favorite = true;
        rows.push(ss);

        // Video stays out.
        let mut vid = make_row("vid-1", "2026-11-19T10:02:00Z", &[SMART_CAPTURED_BY_ME]);
        vid.media_type = "video".to_owned();
        rows.push(vid);

        // Close shots within 10s (both edited-only): v2 collapses edited-only inside <=10s!
        let mut c1 = make_row("c1", "2026-11-19T12:00:00Z", &[SMART_CAPTURED_BY_ME]);
        c1.edited = true;
        let mut c2 = make_row("c2", "2026-11-19T12:00:04Z", &[SMART_CAPTURED_BY_ME]);
        c2.edited = true;
        rows.push(c1);
        rows.push(c2);

        // Saved photo with regular user album beats Rule 1.
        let mut saved_marked = make_row(
            "saved-marked",
            "2026-11-19T12:15:00Z",
            &[SMART_RECENTLY_SAVED],
        );
        saved_marked.albums.push(AssetAlbumRef {
            id: "user-trip".to_owned(),
            kind: "user".to_owned(),
            subtype: ALBUM_REGULAR,
            title: Some("Trip".to_owned()),
        });
        rows.push(saved_marked);

        // Unmarked saved photo 30 min after own moment -> StageOthersFaceCheck!
        let saved_near = make_row(
            "saved-near",
            "2026-11-19T12:30:00Z",
            &[SMART_RECENTLY_SAVED],
        );
        rows.push(saved_near);

        // Unmarked saved photo 10 hours after own moment -> LeftOut (never staged for faces!).
        let saved_far = make_row("saved-far", "2026-11-19T22:30:00Z", &[SMART_RECENTLY_SAVED]);
        rows.push(saved_far);

        // Unmarked cloud-shared photo 10 min after own moment -> StageOthersFaceCheck!
        let mut shared_near = make_row("shared-near", "2026-11-19T12:10:00Z", &[SMART_RECENTS]);
        shared_near.albums.push(AssetAlbumRef {
            id: "shared-1".to_owned(),
            kind: "user".to_owned(),
            subtype: ALBUM_CLOUD_SHARED,
            title: Some("Family".to_owned()),
        });
        rows.push(shared_near);

        let out = run_selection(
            &rows,
            &SelectionConfig::v2(),
            &OwnerCorrections::default(),
            Some(&std_catalog()),
            GeoResolver::shared(),
        )
        .unwrap();

        let by_id: std::collections::BTreeMap<&str, &crate::types::AssetDecision> =
            out.decisions.iter().map(|d| (d.id.as_str(), d)).collect();
        assert_eq!(by_id["hidden-1"].decision, DecisionKind::LeftOut);
        assert_eq!(by_id["hidden-1"].reason, "hidden");
        // Hidden assets must NEVER appear in left_out list.
        assert!(!out.left_out.iter().any(|l| l.id == "hidden-1"));
        assert_eq!(by_id["ss-1"].decision, DecisionKind::LeftOut);
        assert_eq!(by_id["vid-1"].decision, DecisionKind::LeftOut);

        // Edited-only close pair collapsed to 1 kept + 1 collapsed.
        let c_kept = [by_id["c1"].decision, by_id["c2"].decision]
            .iter()
            .filter(|d| **d == DecisionKind::Keep)
            .count();
        assert_eq!(c_kept, 1);
        assert_eq!(by_id["saved-marked"].decision, DecisionKind::Keep);
        assert!(by_id["saved-marked"].admitted_via_owner_mark);
        assert_eq!(
            by_id["saved-near"].decision,
            DecisionKind::StageOthersFaceCheck
        );
        assert_eq!(
            by_id["shared-near"].decision,
            DecisionKind::StageOthersFaceCheck
        );
        assert_eq!(by_id["saved-far"].decision, DecisionKind::LeftOut);
    }

    #[test]
    fn full_lifecycle_sqlite_rebuild_two_devices_others_face_noise_trash_and_turn_off_suppression()
    {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();

        // Seed Alice meeting faceprint + enable faces.enabled (keep photo_match_enabled OFF).
        let encoder = seed_meeting_person(journal, "ent-alice", "Alice", 3);
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            r#"{"photos_library":{"enabled":true},"faces":{"enabled":true}}"#,
        )
        .unwrap();

        // 1. Scan metadata only (include a hidden asset to verify server-side refusal).
        scan_start(
            journal,
            "dev-iphone-1",
            "Test iPhone",
            "iphone",
            "authorized",
            5,
            std_catalog(),
        )
        .unwrap();
        let mut p1 = make_row(
            "asset-own-1",
            "2026-11-19T18:00:00Z",
            &[SMART_CAPTURED_BY_ME],
        );
        p1.cloud_id = Some("cloud-own-1".to_owned());
        p1.location = Some(AssetLocation {
            latitude: 40.0150,
            longitude: -105.2705,
            horizontal_accuracy_m: Some(12.0),
            altitude: None,
        });
        p1.extended = Some(AssetExtended {
            caption: Some("Afternoon walk".to_owned()),
            original_filename: Some("IMG_1001.HEIC".to_owned()),
            ..AssetExtended::default()
        });
        let p_noise = make_row(
            "asset-noise-1",
            "2026-11-19T18:10:00Z",
            &[SMART_CAPTURED_BY_ME],
        );
        let p_gone = make_row(
            "asset-gone-1",
            "2026-11-19T18:20:00Z",
            &[SMART_CAPTURED_BY_ME],
        );
        let p_others_yes = make_row(
            "asset-others-yes",
            "2026-11-19T18:30:00Z",
            &[SMART_RECENTLY_SAVED],
        );
        let p_others_no = make_row(
            "asset-others-no",
            "2026-11-19T18:35:00Z",
            &[SMART_RECENTLY_SAVED],
        );
        let mut p_hidden = make_row(
            "asset-hidden",
            "2026-11-19T18:05:00Z",
            &[SMART_CAPTURED_BY_ME, SMART_HIDDEN],
        );
        p_hidden.hidden = true;

        scan_batch(
            journal,
            "dev-iphone-1",
            0,
            vec![p1, p_noise, p_gone, p_others_yes, p_others_no, p_hidden],
            Some(5),
        )
        .unwrap();
        let summary_before = scan_finish(
            journal,
            "dev-iphone-1",
            Some(5),
            None,
            Some("authorized"),
            Some(0),
        )
        .unwrap();
        // Hidden asset was dropped before storage.
        assert_eq!(summary_before.total_assets, 5);
        assert_eq!(summary_before.kept, 3);
        assert_eq!(summary_before.staged_others_face_check, 2);

        // Holding area files are NEVER indexed or offered to agents.
        assert_eq!(
            classify("config/photos-library/selection/decisions.json"),
            ContentResolution::Unrecognized
        );

        // 2. Delete SQLite cache and rebuild -> identical decisions_sha256!
        let db = sqlite_cache_path(journal);
        assert!(db.is_file());
        fs::remove_file(&db).unwrap();
        assert!(!db.is_file());
        let rebuilt = rebuild_from_plain_json(journal).unwrap();
        assert_eq!(
            rebuilt.summary.decisions_sha256,
            summary_before.decisions_sha256
        );
        assert!(db.is_file());

        // 3. Before owner chooses range: ZERO wanted items!
        let state_before = dashboard_state(journal).unwrap();
        assert_eq!(state_before.state, "setting_up");
        assert_eq!(state_before.substate, "choose");
        assert!(
            lease_wanted(journal, "dev-iphone-1", 20, None)
                .unwrap()
                .wanted
                .is_empty()
        );

        // 4. Owner chooses "all" -> wanted list publishes newest-first & leases per device!
        choose_range(journal, &RangeSpec::Preset("all".to_owned())).unwrap();
        let leased_1 = lease_wanted(journal, "dev-iphone-1", 2, Some(300)).unwrap();
        assert_eq!(leased_1.wanted.len(), 2);
        // Second device does NOT get items actively leased by device 1.
        let leased_2 = lease_wanted(journal, "dev-ipad-2", 10, Some(300)).unwrap();
        for item in &leased_2.wanted {
            assert!(!leased_1.wanted.iter().any(|a| a.asset_id == item.asset_id));
        }

        // 5. Deliver asset-own-1 -> lands in syncing `photos` stream with GPS sighting & observation!
        let wire = SeqWire::new(vec![
            Ok(generated(
                r#"{"description":"Trees along a sunny creek path.","visible_text":"TRAIL 4","belonging_class":"lived_moment"}"#,
            )),
            Ok(generated(
                r#"{"description":"Blurry dark inside of a pocket.","visible_text":null,"belonging_class":"noise_or_accidental"}"#,
            )),
            Ok(generated(
                r#"{"description":"Alice smiling at an outdoor cafe.","visible_text":null,"belonging_class":"lived_moment"}"#,
            )),
        ]);
        let no_face_detector = StubDetector {
            encoder: encoder.clone(),
            faces: vec![],
        };
        let alice_detector = StubDetector {
            encoder: encoder.clone(),
            faces: vec![photo_match::PhotoFaceCandidate {
                face_idx: 0,
                bbox: [100.0, 90.0, 88.0, 88.0],
                det_score: 0.96,
                sharpness: 350.0,
                quality: 0.90,
                embedding: unit_vec(3),
                crop_jpeg: Some(distinct_jpeg(77)),
            }],
        };

        let out1 = ingest_photo_copy_with(
            journal,
            "dev-iphone-1",
            "asset-own-1",
            Some("cloud-own-1"),
            &distinct_jpeg(1),
            None,
            &wire,
            &no_face_detector,
            &NativePublicationOperations,
        )
        .unwrap();
        let (day1, stream1, seg1) = match out1 {
            CopyIngestOutcome::Landed {
                day,
                stream,
                segment,
            } => (day, stream, segment),
            other => panic!("expected Landed, got {other:?}"),
        };
        assert_eq!(stream1, PHOTOS_SOURCE);
        assert!(is_syncing_source_stream(journal, &stream1));
        let seg1_dir = journal
            .join("chronicle")
            .join(&day1)
            .join(&stream1)
            .join(&seg1);
        assert!(seg1_dir.join("location.jsonl").is_file());
        let obs1 = read_photo_observation(&seg1_dir).unwrap();
        assert_eq!(obs1.state, PhotoObservationState::Ready);
        assert_eq!(obs1.time.time_source, "capture");
        assert_eq!(obs1.belonging_class, Some(BelongingClass::LivedMoment));
        assert_eq!(
            obs1.owner_caption.as_ref().map(|c| c.text.as_str()),
            Some("Afternoon walk")
        );
        assert_eq!(
            obs1.visible_text.as_ref().map(|v| v.text.as_str()),
            Some("TRAIL 4")
        );
        assert!(matches!(
            resolve_photo_observation(journal, &obs1.reference()),
            PhotoObservationResolution::Resolved(_)
        ));

        // Second device sending the same library photo via cloud_id produces NO second copy!
        let dup = ingest_photo_copy_with(
            journal,
            "dev-ipad-2",
            "different-ipad-local-id",
            Some("cloud-own-1"),
            &distinct_jpeg(2),
            None,
            &wire,
            &no_face_detector,
            &NativePublicationOperations,
        )
        .unwrap();
        assert!(matches!(dup, CopyIngestOutcome::AlreadyReceived { .. }));

        // 6. Noise copy is NOT kept and joins left-out noise counts!
        let noise_out = ingest_photo_copy_with(
            journal,
            "dev-iphone-1",
            "asset-noise-1",
            None,
            &distinct_jpeg(3),
            None,
            &wire,
            &no_face_detector,
            &NativePublicationOperations,
        )
        .unwrap();
        assert_eq!(
            noise_out,
            CopyIngestOutcome::LeftOut {
                reason: "noise".to_owned()
            }
        );

        // 7. Others'-photos rung: no face match -> discarded without chronicle write;
        //    calibrated Alice face match -> kept!
        let others_no_out = ingest_photo_copy_with(
            journal,
            "dev-iphone-1",
            "asset-others-no",
            None,
            &distinct_jpeg(4),
            None,
            &wire,
            &no_face_detector,
            &NativePublicationOperations,
        )
        .unwrap();
        assert_eq!(
            others_no_out,
            CopyIngestOutcome::LeftOut {
                reason: "others_no_face_match".to_owned()
            }
        );
        let others_yes_out = ingest_photo_copy_with(
            journal,
            "dev-iphone-1",
            "asset-others-yes",
            None,
            &distinct_jpeg(5),
            None,
            &wire,
            &alice_detector,
            &NativePublicationOperations,
        )
        .unwrap();
        assert!(matches!(others_yes_out, CopyIngestOutcome::Landed { .. }));

        // 8. Removed asset reported "gone" leaves wanted list cleanly.
        mark_wanted_non_copy(journal, "dev-iphone-1", "asset-gone-1", "gone").unwrap();
        assert!(
            lease_wanted(journal, "dev-iphone-1", 20, None)
                .unwrap()
                .wanted
                .is_empty()
        );

        // 9. List landed photos & trash asset-own-1 via chosen-set trash.
        let page = list_landed_photos(journal, None, None, None).unwrap();
        assert_eq!(page.items.len(), 2);
        let trashed = trash_chosen_photos(
            journal,
            &[TrashSegmentRef {
                day: day1.clone(),
                stream: stream1.clone(),
                segment: seg1.clone(),
            }],
            None,
        )
        .unwrap();
        assert_eq!(trashed.moved, 1);
        assert_eq!(
            resolve_photo_observation(journal, &obs1.reference()),
            PhotoObservationResolution::NotFound
        );

        // 10. Turn off photos -> holding area completely deleted, landed photo #2 stays!
        let off_state = turn_off(journal).unwrap();
        assert!(!holding_root(journal).exists());
        assert_eq!(off_state.state, "off");
        assert_eq!(off_state.progress.in_journal, 1);

        // 11. Turn back on and rescan the EXACT same library -> trashed asset-own-1
        //     is NEVER asked for again!
        scan_start(
            journal,
            "dev-iphone-1",
            "Test iPhone",
            "iphone",
            "authorized",
            2,
            std_catalog(),
        )
        .unwrap();
        let mut p1_again = make_row(
            "asset-own-1",
            "2026-11-19T18:00:00Z",
            &[SMART_CAPTURED_BY_ME],
        );
        p1_again.cloud_id = Some("cloud-own-1".to_owned());
        let p_brand_new = make_row(
            "asset-brand-new",
            "2026-11-20T12:00:00Z",
            &[SMART_CAPTURED_BY_ME],
        );
        scan_batch(
            journal,
            "dev-iphone-1",
            0,
            vec![p1_again, p_brand_new],
            Some(2),
        )
        .unwrap();
        scan_finish(
            journal,
            "dev-iphone-1",
            Some(2),
            None,
            Some("authorized"),
            Some(0),
        )
        .unwrap();
        choose_range(journal, &RangeSpec::Preset("all".to_owned())).unwrap();
        let after_rescan = lease_wanted(journal, "dev-iphone-1", 20, None).unwrap();
        assert_eq!(after_rescan.wanted.len(), 1);
        assert_eq!(after_rescan.wanted[0].asset_id, "asset-brand-new");
    }

    #[test]
    fn turn_off_before_any_photo_lands_persists_off_state_and_blocks_changes_until_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();

        scan_start(
            journal,
            "dev-iphone-1",
            "Test Phone",
            "iphone",
            "authorized",
            1,
            std_catalog(),
        )
        .unwrap();
        let p1 = make_row(
            "asset-own-1",
            "2026-11-19T18:00:00Z",
            &[SMART_CAPTURED_BY_ME],
        );
        scan_batch(journal, "dev-iphone-1", 0, vec![p1], Some(1)).unwrap();
        scan_finish(
            journal,
            "dev-iphone-1",
            Some(1),
            None,
            Some("authorized"),
            Some(0),
        )
        .unwrap();

        // Turn off directly from the choose screen (zero photos landed, no received/ dir).
        let off = turn_off(journal).unwrap();
        assert!(!holding_root(journal).exists());
        assert_eq!(off.state, "off");
        assert_eq!(off.substate, "off");
        assert_eq!(off.progress.in_journal, 0);

        // Incremental scan_changes cannot resurrect the holding area while turned off.
        assert!(scan_changes(journal, "dev-iphone-1", vec![], vec![], None, None, None).is_err());
        assert_eq!(dashboard_state(journal).unwrap().state, "off");

        // Re-enabling via an explicit scan_start clears the off marker cleanly.
        scan_start(
            journal,
            "dev-iphone-1",
            "Test Phone",
            "iphone",
            "authorized",
            1,
            std_catalog(),
        )
        .unwrap();
        assert_eq!(dashboard_state(journal).unwrap().state, "setting_up");
    }

    #[test]
    fn others_face_check_respects_photo_match_enabled_and_counts_pending_as_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();
        let _encoder = seed_meeting_person(journal, "ent-alice", "Alice", 3);

        // With faces.enabled=true and photo_match_enabled=false, selection face queries stay OFF.
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            r#"{"faces":{"enabled":true,"photo_match_enabled":false}}"#,
        )
        .unwrap();
        assert!(!can_run_others_face_check(journal));

        scan_start(
            journal,
            "dev-iphone-1",
            "Test Phone",
            "iphone",
            "authorized",
            2,
            std_catalog(),
        )
        .unwrap();
        let own = make_row(
            "asset-own-1",
            "2026-11-19T18:00:00Z",
            &[SMART_CAPTURED_BY_ME],
        );
        let other_pending = make_row(
            "asset-others-pending",
            "2026-11-19T18:20:00Z",
            &[SMART_RECENTLY_SAVED],
        );
        scan_batch(
            journal,
            "dev-iphone-1",
            0,
            vec![own, other_pending],
            Some(2),
        )
        .unwrap();
        scan_finish(
            journal,
            "dev-iphone-1",
            Some(2),
            None,
            Some("authorized"),
            Some(0),
        )
        .unwrap();
        choose_range(journal, &RangeSpec::Preset("all".to_owned())).unwrap();

        // While photo_match_enabled is false, only asset-own-1 is leased (NOT StageOthersFaceCheck).
        let wanted_off = lease_wanted(journal, "dev-iphone-1", 10, Some(300)).unwrap();
        assert_eq!(wanted_off.wanted.len(), 1);
        assert_eq!(wanted_off.wanted[0].asset_id, "asset-own-1");
        let st_off = dashboard_state(journal).unwrap();
        assert!(!st_off.profile_pics_enabled);
        assert_eq!(st_off.progress.waiting_on_phone, 1);
        assert_eq!(
            st_off.left_out.reasons.get("saved_and_shared").copied(),
            Some(1)
        );

        // Turn photo_match_enabled ON (default when omitted): StageOthersFaceCheck is leased AND
        // counts toward waiting_on_phone rather than left_out saved_and_shared / others_no_face_match.
        fs::write(journal.join("config/journal.json"), r#"{}"#).unwrap();
        assert!(can_run_others_face_check(journal));
        let wanted_on = lease_wanted(journal, "dev-iphone-1", 10, Some(300)).unwrap();
        assert!(
            wanted_on
                .wanted
                .iter()
                .any(|w| w.asset_id == "asset-others-pending"
                    && w.reason_kind == "others_face_check")
        );
        let st_pending = dashboard_state(journal).unwrap();
        assert!(st_pending.profile_pics_enabled);
        assert_eq!(st_pending.progress.chosen_total, 2);
        assert_eq!(st_pending.progress.waiting_on_phone, 2);
        assert_eq!(st_pending.left_out.total, 0);
        assert!(!st_pending.left_out.reasons.contains_key("saved_and_shared"));
    }

    #[test]
    fn device_connected_reflects_freshness_window_live_seen_and_unpairing() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();

        scan_start(
            journal,
            "sha256:dev-1",
            "Test Phone",
            "iphone",
            "authorized",
            1,
            std_catalog(),
        )
        .unwrap();

        let now = jiff::Timestamp::now();
        let fresh = dashboard_state_at(journal, None, now).unwrap();
        assert_eq!(fresh.devices.len(), 1);
        assert!(fresh.devices[0].connected);

        // After >= DEVICE_CONNECTED_WINDOW_SECS with no report, connected becomes false.
        let later = now
            .checked_add(jiff::SignedDuration::from_secs(
                DEVICE_CONNECTED_WINDOW_SECS + 5,
            ))
            .unwrap();
        let stale = dashboard_state_at(journal, None, later).unwrap();
        assert!(!stale.devices[0].connected);

        // Fresh in-process live_seen keeps connected true even if no photos route fired recently.
        let mut live = std::collections::BTreeMap::new();
        live.insert(
            "sha256:dev-1".to_owned(),
            solstone_core_time::utc_rfc3339(later),
        );
        let live_state = dashboard_state_at(journal, Some(&live), later).unwrap();
        assert!(live_state.devices[0].connected);

        // Unpairing (absent from link/authorized_clients.json) forces connected false immediately.
        fs::create_dir_all(journal.join("link")).unwrap();
        fs::write(journal.join("link/authorized_clients.json"), b"[]").unwrap();
        let unpaired = dashboard_state_at(journal, Some(&live), later).unwrap();
        assert!(!unpaired.devices[0].connected);
    }

    #[test]
    fn row_visibility_requires_capable_ios_device_or_existing_photos_state() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();

        // Case (a): fresh journal with no paired devices -> enabled, NOT visible.
        assert!(photos_library_enabled(journal));
        assert!(!photos_library_visible(journal));

        // Case (b): paired old iOS app (in authorized_clients.json, never checked in) -> NOT visible.
        fs::create_dir_all(journal.join("link")).unwrap();
        fs::write(
            journal.join("link/authorized_clients.json"),
            r#"[{"kind":"cert","fingerprint":"sha256:old-iphone","device_label":"Old Phone","paired_at":"2026-11-01T00:00:00Z","instance_id":"old-1"}]"#,
        )
        .unwrap();
        assert!(!photos_library_visible(journal));

        // Non-iOS device kinds (mac/android) do not unlock phase-1 iOS/iPadOS visibility.
        update_device_status(
            journal,
            "sha256:old-iphone",
            Some("Test Mac"),
            Some("mac"),
            Some("not_determined"),
            Some(0),
            Some(false),
            Some(false),
        )
        .unwrap();
        assert!(!photos_library_visible(journal));
        assert!(!holding_root(journal).exists());

        // Capable iPad checks in with source_enabled=false -> visible, holding dir NOT created!
        fs::write(
            journal.join("link/authorized_clients.json"),
            r#"[
                {"kind":"cert","fingerprint":"sha256:old-iphone","device_label":"Old Phone","paired_at":"2026-11-01T00:00:00Z","instance_id":"old-1"},
                {"kind":"cert","fingerprint":"sha256:new-ipad","device_label":"New iPad","paired_at":"2026-11-25T00:00:00Z","instance_id":"new-1"}
            ]"#,
        )
        .unwrap();
        update_device_status(
            journal,
            "sha256:new-ipad",
            Some("New iPad"),
            Some("ipad"),
            Some("denied"),
            Some(0),
            Some(false),
            Some(false),
        )
        .unwrap();
        assert!(photos_library_visible(journal));
        assert!(!holding_root(journal).exists());
        let st = dashboard_state(journal).unwrap();
        assert_eq!(st.state, "ready_to_set_up");
        assert!(st.attention_reason.is_none());
        assert_eq!(st.capable_device_kinds, vec!["ipad".to_owned()]);

        // Unpairing the capable device hides the row again when no photos state exists.
        remove_capable_device(journal, "sha256:new-ipad").unwrap();
        assert!(!photos_library_visible(journal));
    }

    #[test]
    fn large_synthetic_40k_library_choose_all_state_wanted_copy_and_noop_delta_stay_fast() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();

        // Build a purely synthetic 40,000-asset library matching real large-library
        // populations: ~24k eligible own stills, ~14k kept all-time (~1,700 in last
        // year), ~3,800 others' photos corroborated near own moments, ~12.2k left out.
        let now = jiff::Timestamp::from_second(1_795_100_000).unwrap(); // 2026-11-19T14:53:20Z
        let catalog = vec![
            SmartAlbumCatalogEntry {
                id: "sa-cbm".to_owned(),
                subtype: SMART_CAPTURED_BY_ME,
                title: Some("Captured by Me".to_owned()),
                count: 24_000,
            },
            SmartAlbumCatalogEntry {
                id: "sa-saved".to_owned(),
                subtype: SMART_RECENTLY_SAVED,
                title: Some("Recently Saved".to_owned()),
                count: 6_800,
            },
            SmartAlbumCatalogEntry {
                id: "sa-recents".to_owned(),
                subtype: SMART_RECENTS,
                title: Some("Recents".to_owned()),
                count: 40_000,
            },
        ];

        scan_start(
            journal,
            "sha256:synthetic-scale-device",
            "Scale Phone",
            "iphone",
            "authorized",
            40_000,
            catalog.clone(),
        )
        .unwrap();

        let mut rows: Vec<ScanAssetRow> = Vec::with_capacity(40_000);
        let mut anchor_ts: Vec<i64> = Vec::with_capacity(14_000);

        // 1,700 own anchors across the last year (spaced 5 hours apart) +
        // 12,300 earlier own anchors across prior years (spaced 9 hours apart).
        for i in 0..14_000usize {
            let offset_secs = if i < 1_700 {
                (i as i64 + 1) * 5 * 3600
            } else {
                1_700 * 5 * 3600 + ((i - 1_700 + 1) as i64) * 9 * 3600
            };
            let ts = now.as_second() - offset_secs;
            anchor_ts.push(ts);
            let iso = solstone_core_time::utc_rfc3339(jiff::Timestamp::from_second(ts).unwrap());
            let mut row = make_row(&format!("syn-own-{i:05}"), &iso, &[SMART_CAPTURED_BY_ME]);
            row.cloud_id = Some(format!("syn-cloud-own-{i:05}"));
            if i == 0 || i == 1 {
                row.location = Some(AssetLocation {
                    latitude: 40.015,
                    longitude: -105.2705,
                    horizontal_accuracy_m: Some(8.0),
                    altitude: None,
                });
                row.extended = Some(AssetExtended {
                    caption: Some(format!("Synthetic anchor {i}")),
                    original_filename: Some(format!("SYN_{i:04}.JPG")),
                    camera: Some("SyntheticCam".to_owned()),
                    ..AssetExtended::default()
                });
            }
            rows.push(row);
        }

        // 10,000 near-duplicate own stills (+3 s from first 10k anchors) -> collapsed.
        for (i, &base_ts) in anchor_ts.iter().take(10_000).enumerate() {
            let iso =
                solstone_core_time::utc_rfc3339(jiff::Timestamp::from_second(base_ts + 3).unwrap());
            rows.push(make_row(
                &format!("syn-dup-{i:05}"),
                &iso,
                &[SMART_CAPTURED_BY_ME],
            ));
        }

        // 3,800 others' photos corroborated +20 min from own anchors -> StageOthersFaceCheck.
        for (i, &base_ts) in anchor_ts.iter().take(3_800).enumerate() {
            let iso = solstone_core_time::utc_rfc3339(
                jiff::Timestamp::from_second(base_ts + 1_200).unwrap(),
            );
            let mut row = make_row(&format!("syn-oth-{i:05}"), &iso, &[SMART_RECENTLY_SAVED]);
            row.cloud_id = Some(format!("syn-cloud-oth-{i:05}"));
            rows.push(row);
        }

        // 6,000 videos + 3,200 screenshots + 3,000 uncorroborated saved photos = 12,200 left out.
        let old_base = jiff::Timestamp::from_second(1_200_000_000)
            .unwrap()
            .as_second(); // 2008
        for i in 0..6_000usize {
            let iso = solstone_core_time::utc_rfc3339(
                jiff::Timestamp::from_second(old_base + (i as i64) * 300).unwrap(),
            );
            let mut row = make_row(&format!("syn-vid-{i:05}"), &iso, &[SMART_CAPTURED_BY_ME]);
            row.media_type = "video".to_owned();
            row.duration = Some(12.0);
            rows.push(row);
        }
        for i in 0..3_200usize {
            let iso = solstone_core_time::utc_rfc3339(
                jiff::Timestamp::from_second(old_base + 2_000_000 + (i as i64) * 300).unwrap(),
            );
            let mut row = make_row(&format!("syn-scr-{i:05}"), &iso, &[SMART_SCREENSHOTS]);
            row.screenshot = true;
            rows.push(row);
        }
        for i in 0..3_000usize {
            let iso = solstone_core_time::utc_rfc3339(
                jiff::Timestamp::from_second(old_base + 4_000_000 + (i as i64) * 300).unwrap(),
            );
            rows.push(make_row(
                &format!("syn-sav-{i:05}"),
                &iso,
                &[SMART_RECENTLY_SAVED],
            ));
        }
        assert_eq!(rows.len(), 40_000);

        let sample_unchanged = rows[0].clone();
        for (idx, chunk) in rows.chunks(1_000).enumerate() {
            scan_batch(
                journal,
                "sha256:synthetic-scale-device",
                idx as u32,
                chunk.to_vec(),
                Some((idx + 1) * 1_000),
            )
            .unwrap();
        }
        let summary = scan_finish(
            journal,
            "sha256:synthetic-scale-device",
            Some(40_000),
            None,
            Some("authorized"),
            Some(0),
        )
        .unwrap();
        assert_eq!(summary.total_assets, 40_000);
        assert_eq!(summary.eligible_own_stills, 24_000);
        assert_eq!(summary.kept, 14_000);
        assert_eq!(summary.staged_others_face_check, 3_800);
        assert_eq!(summary.left_out, 22_200);

        // O(1) badge check stays instant and accurate without reading decisions.json.
        let badge_t0 = std::time::Instant::now();
        assert!(!photos_needs_attention(journal));
        assert!(
            badge_t0.elapsed() < std::time::Duration::from_millis(200),
            "photos_needs_attention took {:?}",
            badge_t0.elapsed()
        );

        // Choose "all" and verify range counts + bringing_in transition stay responsive.
        let choose_t0 = std::time::Instant::now();
        let chosen = choose_range(journal, &RangeSpec::Preset("all".to_owned())).unwrap();
        assert!(
            choose_t0.elapsed() < std::time::Duration::from_secs(2),
            "choose_range(\"all\") at 40k scale took {:?}",
            choose_t0.elapsed()
        );
        assert_eq!(chosen.state, "on");
        assert_eq!(chosen.substate, "bringing_in");
        assert_eq!(chosen.choice.range.as_deref(), Some("all"));
        assert_eq!(chosen.ranges["all"].photos, 14_000);
        assert!(chosen.ranges["1y"].photos >= 1_600 && chosen.ranges["1y"].photos <= 1_800);
        assert_eq!(chosen.progress.chosen_total, 14_000);
        assert_eq!(chosen.progress.waiting_on_phone, 14_000);

        // Repeated dashboard_state polls stay fast (cached decisions + snapshot received ledger).
        let state_t0 = std::time::Instant::now();
        for _ in 0..5 {
            let st = dashboard_state_at(journal, None, now).unwrap();
            assert_eq!(st.progress.waiting_on_phone, 14_000);
        }
        assert!(
            state_t0.elapsed() < std::time::Duration::from_secs(2),
            "5x dashboard_state_at at 40k scale took {:?}",
            state_t0.elapsed()
        );

        // Leasing a wanted batch of 20 stays fast and returns newest-first.
        let lease_t0 = std::time::Instant::now();
        let leased = lease_wanted(journal, "sha256:synthetic-scale-device", 20, Some(300)).unwrap();
        assert!(
            lease_t0.elapsed() < std::time::Duration::from_secs(2),
            "lease_wanted at 40k scale took {:?}",
            lease_t0.elapsed()
        );
        assert_eq!(leased.wanted.len(), 20);
        assert_eq!(leased.wanted[0].asset_id, "syn-own-00000");
        assert_eq!(leased.remaining, 13_980);

        // Deliver photo 0 via O(1) SQLite v2 lookup (verifying caption + GPS preserved).
        let wire = SeqWire::new(vec![
            Ok(generated(
                r#"{"description":"Sunny ridge trail.","visible_text":null,"belonging_class":"lived_moment"}"#,
            )),
            Ok(generated(
                r#"{"description":"Mountain creek.","visible_text":null,"belonging_class":"lived_moment"}"#,
            )),
        ]);
        let encoder = Encoder {
            id: "opencv-sface-2021dec".to_owned(),
            sha256: "test-sface-sha256".to_owned(),
            width: faceprints::WIDTH,
        };
        let no_faces = StubDetector {
            encoder,
            faces: vec![],
        };
        let copy_t0 = std::time::Instant::now();
        let out0 = ingest_photo_copy_with(
            journal,
            "sha256:synthetic-scale-device",
            "syn-own-00000",
            Some("syn-cloud-own-00000"),
            &distinct_jpeg(10),
            None,
            &wire,
            &no_faces,
            &NativePublicationOperations,
        )
        .unwrap();
        assert!(
            copy_t0.elapsed() < std::time::Duration::from_secs(2),
            "ingest_photo_copy via SQLite v2 at 40k scale took {:?}",
            copy_t0.elapsed()
        );
        let (day0, stream0, seg0) = match out0 {
            CopyIngestOutcome::Landed {
                day,
                stream,
                segment,
            } => (day, stream, segment),
            other => panic!("expected Landed, got {other:?}"),
        };
        let seg0_dir = journal
            .join("chronicle")
            .join(&day0)
            .join(&stream0)
            .join(&seg0);
        assert!(seg0_dir.join("location.jsonl").is_file());
        let obs0 = read_photo_observation(&seg0_dir).unwrap();
        assert_eq!(
            obs0.owner_caption.as_ref().map(|c| c.text.as_str()),
            Some("Synthetic anchor 0")
        );

        // Delete SQLite cache (simulating pre-v2 upgrade or cache deletion): ingest falls back
        // to plain JSON once, rebuilds SQLite v2, and lands photo 1 with full metadata intact.
        let db_path = sqlite_cache_path(journal);
        fs::remove_file(&db_path).unwrap();
        assert!(!db_path.is_file());
        let out1 = ingest_photo_copy_with(
            journal,
            "sha256:synthetic-scale-device",
            "syn-own-00001",
            Some("syn-cloud-own-00001"),
            &distinct_jpeg(11),
            None,
            &wire,
            &no_faces,
            &NativePublicationOperations,
        )
        .unwrap();
        assert!(matches!(out1, CopyIngestOutcome::Landed { .. }));
        assert!(
            db_path.is_file(),
            "SQLite v2 cache should auto-rebuild on fallback"
        );

        // No-op scan_changes (same row + same smart albums) skips full 40k rewrite & rebuild!
        let dec_path = holding_root(journal).join("selection/decisions.json");
        let dec_mtime_before = fs::metadata(&dec_path).unwrap().modified().unwrap();
        let noop_summary = scan_changes(
            journal,
            "sha256:synthetic-scale-device",
            vec![sample_unchanged],
            vec![],
            Some(catalog),
            Some("authorized"),
            Some(0),
        )
        .unwrap();
        assert_eq!(noop_summary.decisions_sha256, summary.decisions_sha256);
        assert_eq!(
            fs::metadata(&dec_path).unwrap().modified().unwrap(),
            dec_mtime_before,
            "no-op scan_changes must not rewrite decisions.json"
        );

        // Verify post-ingest dashboard progress reflects the 2 landed photos.
        let after = dashboard_state_at(journal, None, now).unwrap();
        assert_eq!(after.progress.arrived, 2);
        assert_eq!(after.progress.in_journal, 2);
        assert_eq!(after.progress.waiting_on_phone, 13_998);

        // Parity check: deleting SQLite and rebuilding from plain JSON yields identical digest.
        fs::remove_file(&db_path).unwrap();
        let rebuilt = rebuild_from_plain_json(journal).unwrap();
        assert_eq!(rebuilt.summary.decisions_sha256, summary.decisions_sha256);

        // Fail-closed check: corrupting a matched receipt in config/photos/received/ must fail lease_wanted.
        let corrupt_hash = received::id_hash("asset", "syn-own-00000");
        let corrupt_path = journal
            .join("config/photos/received")
            .join(&corrupt_hash[..2])
            .join(format!("{corrupt_hash}.json"));
        fs::write(&corrupt_path, b"{not valid json").unwrap();
        let lease_err = lease_wanted(journal, "sha256:synthetic-scale-device", 20, Some(300))
            .expect_err("corrupt received receipt must fail lease_wanted closed");
        assert!(lease_err.contains("photos received ledger"));
    }
}
