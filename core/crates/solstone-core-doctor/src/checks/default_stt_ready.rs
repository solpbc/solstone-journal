// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::parakeet_cpp_stt_ready,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let config = match solstone_core_journal_config::read_journal_config(&context.journal_path) {
        Ok(value) => value,
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Fail,
                super::common::config_error(&error),
                Some(copy::text("journal.cli.doctor.config.restore")),
            ));
        }
    };
    let backend = config
        .config
        .as_ref()
        .and_then(|root| root.get("transcribe"))
        .and_then(|value| value.get("backend"))
        .and_then(serde_json::Value::as_str);
    if backend.is_some_and(|backend| backend != "parakeet") {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::display(
                "journal.cli.doctor.default.stt.ready.backend",
                [("backend", &backend.unwrap())],
            ),
            None::<String>,
        ));
    }
    // Windows runs parakeet through parakeet-cpp, from the signed package.
    if (context.platform == crate::vocabulary::Platform::Linux && context.host_arch == "x86_64")
        || context.platform == crate::vocabulary::Platform::Windows
    {
        return parakeet_cpp_stt_ready::ready(context, check);
    }
    if context.platform == crate::vocabulary::Platform::Darwin && context.host_arch == "arm64" {
        return match solstone_core_system::provider_runtime::check_parakeet_coreml_cache_in(
            &context.home_dir,
            &config,
            "darwin",
            "arm64",
            context.report_form.locale(),
        ) {
            Ok(path) => Ok(make_result(
                check,
                Status::Ok,
                copy::display(
                    "journal.cli.doctor.default.stt.ready.other.model.ready",
                    [("path", &path.display())],
                ),
                None::<String>,
            )),
            Err(error) => Ok(make_result(
                check,
                Status::Warn,
                error,
                Some(copy::text("journal.cli.doctor.default.stt.ready.download")),
            )),
        };
    }
    Ok(make_result(
        check,
        Status::Skip,
        copy::text("journal.cli.doctor.default.stt.ready.unsupported"),
        None::<String>,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::test_support;
    use crate::vocabulary::{Platform, Severity};
    use solstone_core_journal_config::parakeet_coreml::{
        parakeet_coreml_model_root, parakeet_coreml_sentinel_path,
    };
    use std::fs;

    #[test]
    fn doctor_uses_the_configured_tree_and_keeps_complete_legacy_bytes_admitted() {
        let mut staged = test_support::context();
        staged.context.platform = Platform::Darwin;
        staged.context.host_arch = "arm64".to_owned();
        let first = staged.home_dir.join("first/cache");
        let second = staged.home_dir.join("second/cache");
        for cache in [&first, &second] {
            fs::create_dir_all(cache).unwrap();
            for row in solstone_core_assets::catalog()
                .iter()
                .filter(|row| row.unit == "parakeet-coreml")
            {
                let path = parakeet_coreml_model_root(cache).join(row.filename);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"intact legacy bytes from an earlier pin").unwrap();
            }
        }
        let sentinel = parakeet_coreml_sentinel_path(&staged.home_dir);
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(&sentinel, serde_json::to_vec(&serde_json::json!({
            "schema_version": 1, "backend": "parakeet", "variant": "coreml", "model_version": "v3", "quantization": "fp32", "fluidaudio_version": "0.14.0", "platform": {"os": "darwin", "arch": "arm64"}, "cache_dir": first
        })).unwrap()).unwrap();
        let path = staged.journal_path.join("config/journal.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let write_config = |cache: &std::path::Path| {
            fs::write(&path, serde_json::to_vec(&serde_json::json!({"transcribe": {"backend": "parakeet", "parakeet": {"cache_dir": cache}}})).unwrap()).unwrap()
        };
        let check = || {
            run(
                &staged,
                test_support::check("default_stt_ready", Severity::Advisory),
            )
            .unwrap()
        };
        write_config(&first);
        assert_eq!(check().status, Status::Ok);
        write_config(&second);
        assert_eq!(check().status, Status::Warn);
        write_config(&first);
        assert_eq!(check().status, Status::Ok);
        fs::remove_file(
            parakeet_coreml_model_root(&first).join("Encoder.mlmodelc/weights/weight.bin"),
        )
        .unwrap();
        assert_eq!(check().status, Status::Warn);
    }
}
