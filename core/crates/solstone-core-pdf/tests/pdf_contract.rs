// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use image::GenericImageView;
use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core-pdf");

fn corpus_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pdf_corpus")
        .join(name)
}

fn staged_pdfium_is_available() -> bool {
    solstone_core_pdf::pdfium_library_path().is_ok_and(|path| path.is_file())
}

fn test_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "solstone-core-pdf-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("create temporary test root");
    path
}

fn invoke(arguments: &[&str]) -> Output {
    Command::new(BINARY)
        .args(arguments)
        .output()
        .expect("run solstone-core-pdf")
}

fn json_output(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.lines().count(), 1, "stdout={stdout:?}");
    serde_json::from_str(&stdout).expect("one valid JSON document")
}

fn assert_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = json_output(output);
    assert_eq!(payload["schema"], "sol-pdf/1");
    payload
}

fn assert_oracle_payload(actual: &Value, mut expected: Value, normalize_render_dir: bool) {
    if expected.get("engine").is_some() {
        expected["engine"] = actual["engine"].clone();
    }
    if normalize_render_dir {
        expected["render"]["dir"] = actual["render"]["dir"].clone();
    }
    assert_eq!(actual, &expected);
}

#[test]
fn binary_contract_matches_oracle_or_skips_without_staged_pdfium() {
    if !staged_pdfium_is_available() {
        eprintln!(
            "skipping PDFium subprocess contract tests: set SOLSTONE_CORE_PDF_LIBRARY to a staged libpdfium"
        );
        return;
    }

    let text = corpus_path("text.pdf");
    let inspect = assert_success(&invoke(&[
        "inspect",
        text.to_str().expect("UTF-8 fixture path"),
    ]));
    assert_oracle_payload(
        &inspect,
        json!({
            "schema": "sol-pdf/1",
            "engine": "oracle-only",
            "sha256": "0a5c0aef0776024b3fa4ca8f29a7d12dbc9df56c2157c5ae6474dc6fa68479c6",
            "page_count": 2,
            "encrypted": false,
            "metadata": {"title": null, "author": null, "creation_date": null, "mod_date": null, "producer": null},
            "pages": [
                {"index": 1, "chars": 36, "width_pt": 612.0, "height_pt": 792.0, "image_area_fraction": 0.0, "rendered": null, "error": null},
                {"index": 2, "chars": 52, "width_pt": 612.0, "height_pt": 792.0, "image_area_fraction": 0.0, "rendered": null, "error": null}
            ],
            "render": null,
            "warnings": []
        }),
        false,
    );
    let inspect_password_after = assert_success(&invoke(&[
        "inspect",
        text.to_str().expect("UTF-8 fixture path"),
        "--password",
        "unused",
    ]));
    let inspect_password_before = assert_success(&invoke(&[
        "inspect",
        "--password",
        "unused",
        text.to_str().expect("UTF-8 fixture path"),
    ]));
    assert_eq!(inspect_password_before, inspect_password_after);

    let extract = assert_success(&invoke(&[
        "extract",
        text.to_str().expect("UTF-8 fixture path"),
    ]));
    assert_oracle_payload(
        &extract,
        json!({
            "schema": "sol-pdf/1",
            "engine": "oracle-only",
            "sha256": "0a5c0aef0776024b3fa4ca8f29a7d12dbc9df56c2157c5ae6474dc6fa68479c6",
            "page_count": 2,
            "encrypted": false,
            "metadata": {"title": null, "author": null, "creation_date": null, "mod_date": null, "producer": null},
            "pages": [
                {"index": 1, "chars": 36, "width_pt": 612.0, "height_pt": 792.0, "image_area_fraction": 0.0, "rendered": null, "error": null, "text": "First page has ordinary extractable text."},
                {"index": 2, "chars": 52, "width_pt": 612.0, "height_pt": 792.0, "image_area_fraction": 0.0, "rendered": null, "error": null, "text": "Second page carries SOLPDF_SENTINEL_PAGE_2 for assertion."}
            ],
            "render": null,
            "warnings": []
        }),
        false,
    );

    let image_only = assert_success(&invoke(&[
        "extract",
        corpus_path("scan.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
    ]));
    assert_oracle_payload(
        &image_only,
        json!({
            "schema": "sol-pdf/1",
            "engine": "oracle-only",
            "sha256": "f90c290a2001d796104204c090af15796e25dcbcd03941819b778fea1a6f2536",
            "page_count": 2,
            "encrypted": false,
            "metadata": {"title": null, "author": null, "creation_date": null, "mod_date": null, "producer": null},
            "pages": [
                {"index": 1, "chars": 0, "width_pt": 612.0, "height_pt": 792.0, "image_area_fraction": 1.0, "rendered": null, "error": null, "text": ""},
                {"index": 2, "chars": 0, "width_pt": 612.0, "height_pt": 792.0, "image_area_fraction": 1.0, "rendered": null, "error": null, "text": ""}
            ],
            "render": null,
            "warnings": []
        }),
        false,
    );

    let oracle_renders = test_root("oracle-renders");
    let rendered_oracle = assert_success(&invoke(&[
        "extract",
        corpus_path("scan.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
        "--render-below-chars",
        "50",
        "--render-dir",
        oracle_renders.to_str().expect("UTF-8 temporary path"),
        "--dpi",
        "72",
    ]));
    assert_oracle_payload(
        &rendered_oracle,
        json!({
            "schema": "sol-pdf/1",
            "engine": "oracle-only",
            "sha256": "f90c290a2001d796104204c090af15796e25dcbcd03941819b778fea1a6f2536",
            "page_count": 2,
            "encrypted": false,
            "metadata": {"title": null, "author": null, "creation_date": null, "mod_date": null, "producer": null},
            "pages": [
                {"index": 1, "chars": 0, "width_pt": 612.0, "height_pt": 792.0, "image_area_fraction": 1.0, "rendered": "page-0001.png", "error": null, "text": ""},
                {"index": 2, "chars": 0, "width_pt": 612.0, "height_pt": 792.0, "image_area_fraction": 1.0, "rendered": "page-0002.png", "error": null, "text": ""}
            ],
            "render": {"dpi": 72, "dir": "/tmp/render-fixture/rasters"},
            "warnings": []
        }),
        true,
    );

    let renders = test_root("renders");
    let mixed = corpus_path("mixed-render-union.pdf");
    let rendered = assert_success(&invoke(&[
        "extract",
        mixed.to_str().expect("UTF-8 fixture path"),
        "--render-pages",
        "1",
        "--render-below-chars",
        "1",
        "--render-above-image-fraction",
        "0.30",
        "--render-dir",
        renders.to_str().expect("UTF-8 temporary path"),
    ]));
    assert_eq!(
        rendered["pages"]
            .as_array()
            .expect("pages")
            .iter()
            .map(|page| page["rendered"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("page-0001.png"),
            json!("page-0002.png"),
            json!("page-0003.png")
        ]
    );
    let first_render = image::open(renders.join("page-0001.png")).expect("open rendered PNG");
    assert_eq!(first_render.dimensions(), (1275, 1650));

    let equals_render_dir = test_root("equals-render-flags");
    let equals_render_dir = equals_render_dir.to_str().expect("UTF-8 temporary path");
    let equals_flags = assert_success(&invoke(&[
        "extract",
        text.to_str().expect("UTF-8 fixture path"),
        "--dpi=72",
        "--render-below-chars=10",
        &format!("--render-dir={equals_render_dir}"),
    ]));
    let spaced_flags = assert_success(&invoke(&[
        "extract",
        text.to_str().expect("UTF-8 fixture path"),
        "--dpi",
        "72",
        "--render-below-chars",
        "10",
        "--render-dir",
        equals_render_dir,
    ]));
    assert_eq!(equals_flags, spaced_flags);

    let nan_render_dir = test_root("nan-render-threshold");
    let nan = assert_success(&invoke(&[
        "extract",
        corpus_path("scan.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
        "--render-above-image-fraction",
        "nan",
        "--render-dir",
        nan_render_dir.to_str().expect("UTF-8 temporary path"),
    ]));
    assert!(
        nan["pages"]
            .as_array()
            .expect("pages")
            .iter()
            .all(|page| page["rendered"].is_null())
    );

    let whitespace = assert_success(&invoke(&[
        "extract",
        corpus_path("whitespace.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
    ]));
    assert_eq!(whitespace["pages"][0]["chars"], 0);

    let dated = assert_success(&invoke(&[
        "inspect",
        corpus_path("dated.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
    ]));
    assert_eq!(
        dated["metadata"],
        json!({"title":"Dated Fixture","author":"sol","creation_date":"2026-03-04T11:02:00-07:00","mod_date":"2026-03-04T12:22:33+02:30","producer":"fixture"})
    );
    for fixture in ["missing-dates.pdf", "garbled-dates.pdf"] {
        let payload = assert_success(&invoke(&[
            "inspect",
            corpus_path(fixture).to_str().expect("UTF-8 fixture path"),
        ]));
        assert!(payload["metadata"]["creation_date"].is_null());
        assert!(payload["metadata"]["mod_date"].is_null());
    }

    let encrypted = invoke(&[
        "inspect",
        corpus_path("encrypted-user.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
    ]);
    assert_eq!(encrypted.status.code(), Some(3));
    assert_eq!(
        json_output(&encrypted),
        json!({"schema":"sol-pdf/1","error":"encrypted"})
    );
    let unlocked = assert_success(&invoke(&[
        "extract",
        corpus_path("encrypted-user.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
        "--password",
        "userpass",
    ]));
    assert_eq!(unlocked["encrypted"], true);
    let owner_only = assert_success(&invoke(&[
        "extract",
        corpus_path("encrypted-owner.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
    ]));
    assert_eq!(owner_only["encrypted"], true);

    let clean = assert_success(&invoke(&[
        "extract",
        corpus_path("truncation-clean.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
    ]));
    for fixture in ["truncated-drop-startxref.pdf", "truncated-drop-eof.pdf"] {
        let payload = assert_success(&invoke(&[
            "extract",
            corpus_path(fixture).to_str().expect("UTF-8 fixture path"),
        ]));
        assert_eq!(payload["page_count"], clean["page_count"]);
        assert_eq!(payload["warnings"], json!([]));
    }
    for fixture in ["truncated-deep.pdf", "zero-byte.pdf", "garbage.pdf"] {
        let corrupt = invoke(&[
            "inspect",
            corpus_path(fixture).to_str().expect("UTF-8 fixture path"),
        ]);
        assert_eq!(corrupt.status.code(), Some(4), "fixture={fixture}");
        assert_eq!(json_output(&corrupt)["error"], "corrupt");
    }
    let oracle_corrupt = invoke(&[
        "extract",
        corpus_path("garbage.pdf")
            .to_str()
            .expect("UTF-8 fixture path"),
    ]);
    assert_eq!(oracle_corrupt.status.code(), Some(4));
    assert_eq!(
        json_output(&oracle_corrupt),
        json!({"schema":"sol-pdf/1","error":"corrupt","detail":"Failed to load document (PDFium: Data format error)."})
    );

    let missing_root = test_root("oracle-missing");
    let missing = missing_root.join("nope.pdf");
    let usage_oracle = invoke(&["extract", missing.to_str().expect("UTF-8 temporary path")]);
    assert_eq!(usage_oracle.status.code(), Some(2));
    assert_oracle_payload(
        &json_output(&usage_oracle),
        json!({"schema":"sol-pdf/1","error":"usage","detail":format!("PDF not found: {}", missing.display())}),
        false,
    );

    let not_a_directory = test_root("not-a-directory").join("file");
    fs::write(&not_a_directory, "x").expect("create render parent file");
    let render_failure = invoke(&[
        "extract",
        text.to_str().expect("UTF-8 fixture path"),
        "--render-below-chars",
        "1000",
        "--render-dir",
        not_a_directory
            .join("child")
            .to_str()
            .expect("UTF-8 temporary path"),
    ]);
    assert_eq!(render_failure.status.code(), Some(5));
    assert_eq!(json_output(&render_failure)["error"], "render-io");
    fs::remove_file(not_a_directory).expect("remove render parent file");

    let usage = invoke(&[
        "extract",
        text.to_str().expect("UTF-8 fixture path"),
        "--render-pages",
        "1",
    ]);
    assert_eq!(usage.status.code(), Some(2));
    assert_eq!(json_output(&usage)["error"], "usage");

    fs::remove_dir_all(renders).expect("remove temporary renders");
    fs::remove_dir_all(oracle_renders).expect("remove oracle renders");
    fs::remove_dir_all(missing_root).expect("remove missing-file root");
}

#[test]
fn dependency_closure_has_no_network_client() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--offline",
            "--format-version=1",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value =
        serde_json::from_slice(&output.stdout).expect("valid cargo metadata JSON");
    let package_names = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .map(|package| {
            (
                package["id"].as_str().expect("package id").to_owned(),
                package["name"].as_str().expect("package name").to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let root = package_names
        .iter()
        .find_map(|(id, name)| (name == "solstone-core-pdf").then_some(id.clone()))
        .expect("PDF package");
    let dependencies = metadata["resolve"]["nodes"]
        .as_array()
        .expect("metadata resolve nodes")
        .iter()
        .map(|node| {
            let id = node["id"].as_str().expect("node id").to_owned();
            let dependencies = node["dependencies"]
                .as_array()
                .expect("node dependencies")
                .iter()
                .map(|dependency| dependency.as_str().expect("dependency id").to_owned())
                .collect::<Vec<_>>();
            (id, dependencies)
        })
        .collect::<BTreeMap<_, _>>();
    let mut closure = BTreeSet::from([root]);
    let mut pending = closure.clone();
    while let Some(id) = pending.pop_first() {
        for dependency in dependencies.get(&id).expect("resolved package") {
            if closure.insert(dependency.clone()) {
                pending.insert(dependency.clone());
            }
        }
    }
    let forbidden = [
        "async-net",
        "curl",
        "hyper",
        "hyper-util",
        "isahc",
        "mio",
        "native-tls",
        "reqwest",
        "socket2",
        "surf",
        "tokio",
        "ureq",
    ];
    let present = closure
        .into_iter()
        .filter_map(|id| package_names.get(&id))
        .filter(|name| forbidden.contains(&name.as_str()))
        .collect::<Vec<_>>();
    assert!(
        present.is_empty(),
        "solstone-core-pdf dependency closure contains network-client crates: {present:?}"
    );
}
