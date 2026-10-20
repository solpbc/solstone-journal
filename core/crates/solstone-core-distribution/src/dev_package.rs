// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A package around a checkout's own build, for running that build where an
//! installed journal would run.
//!
//! Backup, confidential processing and sound tagging run only members of the
//! installed package beside the executable (`lib/solstone-restic/restic`, the
//! nvattest tree, the CED runtime), verified against the installed-payload
//! manifest. A checkout build has no package, so those features refuse there.
//! This writes `<out>/bin` (the build's own executables, under their build
//! names, beside the `solstone` and `journal` launchers that exec them), every pinned member the host target ships, staged and verified
//! exactly as `produce` stages them, and the manifest over the result. The
//! executables still find the checkout's talent and app roots by walking up.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use solstone_core_installed_payload::{
    COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, PRODUCT, compiled_target,
    render_installed_payload,
};

use crate::acquire::PDF_LINK_ROOT;
use crate::inventory::{Entry, load_inventory};
use crate::pdfium;
use crate::produce::{ProduceError, stage_pinned_entry};

const INVENTORY: &str = "core/distribution/inventory.toml";

pub fn usage() -> &'static str {
    "usage: solstone-distribution dev-package <out> --bin-dir <dir> --runtime-root <dir> --screen-ocr-helper-dir <dir> --source-commit <sha>"
}

#[derive(Debug, PartialEq, Eq)]
pub struct DevPackageArgs {
    pub out: PathBuf,
    pub bin_dir: PathBuf,
    /// Where the build staged its runtime directories (the Cargo target dir):
    /// the ONNX Runtime the VAD and speaker helpers load is read from
    /// `<runtime_root>/<dest_dir>`, the same relative path the package uses.
    pub runtime_root: PathBuf,
    pub screen_ocr_helper_dir: PathBuf,
    pub source_commit: String,
}

pub fn parse(args: &[String]) -> Result<DevPackageArgs, ProduceError> {
    let usage = || ProduceError::new(usage());
    let (out, rest) = args.split_first().ok_or_else(usage)?;
    let (mut bin_dir, mut runtime_root, mut screen_ocr_helper_dir, mut source_commit) =
        (None, None, None, None);
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or_else(usage)?;
        match flag.as_str() {
            "--bin-dir" => bin_dir = Some(PathBuf::from(value)),
            "--runtime-root" => runtime_root = Some(PathBuf::from(value)),
            "--screen-ocr-helper-dir" => screen_ocr_helper_dir = Some(PathBuf::from(value)),
            "--source-commit" => source_commit = Some(value.clone()),
            _ => return Err(usage()),
        }
    }
    Ok(DevPackageArgs {
        out: PathBuf::from(out),
        bin_dir: bin_dir.ok_or_else(usage)?,
        runtime_root: runtime_root.ok_or_else(usage)?,
        screen_ocr_helper_dir: screen_ocr_helper_dir.ok_or_else(usage)?,
        source_commit: source_commit.ok_or_else(usage)?,
    })
}

/// Build the package. The caller has already fetched the host target's pinned
/// inputs into the catalog cache (`acquire catalog-inputs`).
pub fn run(repo: &Path, args: &DevPackageArgs) -> Result<(), ProduceError> {
    let inventory = load_inventory(&repo.join(INVENTORY))
        .map_err(|error| ProduceError::new(error.to_string()))?;
    build(repo, &inventory.entry, compiled_target(), args)
}

pub(crate) fn build(
    repo: &Path,
    entries: &[Entry],
    target_id: &str,
    args: &DevPackageArgs,
) -> Result<(), ProduceError> {
    let out = &args.out;
    if out.parent().is_none() || out.as_os_str().is_empty() {
        return Err(ProduceError::new(
            "dev-package refuses an empty or root output",
        ));
    }
    match fs::remove_dir_all(out) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    fs::create_dir_all(out.join("bin"))?;
    let catalog_cache = crate::pinned_stage::catalog_input_cache_dir(repo);
    let mut placed = 0;
    for entry in entries {
        match entry {
            Entry::Bin {
                bin, dest, targets, ..
            } if targets.iter().any(|t| t == target_id) => {
                let built = if bin == crate::screen_ocr::HELPER_BIN {
                    args.screen_ocr_helper_dir
                        .join(if target_id == "windows-x86_64" {
                            format!("{bin}.exe")
                        } else {
                            bin.clone()
                        })
                } else {
                    args.bin_dir.join(bin)
                };
                if built.is_file() {
                    link_or_copy(&built, &out.join(dest))?;
                    placed += 1;
                }
            }
            Entry::Launcher {
                source,
                dest,
                mode,
                targets,
                ..
            } if targets.iter().any(|t| t == target_id) => {
                let bytes = fs::read(repo.join(source))?;
                crate::stage::write_staged_file_mode(out, dest, &bytes, *mode)?;
            }
            Entry::OnnxRuntime {
                dest_dir, targets, ..
            } if targets.iter().any(|t| t == target_id) => {
                copy_runtime_dir(&args.runtime_root.join(dest_dir), &out.join(dest_dir))?;
            }
            Entry::Pdfium {
                dest_dir, targets, ..
            } if targets.iter().any(|t| t == target_id) => {
                copy_pdfium(repo, target_id, dest_dir, out)?;
            }
            Entry::ScreenOcr { targets, .. } if targets.iter().any(|t| t == target_id) => {
                let target = crate::screen_ocr::target_spec(target_id).ok_or_else(|| {
                    ProduceError::new(format!("screen OCR target unavailable: {target_id}"))
                })?;
                let helper = args.screen_ocr_helper_dir.join(if target.os == "windows" {
                    format!("{}.exe", crate::screen_ocr::HELPER_BIN)
                } else {
                    crate::screen_ocr::HELPER_BIN.to_owned()
                });
                let bundle = crate::screen_ocr::prepare_from_cache(
                    repo,
                    &catalog_cache.join("screen-ocr"),
                    target_id,
                    &helper,
                    target.triple,
                )
                .map_err(ProduceError::new)?;
                crate::screen_ocr::stage_files(&bundle, out).map_err(ProduceError::new)?;
            }
            Entry::PinnedNative { .. } | Entry::PinnedMembers { .. } => {
                stage_pinned_entry(entry, repo, &catalog_cache, target_id, out)?;
            }
            _ => {}
        }
    }
    if placed == 0 {
        return Err(ProduceError::new(format!(
            "dev-package found no built executable in {}",
            args.bin_dir.display()
        )));
    }
    let manifest = render_installed_payload(
        out,
        PRODUCT,
        COMPILED_VERSION,
        target_id,
        &args.source_commit,
    )
    .map_err(|refusal| ProduceError::new(format!("dev-package manifest: {}", refusal.code)))?;
    let path = out.join(INSTALLED_PAYLOAD_MANIFEST);
    fs::create_dir_all(path.parent().expect("manifest has a parent"))?;
    fs::write(path, manifest)?;
    Ok(())
}

fn copy_runtime_dir(from: &Path, to: &Path) -> Result<(), ProduceError> {
    let entries = fs::read_dir(from).map_err(|error| {
        ProduceError::new(format!(
            "dev-package needs the staged runtime {} ({error}); run make build-sandbox-processing",
            from.display()
        ))
    })?;
    fs::create_dir_all(to)?;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// The PDFium library `acquire pdfium` staged for the target, written where and
/// with the mode the release writes it; the PDF worker loads it from
/// `<prefix>/lib/solstone-core-pdf` beside its `bin`.
fn copy_pdfium(
    repo: &Path,
    target_id: &str,
    dest_dir: &str,
    out: &Path,
) -> Result<(), ProduceError> {
    let spec = pdfium::spec_for(target_id).ok_or_else(|| {
        ProduceError::new(format!("missing required:\n  pdfium runtime {target_id}"))
    })?;
    let from = repo
        .join(PDF_LINK_ROOT)
        .join(spec.key)
        .join(spec.library_name);
    let bytes = fs::read(&from).map_err(|error| {
        ProduceError::new(format!(
            "dev-package needs the staged PDFium {} ({error}); run make check-rust-pdf-stage",
            from.display()
        ))
    })?;
    crate::stage::write_staged_file_mode(
        out,
        &format!("{dest_dir}/{}", spec.library_name),
        &bytes,
        pdfium::LIB_MODE,
    )?;
    Ok(())
}

fn link_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    if fs::hard_link(from, to).is_err() {
        fs::copy(from, to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use solstone_core_installed_payload::{InstalledPackage, TARGET_LINUX_X86_64};

    fn args(rest: &[&str]) -> Vec<String> {
        rest.iter().map(|s| (*s).to_owned()).collect()
    }

    fn write_screen_ocr_fixture(repo: &Path) {
        use crate::screen_ocr::{Recipe, materialize_dictionary, target_spec};

        let target = TARGET_LINUX_X86_64;
        let spec = target_spec(target).unwrap();
        let detector = b"fixture detector";
        let recognizer = b"fixture recognizer";
        let dictionary_yaml = b"PostProcess:\n  character_dict:\n    - A\n    - B\n";
        let dictionary = materialize_dictionary(dictionary_yaml).unwrap();
        let wheel = crate::zip::write_stored_zip(&[
            (spec.runtime_member, b"fixture runtime"),
            (
                spec.dependency_member.unwrap(),
                b"fixture provider dependency",
            ),
            ("onnxruntime/LICENSE", b"ORT license"),
            (
                "onnxruntime/ThirdPartyNotices.txt",
                b"ORT third-party notices",
            ),
        ])
        .unwrap();
        let wheel_sha = crate::digest::sha256_hex(&wheel);
        let recipe = format!(
            "schema_version = 1\nprotocol_version = 1\n\
             [source]\noar_git = \"https://github.com/GreatV/oar-ocr\"\noar_revision = \"84817f58d7a0932d5368a9fa18105ba5d842af29\"\noar_default_features = false\noar_features = [\"simd\"]\nort_version = \"2.0.0-rc.13\"\nort_features = [\"std\", \"load-dynamic\"]\n\
             [settings]\ntelemetry = false\ncpu_only = true\n\
             [limits]\nmax_regions = 3000\n\
             [models.detector]\nfilename = \"detector.onnx\"\nurl = \"https://example.invalid/detector\"\nsha256 = \"{}\"\n\
             [models.recognizer]\nfilename = \"recognizer.onnx\"\nurl = \"https://example.invalid/recognizer\"\nsha256 = \"{}\"\n\
             [models.dictionary]\nfilename = \"dictionary.txt\"\nsource_url = \"https://example.invalid/dictionary\"\nsource_sha256 = \"{}\"\nmaterialized_entries = 2\nmaterialized_final_newline = true\nmaterialized_sha256 = \"{}\"\n\
             [runtime]\nversion = \"1.31.0\"\nlinux_max_glibc = \"2.34\"\nlinux_build_lane = \"GNU Zig\"\n\
             [runtime.linux_x86_64]\nfilename = \"runtime.whl\"\nurl = \"https://example.invalid/runtime\"\nsha256 = \"{wheel_sha}\"\n\
             [runtime.linux_aarch64]\nfilename = \"runtime.whl\"\nurl = \"https://example.invalid/runtime\"\nsha256 = \"{wheel_sha}\"\n\
             [runtime.macos_arm64]\nfilename = \"runtime.whl\"\nurl = \"https://example.invalid/runtime\"\nsha256 = \"{wheel_sha}\"\n\
             [runtime.windows_x86_64]\nfilename = \"runtime.whl\"\nurl = \"https://example.invalid/runtime\"\nsha256 = \"{wheel_sha}\"\n",
            crate::digest::sha256_hex(detector),
            crate::digest::sha256_hex(recognizer),
            crate::digest::sha256_hex(dictionary_yaml),
            crate::digest::sha256_hex(&dictionary),
        );
        let recipe_bytes = recipe.as_bytes();
        Recipe::load(recipe_bytes).unwrap();
        let helper_manifest = b"[package]\nname = \"solstone-screen-ocr-helper\"\n";
        let helper_lock = b"version = 4\n\n[[package]]\nname = \"oar-ocr\"\nversion = \"0.10.0\"\nsource = \"git+https://github.com/GreatV/oar-ocr?rev=84817f58d7a0932d5368a9fa18105ba5d842af29#84817f58d7a0932d5368a9fa18105ba5d842af29\"\n";
        let source_directions = b"ORT and Eigen source directions";
        let rust_notices = b"Rust dependency notices";
        let index = serde_json::to_vec(&json!({
            "schema": "solstone.screen-ocr-rust-notices.v1",
            "helper_manifest_sha256": crate::digest::sha256_hex(helper_manifest),
            "cargo_lock_sha256": crate::digest::sha256_hex(helper_lock),
            "recipe_sha256": crate::digest::sha256_hex(recipe_bytes),
            "source_directions_sha256": crate::digest::sha256_hex(source_directions),
            "oar_git": "https://github.com/GreatV/oar-ocr",
            "oar_revision": "84817f58d7a0932d5368a9fa18105ba5d842af29",
            "notices_sha256": crate::digest::sha256_hex(rust_notices),
        }))
        .unwrap();

        let put = |relative: &str, bytes: &[u8]| {
            let path = repo.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        };
        put(crate::screen_ocr::RECIPE, recipe_bytes);
        put(crate::screen_ocr::HELPER_MANIFEST, helper_manifest);
        put(crate::screen_ocr::HELPER_LOCK, helper_lock);
        put("core/screen-ocr-helper/LICENSE", b"AGPL license");
        put("core/screen-ocr-helper/LICENSE-APACHE", b"Apache license");
        put(
            "core/screen-ocr-helper/SOURCE-DIRECTIONS.md",
            source_directions,
        );
        put(
            "core/screen-ocr-helper/NOTICE",
            b"OAR and model attribution",
        );
        put(crate::screen_ocr::CARGO_NOTICES, rust_notices);
        put(crate::screen_ocr::CARGO_SOURCE_INDEX, &index);

        let cache = crate::pinned_stage::catalog_input_cache_dir(repo)
            .join("screen-ocr")
            .join(target);
        put_cache(&cache, "detector.onnx", detector);
        put_cache(&cache, "recognizer.onnx", recognizer);
        put_cache(&cache, "inference.yml", dictionary_yaml);
        put_cache(&cache, "runtime.whl", &wheel);
    }

    fn put_cache(cache: &Path, name: &str, bytes: &[u8]) {
        fs::create_dir_all(cache).unwrap();
        fs::write(cache.join(name), bytes).unwrap();
    }

    #[test]
    fn parse_requires_the_bin_dir_helper_dir_and_the_commit() {
        assert_eq!(
            parse(&args(&[
                "out",
                "--bin-dir",
                "b",
                "--runtime-root",
                "r",
                "--screen-ocr-helper-dir",
                "h",
                "--source-commit",
                "c"
            ]))
            .unwrap(),
            DevPackageArgs {
                out: "out".into(),
                bin_dir: "b".into(),
                runtime_root: "r".into(),
                screen_ocr_helper_dir: "h".into(),
                source_commit: "c".into()
            }
        );
        assert!(parse(&args(&["out", "--bin-dir", "b", "--source-commit", "c"])).is_err());
        assert!(
            parse(&args(&[
                "out",
                "--bin-dir",
                "b",
                "--runtime-root",
                "r",
                "--source-commit",
                "c"
            ]))
            .is_err()
        );
        assert!(parse(&args(&["out", "--source-commit", "c", "--other", "x"])).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn the_package_admits_and_carries_the_built_executables_under_their_build_names() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("debug");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("solstone-core"), b"core").unwrap();
        let screen_ocr_helper_dir = tmp.path().join("screen-ocr-helper");
        fs::create_dir_all(&screen_ocr_helper_dir).unwrap();
        fs::write(
            screen_ocr_helper_dir.join(crate::screen_ocr::HELPER_BIN),
            b"screen OCR helper",
        )
        .unwrap();
        fs::write(bin_dir.join("unlisted-tool"), b"not shipped").unwrap();
        write_screen_ocr_fixture(tmp.path());
        let bin = |package: &str, name: &str| Entry::Bin {
            class: None,
            package: package.into(),
            bin: name.into(),
            dest: format!("bin/{name}-packaged"),
            mode: 0o755,
            lane: "native".into(),
            targets: vec![TARGET_LINUX_X86_64.into()],
        };
        fs::create_dir_all(tmp.path().join("scripts")).unwrap();
        fs::write(tmp.path().join("scripts/journal"), b"#!/bin/sh\n").unwrap();
        let entries = [
            bin("solstone-core", "solstone-core"),
            bin("solstone-core-journal-bin", "solstone-core-journal"),
            Entry::Bin {
                class: None,
                package: crate::screen_ocr::HELPER_PACKAGE.into(),
                bin: crate::screen_ocr::HELPER_BIN.into(),
                dest: "bin/solstone-screen-ocr-helper".into(),
                mode: 0o755,
                lane: "native".into(),
                targets: vec![TARGET_LINUX_X86_64.into()],
            },
            Entry::ScreenOcr {
                class: None,
                dest_dir: crate::screen_ocr::BUNDLE_DEST.into(),
                targets: vec![TARGET_LINUX_X86_64.into()],
            },
            Entry::Launcher {
                class: None,
                source: "scripts/journal".into(),
                dest: "bin/journal".into(),
                mode: 0o755,
                targets: vec![TARGET_LINUX_X86_64.into()],
            },
            Entry::OnnxRuntime {
                class: None,
                dest_dir: "lib/onnx-helper".into(),
                mode: 0o755,
                identities: vec![],
                targets: vec![TARGET_LINUX_X86_64.into()],
            },
        ];
        fs::create_dir_all(tmp.path().join("target/lib/onnx-helper")).unwrap();
        fs::write(tmp.path().join("target/lib/onnx-helper/libort.so"), b"ort").unwrap();
        let out = tmp.path().join("pkg");
        fs::create_dir_all(out.join("stale")).unwrap();
        build(
            tmp.path(),
            &entries,
            TARGET_LINUX_X86_64,
            &DevPackageArgs {
                out: out.clone(),
                bin_dir,
                runtime_root: tmp.path().join("target"),
                screen_ocr_helper_dir,
                source_commit: "0123abc".into(),
            },
        )
        .unwrap();
        assert!(!out.join("stale").exists());
        assert_eq!(
            fs::read(out.join("bin/solstone-core-packaged")).unwrap(),
            b"core"
        );
        assert_eq!(
            fs::read(out.join("bin/solstone-screen-ocr-helper")).unwrap(),
            b"screen OCR helper"
        );
        assert!(!out.join("bin/unlisted-tool").exists());
        assert_eq!(fs::read(out.join("bin/journal")).unwrap(), b"#!/bin/sh\n");
        assert_eq!(
            fs::read(out.join("lib/onnx-helper/libort.so")).unwrap(),
            b"ort"
        );
        let package = InstalledPackage::admit(&out, COMPILED_VERSION, TARGET_LINUX_X86_64).unwrap();
        assert_eq!(
            package.member("bin/solstone-core-packaged").unwrap(),
            out.join("bin/solstone-core-packaged")
        );
        assert_eq!(
            package.member("bin/solstone-screen-ocr-helper").unwrap(),
            out.join("bin/solstone-screen-ocr-helper")
        );
        assert_eq!(
            package
                .member("lib/solstone-screen-ocr/models/detector.onnx")
                .unwrap(),
            out.join("lib/solstone-screen-ocr/models/detector.onnx")
        );
        assert_eq!(
            package
                .member("lib/solstone-screen-ocr/runtime/libonnxruntime.so.1.31.0")
                .unwrap(),
            out.join("lib/solstone-screen-ocr/runtime/libonnxruntime.so.1.31.0")
        );
        assert!(
            package
                .member("lib/solstone-screen-ocr/provenance/rust-sources.json")
                .is_ok()
        );
    }

    #[test]
    fn the_package_carries_the_staged_pdf_library_for_its_target_only() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("debug");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("solstone-core-pdf"), b"pdf").unwrap();
        let staged = tmp.path().join("target/pdfium-runtime-link/linux-x86_64");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("libpdfium.so"), b"pdf library").unwrap();
        let pdf_library = |dest_dir: &str, target: &str| Entry::Pdfium {
            class: None,
            dest_dir: dest_dir.into(),
            mode: 0o755,
            identities: vec![],
            targets: vec![target.into()],
        };
        let entries = [
            Entry::Bin {
                class: None,
                package: "solstone-core-pdf".into(),
                bin: "solstone-core-pdf".into(),
                dest: "bin/solstone-core-pdf".into(),
                mode: 0o755,
                lane: "native".into(),
                targets: vec![TARGET_LINUX_X86_64.into()],
            },
            pdf_library("lib/solstone-core-pdf", TARGET_LINUX_X86_64),
            pdf_library("lib/other-target-pdf", "macos-arm64"),
        ];
        let out = tmp.path().join("pkg");
        build(
            tmp.path(),
            &entries,
            TARGET_LINUX_X86_64,
            &DevPackageArgs {
                out: out.clone(),
                bin_dir,
                runtime_root: tmp.path().join("target"),
                screen_ocr_helper_dir: tmp.path().join("helper"),
                source_commit: "0123abc".into(),
            },
        )
        .unwrap();
        let carried = out.join("lib/solstone-core-pdf/libpdfium.so");
        assert_eq!(fs::read(&carried).unwrap(), b"pdf library");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&carried).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        assert!(!out.join("lib/other-target-pdf").exists());
    }
}
