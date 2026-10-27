// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Adapt the maintained indexed archive protocol to the ordinary retained route.
//! Archive member/tile metrics stay separate from generic recording previews.

use std::path::{Component, Path};

use serde_json::{Value, json};
use solstone_core_import::ImportMetadata;

use crate::preview::PreviewError;

pub(crate) const SIZE_SCHEMA: &str = "solstone.import.audio-archive.size.v1";

pub(crate) fn selected(metadata: &ImportMetadata) -> bool {
    crate::lifecycle::source_hint(metadata).as_deref() == Some("audio_archive")
}

fn identity(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// The index is only a lookup accelerator; its reader validates the current run.
/// This never allocates a run, repairs state or treats a missing run as saved.
pub(crate) fn run_id(
    root: &Path,
    metadata: &ImportMetadata,
) -> Result<Option<String>, PreviewError> {
    if !selected(metadata) {
        return Ok(None);
    }
    let Some(digest) = metadata
        .get("preview")
        .and_then(|record| identity(&record["size"]["archive_identity"]))
    else {
        return Ok(None);
    };
    solstone_core_import::audio_archive::read_matching_archive_run(root, digest)
        .map_err(|error| PreviewError::PlanFailed(error.to_string()))
}

pub(crate) fn configure_options(
    root: &Path,
    data: &Value,
    metadata: &mut ImportMetadata,
    options: &mut Value,
) -> Result<(), PreviewError> {
    if !selected(metadata) {
        return Ok(());
    }
    if data["force"].as_bool() == Some(true)
        || crate::lifecycle::owner_timestamp_choice(data, metadata).is_some()
    {
        return Err(PreviewError::InvalidRequest(
            "archive recording times come from their filenames or manifest".into(),
        ));
    }
    let manifest = data["archive_manifest"]
        .as_str()
        .filter(|value| !value.is_empty());
    if (data
        .get("archive_manifest")
        .is_some_and(|value| !value.is_null() && !value.is_string()))
        || manifest.is_some_and(|name| {
            name.len() > 1024
                || Path::new(name)
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
        })
    {
        return Err(PreviewError::InvalidRequest(
            "choose a JSON manifest inside the selected folder".into(),
        ));
    }
    let zone = data["source_zone"]
        .as_str()
        .filter(|value| !value.is_empty());
    if data
        .get("source_zone")
        .is_some_and(|value| !value.is_null() && !value.is_string())
        || zone.is_some_and(|value| solstone_core_journal_config::parse_zone(value).is_none())
        || (manifest.is_none() && zone.is_none())
    {
        return Err(PreviewError::InvalidRequest(
            "choose the time zone used by the recording filenames".into(),
        ));
    }
    options["source_zone"] = json!(zone);
    options["archive_manifest"] = json!(manifest);
    let previous = metadata.get("preview");
    let same = previous.is_some_and(|record| {
        record["options"]["source_zone"] == options["source_zone"]
            && record["options"]["archive_manifest"] == options["archive_manifest"]
    });
    let resume = if same {
        json!(run_id(root, metadata)?)
    } else {
        Value::Null
    };
    metadata.insert("_archive_run_id".into(), resume);
    Ok(())
}

pub(crate) fn resume_hint(root: &Path, metadata: &mut ImportMetadata) -> Result<(), PreviewError> {
    let resume = run_id(root, metadata)?;
    metadata.insert("_archive_run_id".into(), json!(resume));
    Ok(())
}

pub(crate) fn completed_run(
    root: &Path,
    data: &Value,
    metadata: &ImportMetadata,
) -> Result<Option<String>, PreviewError> {
    if !selected(metadata) {
        return Ok(None);
    }
    let mut observed = metadata.clone();
    configure_options(root, data, &mut observed, &mut json!({}))?;
    let Some(id) = observed.get("_archive_run_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let store = solstone_core_import::audio_archive::ArchiveStore::open(root, id, false)
        .map_err(|error| PreviewError::PlanFailed(error.to_string()))?;
    Ok(store
        .status()
        .map_err(|error| PreviewError::PlanFailed(error.to_string()))?
        .save_complete
        .then(|| id.to_owned()))
}

/// Arguments go through the one existing importer parser/classifier.
pub(crate) fn command(path: &str, metadata: &ImportMetadata) -> Vec<String> {
    let absent = Value::Null;
    let options = metadata
        .get("archive_options")
        .or_else(|| metadata.get("preview").map(|record| &record["options"]))
        .unwrap_or(&absent);
    let path = options["archive_manifest"].as_str().map_or_else(
        || Path::new(path).to_path_buf(),
        |manifest| Path::new(path).join(manifest),
    );
    let mut args = vec![
        "solstone".into(),
        "journal".into(),
        "importer".into(),
        "--audio-archive".into(),
        path.to_string_lossy().into_owned(),
        "--json".into(),
    ];
    if let Some(zone) = options["source_zone"].as_str() {
        args.extend(["--source-zone".into(), zone.into()]);
    }
    if let Some(zone) = options["zone"].as_str() {
        args.extend(["--preview-zone".into(), zone.into()]);
    }
    if let Some(id) = metadata.get("_archive_run_id").and_then(Value::as_str) {
        args.extend(["--resume".into(), id.into()]);
    }
    if let Some(digest) = metadata
        .get("preview")
        .and_then(|record| identity(&record["size"]["archive_identity"]))
    {
        args.extend(["--archive-identity".into(), digest.into()]);
    }
    args
}

pub(crate) fn size(value: &Value) -> Option<Value> {
    if !matches!(value["operation"].as_str(), Some("dry_run" | "status")) {
        return None;
    }
    let digest = identity(&value["archive_identity"])?;
    let mut size = value["size"].as_object()?.clone();
    for field in ["members", "source_bytes", "bytes", "days", "tiles"] {
        size.get(field)?.as_u64()?;
    }
    if !size
        .get("left_out")?
        .as_object()?
        .values()
        .all(|count| count.as_u64().is_some())
    {
        return None;
    }
    size.insert("schema".into(), json!(SIZE_SCHEMA));
    size.insert("archive_identity".into(), json!(digest));
    if value["operation"] == "status" {
        for field in [
            "total_parts",
            "saved_parts",
            "unavailable_parts",
            "rejected_parts",
            "save_complete",
        ] {
            size.insert(field.into(), value.get(field)?.clone());
        }
    }
    Some(Value::Object(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_archive_options_refuse_instead_of_selecting_other_inputs() {
        let mut metadata = ImportMetadata::new();
        metadata.insert("source_hint".into(), json!("audio_archive"));
        for manifest in [
            json!(true),
            json!(7),
            json!([]),
            json!("../outside.json"),
            json!("/outside.json"),
        ] {
            let data = json!({"source_zone":"UTC", "archive_manifest":manifest});
            assert!(
                configure_options(Path::new("unused"), &data, &mut metadata, &mut json!({}))
                    .is_err()
            );
        }
        for zone in [json!(false), json!(7), json!([]), json!("unknown-zone")] {
            let data = json!({"source_zone":zone, "archive_manifest":"nested/members.json"});
            assert!(
                configure_options(Path::new("unused"), &data, &mut metadata, &mut json!({}))
                    .is_err()
            );
        }
        let mut options = json!({});
        configure_options(
            Path::new("unused"),
            &json!({"archive_manifest":"nested/members.json"}),
            &mut metadata,
            &mut options,
        )
        .unwrap();
        assert_eq!(
            options,
            json!({"source_zone":null,"archive_manifest":"nested/members.json"})
        );
    }
}
