// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use solstone_core_import::{
    AutoTimestamp, DetectedTimestamp, ManifestSummary, RegistrySource, ResolutionError,
    ResolutionOptions, ResolutionOutcome, ResolutionSeams, ResolvedSource, SkipReason, SourceHash,
    validate_timestamp,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-w1b-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, b"contents").unwrap();
        path
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn opt(media: &Path) -> ResolutionOptions<'_> {
    ResolutionOptions {
        media,
        source: None,
        timestamp: Some("20260311_120000"),
        auto: AutoTimestamp::Absent,
        dry_run: true,
        deterministic_only: false,
        force: false,
    }
}
fn answer() -> DetectedTimestamp {
    DetectedTimestamp::new(validate_timestamp("20260311_120000").unwrap())
}
fn no_apple(_: &Path) -> Result<bool, ()> {
    Ok(false)
}
fn yes_apple(_: &Path) -> Result<bool, ()> {
    Ok(true)
}
fn no_claim(_: RegistrySource, _: &Path) -> Result<bool, ()> {
    Ok(false)
}
fn document(source: RegistrySource, _: &Path) -> Result<bool, ()> {
    Ok(source == RegistrySource::Document)
}
fn corpus_extension_claim(source: RegistrySource, path: &Path) -> Result<bool, ()> {
    Ok(matches!(
        (
            source,
            path.extension().and_then(|extension| extension.to_str())
        ),
        (RegistrySource::Ics, Some("ics"))
            | (RegistrySource::Document, Some("pdf"))
            | (RegistrySource::Image, Some("png"))
    ))
}
fn failed_claim(_: RegistrySource, _: &Path) -> Result<bool, ()> {
    Err(())
}
fn deterministic(_: &Path, _: Option<&str>) -> Option<DetectedTimestamp> {
    Some(answer())
}
fn undetected(_: &Path, _: Option<&str>) -> Option<DetectedTimestamp> {
    None
}
fn no_model(_: &Path, _: Option<&str>) -> Result<Option<DetectedTimestamp>, ()> {
    Ok(None)
}
fn lookup_none(_: &SourceHash) -> Option<ManifestSummary> {
    None
}
type TestSeams = ResolutionSeams<
    fn(&Path) -> Result<bool, ()>,
    fn(RegistrySource, &Path) -> Result<bool, ()>,
    fn(&Path, Option<&str>) -> Option<DetectedTimestamp>,
    fn(&Path, Option<&str>) -> Result<Option<DetectedTimestamp>, ()>,
    fn(&SourceHash) -> Option<ManifestSummary>,
>;

fn seams() -> TestSeams {
    ResolutionSeams {
        apple_detector: no_apple,
        claims: no_claim,
        deterministic_detector: deterministic,
        model_detector: no_model,
        manifest_lookup: lookup_none,
    }
}

#[test]
fn missing_path_precedes_every_seam() {
    // corpus missing_path
    let tree = Tree::new();
    let mut seam = seams();
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&opt(&tree.0.join("missing")), &mut seam),
        Err(ResolutionError::MissingPath)
    ));
}

#[test]
fn ac2_source_presence_gates_apple_preempt() {
    // constructed Apple export plus top-level PDF tree
    let tree = Tree::new();
    tree.file("top.pdf");
    let mut seam = seams();
    seam.apple_detector = yes_apple;
    assert_eq!(
        solstone_core_import::detect::resolve_import(&opt(&tree.0), &mut seam).unwrap(),
        ResolutionOutcome::RouteAppleHealth
    );
    for source in ["recording", "nosuch", "plaud"] {
        let mut options = opt(&tree.0);
        options.source = Some(source);
        let mut no = seams();
        no.claims = document;
        assert!(matches!(
            solstone_core_import::detect::resolve_import(&options, &mut no),
            Ok(ResolutionOutcome::Resolved {
                source: ResolvedSource::Registry(RegistrySource::Document),
                ..
            })
        ));
    }
    let mut forced = opt(&tree.0);
    forced.source = Some("ics");
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&forced, &mut seam),
        Ok(ResolutionOutcome::Resolved {
            source: ResolvedSource::Registry(RegistrySource::Ics),
            ..
        })
    ));
}

#[test]
fn ac2a_apple_detector_seam_expresses_yes_no_and_error() {
    // constructed Apple export directory
    let tree = Tree::new();
    let mut yes = seams();
    yes.apple_detector = yes_apple;
    assert_eq!(
        solstone_core_import::detect::resolve_import(&opt(&tree.0), &mut yes).unwrap(),
        ResolutionOutcome::RouteAppleHealth
    );
    let mut no = seams();
    no.claims = document;
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&opt(&tree.0), &mut no),
        Ok(ResolutionOutcome::Resolved { .. })
    ));
    let mut error = ResolutionSeams {
        apple_detector: |_: &Path| Err::<bool, _>("no answer"),
        claims: no_claim,
        deterministic_detector: deterministic,
        model_detector: no_model,
        manifest_lookup: lookup_none,
    };
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&opt(&tree.0), &mut error),
        Err(ResolutionError::AppleDetection(_))
    ));
}

#[test]
fn ac5_apple_no_continues_to_document_claim() {
    // corpus bare::dir_apple_AND_pdf, native_detector_answers_no
    let tree = Tree::new();
    tree.file("top.pdf");
    let mut seam = seams();
    seam.claims = document;
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&opt(&tree.0), &mut seam),
        Ok(ResolutionOutcome::Resolved {
            source: ResolvedSource::Registry(RegistrySource::Document),
            ..
        })
    ));
}

#[test]
fn ac7_apple_route_leaves_source_tree_byte_identical() {
    // constructed Apple source tree
    let tree = Tree::new();
    tree.file("export.xml");
    let before = snapshot(&tree.0);
    let mut seam = seams();
    seam.apple_detector = yes_apple;
    assert_eq!(
        solstone_core_import::detect::resolve_import(&opt(&tree.0), &mut seam).unwrap(),
        ResolutionOutcome::RouteAppleHealth
    );
    assert_eq!(snapshot(&tree.0), before);
}

#[test]
fn ac3_non_registry_source_is_ignored_on_files() {
    // corpus source=recording::{cal.ics,doc.pdf,pic.png,audio.m4a,plain.txt}; source=nosuch::plain.txt
    let tree = Tree::new();
    for (name, expected) in [
        ("cal.ics", ResolvedSource::Registry(RegistrySource::Ics)),
        (
            "doc.pdf",
            ResolvedSource::Registry(RegistrySource::Document),
        ),
        ("pic.png", ResolvedSource::Registry(RegistrySource::Image)),
        ("audio.m4a", ResolvedSource::GenericAudio),
        ("plain.txt", ResolvedSource::GenericText),
    ] {
        let path = tree.file(name);
        for source in ["recording", "nosuch"] {
            let mut options = opt(&path);
            options.source = Some(source);
            let mut seam = seams();
            seam.claims = corpus_extension_claim;
            let outcome =
                solstone_core_import::detect::resolve_import(&options, &mut seam).unwrap();
            assert!(
                matches!(outcome, ResolutionOutcome::Resolved { source: actual, .. } if actual == expected)
            );
        }
    }
}

#[test]
fn ac6_claim_errors_are_swallowed_as_non_answers() {
    // corpus bare::zip_generic
    let tree = Tree::new();
    let path = tree.file("generic.zip");
    let mut seam = seams();
    seam.claims = failed_claim;
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&opt(&path), &mut seam),
        Ok(ResolutionOutcome::Resolved {
            source: ResolvedSource::GenericAudio,
            ..
        })
    ));
}

#[test]
fn ac8_oura_file_source_requires_sync() {
    // corpus source=oura::plain.txt
    let tree = Tree::new();
    let path = tree.file("plain.txt");
    let mut option = opt(&path);
    option.source = Some("oura");
    let mut seam = seams();
    let error = solstone_core_import::detect::resolve_import(&option, &mut seam).unwrap_err();
    assert_eq!(
        error.message(),
        "Oura body data imports through sync; use journal importer --sync oura"
    );
    option.source = None;
    option.timestamp = None;
    option.auto = AutoTimestamp::Bare;
    option.dry_run = false;
    let mut duplicate = seams();
    duplicate.manifest_lookup = |_| Some(ManifestSummary { entry_count: 1 });
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&option, &mut duplicate),
        Ok(ResolutionOutcome::Skipped {
            reason: SkipReason::AlreadyImported,
            ..
        })
    ));
    option.dry_run = true;
    let mut dry = seams();
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&option, &mut dry),
        Ok(ResolutionOutcome::Resolved { .. })
    ));
}

#[test]
fn ac9_zero_entry_manifest_does_not_skip_generic_source() {
    // constructed source; the manifest's entry_count is the negative dedup twin
    let tree = Tree::new();
    let path = tree.file("generic.m4a");
    let mut options = opt(&path);
    options.timestamp = None;
    options.auto = AutoTimestamp::Bare;
    options.dry_run = false;
    let mut seam = seams();
    seam.manifest_lookup = |_| Some(ManifestSummary { entry_count: 0 });
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&options, &mut seam),
        Ok(ResolutionOutcome::Resolved { .. })
    ));
}

#[test]
fn ac10_dry_run_suppresses_generic_dedup_lookup() {
    // constructed generic source
    let tree = Tree::new();
    let path = tree.file("generic.m4a");
    let mut options = opt(&path);
    options.timestamp = None;
    options.auto = AutoTimestamp::Bare;
    let mut seam = seams();
    seam.manifest_lookup = |_| panic!("dry run must not look up a manifest");
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&options, &mut seam),
        Ok(ResolutionOutcome::Resolved { .. })
    ));
}

#[test]
fn ac12_auto_states_preserve_adoption_rules() {
    // corpus auto=*::dated.m4a
    let tree = Tree::new();
    let path = tree.file("audio.m4a");
    for (auto, resolved) in [
        (AutoTimestamp::Absent, false),
        (AutoTimestamp::Bare, true),
        (AutoTimestamp::from_raw(Some(Some("hint"))), true),
        (AutoTimestamp::EmptyGuidance, false),
    ] {
        let mut option = opt(&path);
        option.timestamp = None;
        option.auto = auto;
        let mut seam = seams();
        assert_eq!(
            matches!(
                solstone_core_import::detect::resolve_import(&option, &mut seam).unwrap(),
                ResolutionOutcome::Resolved { .. }
            ),
            resolved
        );
    }
}

#[test]
fn ac13_deterministic_only_skips_without_model_detection() {
    // corpus bare::audio.m4a, auto=absent::audio.m4a_undetectable
    let tree = Tree::new();
    let path = tree.file("audio.m4a");
    let mut option = opt(&path);
    option.timestamp = None;
    option.deterministic_only = true;
    let mut seam = seams();
    seam.deterministic_detector = undetected;
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&option, &mut seam),
        Ok(ResolutionOutcome::Skipped {
            reason: SkipReason::NoDeterministicMatch,
            ..
        })
    ));
}

#[test]
fn ac11a_calendar_validation_applies_to_registry_sources() {
    // constructed file-importer path; corpus calendar message class
    let tree = Tree::new();
    let path = tree.file("calendar.ics");
    let mut options = opt(&path);
    options.source = Some("ics");
    options.timestamp = Some("20260230_120000");
    let mut seam = seams();
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&options, &mut seam),
        Err(ResolutionError::InvalidTimestampCalendar { .. })
    ));
}

#[test]
fn ac12a_guidance_reaches_model_for_non_detectable_source() {
    // constructed non-detectable source
    let tree = Tree::new();
    let path = tree.file("unknown.m4a");
    for (auto, expected) in [
        (
            AutoTimestamp::from_raw(Some(Some("help"))),
            Some("help".to_owned()),
        ),
        (AutoTimestamp::EmptyGuidance, Some(String::new())),
    ] {
        let mut options = opt(&path);
        options.timestamp = None;
        options.auto = auto;
        let mut captured = None;
        let mut seam = ResolutionSeams {
            apple_detector: no_apple,
            claims: no_claim,
            deterministic_detector: undetected,
            model_detector: |_: &Path, guidance: Option<&str>| {
                captured = guidance.map(str::to_owned);
                Ok::<_, ()>(Some(answer()))
            },
            manifest_lookup: lookup_none,
        };
        let _ = solstone_core_import::detect::resolve_import(&options, &mut seam).unwrap();
        assert_eq!(captured, expected);
    }
}

#[test]
fn ac12b_model_detector_runs_exactly_once() {
    // constructed non-detectable source
    let tree = Tree::new();
    let path = tree.file("unknown.m4a");
    let mut options = opt(&path);
    options.timestamp = None;
    options.auto = AutoTimestamp::Bare;
    let mut calls = 0;
    let mut seam = ResolutionSeams {
        apple_detector: no_apple,
        claims: no_claim,
        deterministic_detector: undetected,
        model_detector: |_: &Path, _: Option<&str>| {
            calls += 1;
            Ok::<_, ()>(Some(answer()))
        },
        manifest_lookup: lookup_none,
    };
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&options, &mut seam),
        Ok(ResolutionOutcome::Resolved { .. })
    ));
    assert_eq!(calls, 1);
}

#[test]
fn ac12c_unexpected_model_failure_is_named_refusal() {
    // constructed non-detectable source; deliberate divergence from detect_created.py:282-288
    let tree = Tree::new();
    let path = tree.file("unknown.m4a");
    let mut options = opt(&path);
    options.timestamp = None;
    let mut seam = ResolutionSeams {
        apple_detector: no_apple,
        claims: no_claim,
        deterministic_detector: undetected,
        model_detector: |_: &Path, _: Option<&str>| {
            Err::<Option<DetectedTimestamp>, _>("raw provider failure")
        },
        manifest_lookup: lookup_none,
    };
    let error = solstone_core_import::detect::resolve_import(&options, &mut seam).unwrap_err();
    assert!(matches!(
        error,
        ResolutionError::ModelDetectionFailed { .. }
    ));
    assert!(error.message().contains("provide --timestamp"));
    assert!(!error.message().contains("raw provider failure"));
}

#[test]
fn ac16_corpus_directory_and_extension_boundaries() {
    // corpus bare::{UPPER.PDF,dir_pdf_in_subdir,dir_vault_3md_hidden,dir_vault_3md_nested,dir_vault_logseq,dir_only_images}
    let tree = Tree::new();
    let upper = tree.file("UPPER.PDF");
    let mut upper_seam = seams();
    upper_seam.claims = |source: RegistrySource, path: &Path| {
        Ok::<_, ()>(
            source == RegistrySource::Document
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf")),
        )
    };
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&opt(&upper), &mut upper_seam),
        Ok(ResolutionOutcome::Resolved {
            source: ResolvedSource::Registry(RegistrySource::Document),
            ..
        })
    ));

    for name in [
        "dir_pdf_in_subdir",
        "dir_vault_3md_hidden",
        "dir_only_images",
    ] {
        let path = tree.0.join(name);
        fs::create_dir_all(&path).unwrap();
        let mut seam = seams();
        assert!(matches!(
            solstone_core_import::detect::resolve_import(&opt(&path), &mut seam),
            Err(ResolutionError::UnclaimedDirectory)
        ));
    }
    for name in ["dir_vault_3md_nested", "dir_vault_logseq"] {
        let path = tree.0.join(name);
        fs::create_dir_all(&path).unwrap();
        let mut seam = seams();
        seam.claims =
            |source: RegistrySource, _: &Path| Ok::<_, ()>(source == RegistrySource::Obsidian);
        assert!(matches!(
            solstone_core_import::detect::resolve_import(&opt(&path), &mut seam),
            Ok(ResolutionOutcome::Resolved {
                source: ResolvedSource::Registry(RegistrySource::Obsidian),
                ..
            })
        ));
    }
}

#[test]
fn ac17_unclaimed_directory_refusal_follows_pdf_boundary() {
    // not measured — derived from cli.py:557-561 and :681-690
    let tree = Tree::new();
    let mut seam = seams();
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&opt(&tree.0), &mut seam),
        Err(ResolutionError::UnclaimedDirectory)
    ));
    let pdf = tree.0.with_extension("pdf");
    fs::rename(&tree.0, &pdf).unwrap();
    std::mem::forget(tree);
    let mut seam = seams();
    assert!(matches!(
        solstone_core_import::detect::resolve_import(&opt(&pdf), &mut seam),
        Err(ResolutionError::PdfRequiresDocumentImporter)
    ));
    let _ = fs::remove_dir_all(pdf);
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut rows = fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.path(), fs::read(entry.path()).unwrap_or_default())
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows
}
