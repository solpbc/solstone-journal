// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};

pub const RECIPE_TOML: &str = include_str!("../recipe.toml");

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub protocol_version: u16,
    pub source: SourceRecipe,
    pub settings: Settings,
    pub limits: Limits,
    pub models: Models,
    pub runtime: RuntimeRecipe,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRecipe {
    pub oar_git: String,
    pub oar_revision: String,
    pub oar_default_features: bool,
    pub oar_features: Vec<String>,
    pub ort_version: String,
    pub ort_features: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub cpu_only: bool,
    pub telemetry: bool,
    pub ort_intra_threads: usize,
    pub ort_inter_threads: usize,
    pub rayon_threads: usize,
    pub image_batch_size: usize,
    pub region_batch_size: usize,
    pub detector_score_threshold: f32,
    pub detector_box_threshold: f32,
    pub detector_unclip_ratio: f32,
    pub detector_max_candidates: usize,
    pub detector_limit_side_len: u32,
    pub detector_limit_type: String,
    pub detector_max_side_len: u32,
    pub recognizer_score_threshold: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_header_bytes: usize,
    pub max_png_bytes: usize,
    pub max_decoded_pixels: u64,
    pub max_dictionary_bytes: usize,
    pub max_regions: usize,
    pub max_response_bytes: usize,
    pub max_text_bytes: usize,
    pub max_geometry_points: usize,
    pub max_request_id_bytes: usize,
    pub max_diagnostic_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub detector: ModelAsset,
    pub recognizer: ModelAsset,
    pub dictionary: DictionaryAsset,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAsset {
    pub filename: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DictionaryAsset {
    pub filename: String,
    pub source_url: String,
    pub source_sha256: String,
    pub materialized_entries: usize,
    pub materialized_final_newline: bool,
    pub materialized_sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRecipe {
    pub version: String,
    pub linux_max_glibc: String,
    pub linux_build_lane: String,
    pub linux_x86_64: RuntimeArchive,
    pub macos_arm64: RuntimeArchive,
    pub linux_aarch64: RuntimeArchive,
    pub windows_x86_64: RuntimeArchive,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeArchive {
    pub filename: String,
    pub url: String,
    pub sha256: String,
}

impl Recipe {
    pub fn load() -> Result<Self, toml::de::Error> {
        toml::from_str(RECIPE_TOML)
    }

    pub fn identity(&self) -> String {
        crate::protocol::sha256_hex(RECIPE_TOML.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_recipe_has_the_required_pins_and_bounds() {
        let recipe = Recipe::load().expect("recipe should parse");
        assert_eq!(recipe.schema_version, 1);
        assert_eq!(recipe.protocol_version, 1);
        assert_eq!(recipe.source.oar_git, "https://github.com/GreatV/oar-ocr");
        assert_eq!(
            recipe.source.oar_revision,
            "84817f58d7a0932d5368a9fa18105ba5d842af29"
        );
        assert!(!recipe.source.oar_default_features);
        assert_eq!(recipe.source.oar_features, ["simd".to_string()]);
        assert_eq!(recipe.source.ort_version, "2.0.0-rc.13");
        assert_eq!(
            recipe.source.ort_features,
            ["std".to_string(), "load-dynamic".to_string()]
        );

        assert!(recipe.settings.cpu_only);
        assert!(!recipe.settings.telemetry);
        assert_eq!(recipe.settings.ort_intra_threads, 1);
        assert_eq!(recipe.settings.ort_inter_threads, 1);
        assert_eq!(recipe.settings.rayon_threads, 1);
        assert_eq!(recipe.settings.image_batch_size, 1);
        assert_eq!(recipe.settings.region_batch_size, 1);
        assert_eq!(recipe.settings.detector_score_threshold, 0.2);
        assert_eq!(recipe.settings.detector_box_threshold, 0.4);
        assert_eq!(recipe.settings.detector_unclip_ratio, 1.4);
        assert_eq!(recipe.settings.detector_max_candidates, 3000);
        assert_eq!(recipe.settings.detector_limit_side_len, 1920);
        assert_eq!(recipe.settings.detector_limit_type, "max");
        assert_eq!(recipe.settings.detector_max_side_len, 4096);
        assert_eq!(recipe.settings.recognizer_score_threshold, 0.0);

        assert_eq!(recipe.limits.max_header_bytes, 4096);
        assert_eq!(recipe.limits.max_png_bytes, 32 * 1024 * 1024);
        assert_eq!(recipe.limits.max_decoded_pixels, 40_000_000);
        assert_eq!(recipe.limits.max_dictionary_bytes, 1024 * 1024);
        assert_eq!(recipe.limits.max_regions, 3000);
        assert_eq!(recipe.limits.max_response_bytes, 32 * 1024 * 1024);
        assert_eq!(recipe.limits.max_text_bytes, 4096);
        assert_eq!(recipe.limits.max_geometry_points, 16);
        assert_eq!(recipe.limits.max_request_id_bytes, 128);
        assert_eq!(recipe.limits.max_diagnostic_bytes, 512);

        assert_eq!(recipe.models.detector.filename, "detector.onnx");
        assert_eq!(
            recipe.models.detector.url,
            "https://huggingface.co/PaddlePaddle/PP-OCRv6_tiny_det_onnx/resolve/2ba1506c0380b8f0b03dd142459aac66d4421f6c/inference.onnx"
        );
        assert_eq!(
            recipe.models.detector.sha256,
            "193bab7a04fca699a6c82e6abb5b81bdb28177f0abd4062552b04908dafb19f8"
        );
        assert_eq!(recipe.models.recognizer.filename, "recognizer.onnx");
        assert_eq!(
            recipe.models.recognizer.url,
            "https://huggingface.co/PaddlePaddle/PP-OCRv6_tiny_rec_onnx/resolve/2612ab37152ae0a677521bae4e1e3d4fb4cf7c30/inference.onnx"
        );
        assert_eq!(
            recipe.models.recognizer.sha256,
            "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6"
        );
        assert_eq!(recipe.models.dictionary.filename, "dictionary.txt");
        assert_eq!(
            recipe.models.dictionary.source_url,
            "https://huggingface.co/PaddlePaddle/PP-OCRv6_tiny_rec_onnx/resolve/2612ab37152ae0a677521bae4e1e3d4fb4cf7c30/inference.yml"
        );
        assert_eq!(
            recipe.models.dictionary.source_sha256,
            "66170210bad538e83fff3c4a3867e547d6bf20b50d64b20347c4b913f3034ea1"
        );
        assert_eq!(recipe.models.dictionary.materialized_entries, 6904);
        assert!(recipe.models.dictionary.materialized_final_newline);
        assert_eq!(
            recipe.models.dictionary.materialized_sha256,
            "c5cbe34ef40c29c4df07ed012bf96569cb69a2d2a01a07027e9f13cb832bd9cd"
        );

        assert_eq!(recipe.runtime.version, "1.31.0");
        assert_eq!(recipe.runtime.linux_max_glibc, "2.34");
        assert_eq!(recipe.runtime.linux_build_lane, "GNU Zig");
        assert_eq!(
            recipe.runtime.linux_x86_64,
            RuntimeArchive {
                filename: "onnxruntime-1.31.0-cp313-cp313-manylinux_2_28_x86_64.whl".into(),
                url: "https://files.pythonhosted.org/packages/0d/ac/67ebbaab4b3083f2a6b27ee6c4aa400c7f8d6c72b5499aac7e4cd6ba74f5/onnxruntime-1.31.0-cp313-cp313-manylinux_2_28_x86_64.whl".into(),
                sha256: "37fd78cee5160c7a43a1730ccb3682ffd880af9c9e80385d625c0c2f8b125809".into(),
            }
        );
        assert_eq!(
            recipe.runtime.macos_arm64,
            RuntimeArchive {
                filename: "onnxruntime-1.31.0-cp313-cp313-macosx_14_0_arm64.whl".into(),
                url: "https://files.pythonhosted.org/packages/e0/2b/117f94d73a3bac4276c285c47e384e1b3ea67b191aa4c7592df9d3f4a136/onnxruntime-1.31.0-cp313-cp313-macosx_14_0_arm64.whl".into(),
                sha256: "0ba02a44acb6203040354d9a1f160e3f37a43feac7bb05caa3e0ea545efed505".into(),
            }
        );
        assert_eq!(
            recipe.runtime.linux_aarch64,
            RuntimeArchive {
                filename: "onnxruntime-1.31.0-cp313-cp313-manylinux_2_28_aarch64.whl".into(),
                url: "https://files.pythonhosted.org/packages/8a/d0/3677fe93ec0fa3c637744aa4c3ae6ef89a93ee229cd3c5157820f267c7bd/onnxruntime-1.31.0-cp313-cp313-manylinux_2_28_aarch64.whl".into(),
                sha256: "ad663106f6eeff3d454f24a786450459d07f30e74863851104fc1b8b3f368127".into(),
            }
        );
        assert_eq!(
            recipe.runtime.windows_x86_64,
            RuntimeArchive {
                filename: "onnxruntime-1.31.0-cp313-cp313-win_amd64.whl".into(),
                url: "https://files.pythonhosted.org/packages/c4/86/05ed2056f43b27aaf12ebc592ebd9037a26bed315958cf882f43425fd469/onnxruntime-1.31.0-cp313-cp313-win_amd64.whl".into(),
                sha256: "73e0165d58ece068c2a8a1c477c90b38e5a8adbbd399fdfdfd4bd79cbc28ff8d".into(),
            }
        );
    }
}
