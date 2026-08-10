// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Transcription configuration extracted from the journal's JSON object.

use std::path::Path;

use solstone_core_journal_config::{ConfigLoadError, JournalConfigRead, read_journal_config};

/// Read the journal configuration used by the transcription stage.
pub(crate) fn read_transcribe_config(
    journal_path: &Path,
) -> Result<JournalConfigRead, ConfigLoadError> {
    read_journal_config(journal_path)
}

/// Whether confidential audio handling is enabled for transcription.
pub(crate) fn confidential_audio_enabled(config: &JournalConfigRead) -> bool {
    config
        .config
        .as_ref()
        .and_then(|root| root.get("transcribe"))
        .and_then(|transcribe| transcribe.as_object())
        .and_then(|transcribe| transcribe.get("confidential_audio"))
        .is_none_or(|value| value.as_bool().unwrap_or(false))
}

/// Minimum detected speech duration before processing continues.
pub(crate) fn min_speech_seconds(config: &JournalConfigRead) -> f64 {
    config
        .config
        .as_ref()
        .and_then(|root| root.get("transcribe"))
        .and_then(|transcribe| transcribe.as_object())
        .and_then(|transcribe| transcribe.get("min_speech_seconds"))
        .and_then(|value| value.as_f64())
        .unwrap_or(1.0)
}

/// Whether all raw audio is retained after successful processing.
pub(crate) fn preserve_all(config: &JournalConfigRead) -> bool {
    config
        .config
        .as_ref()
        .and_then(|root| root.get("transcribe"))
        .and_then(|transcribe| transcribe.as_object())
        .and_then(|transcribe| transcribe.get("preserve_all"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Optional Parakeet C++ device preference.
pub(crate) fn parakeet_cpp_device(config: &JournalConfigRead) -> Option<String> {
    config
        .config
        .as_ref()
        .and_then(|root| root.get("transcribe"))
        .and_then(|transcribe| transcribe.as_object())
        .and_then(|transcribe| transcribe.get("parakeet-cpp"))
        .and_then(|parakeet| parakeet.as_object())
        .and_then(|parakeet| parakeet.get("device"))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        confidential_audio_enabled, min_speech_seconds, parakeet_cpp_device, preserve_all,
        read_transcribe_config,
    };
    use solstone_core_journal_config::JournalConfigRead;
    use std::fs;

    #[test]
    fn reads_absent_journal_config() {
        let temporary = tempfile::tempdir().unwrap();

        assert!(
            read_transcribe_config(temporary.path())
                .unwrap()
                .config
                .is_none()
        );
    }

    #[test]
    fn reads_present_journal_config() {
        let config = read_config("{\"preserve_all\":true}");

        assert!(preserve_all(&config));
    }

    #[test]
    fn confidential_audio_defaults_true_and_invalid_values_fail_closed() {
        assert!(confidential_audio_enabled(&config(None)));
        assert!(!confidential_audio_enabled(&config(Some("false"))));
        assert!(!confidential_audio_enabled(&config(Some("\"enabled\""))));
    }

    #[test]
    fn min_speech_seconds_uses_valid_nondefault_and_rejects_invalid_values() {
        assert_eq!(min_speech_seconds(&config(None)), 1.0);
        assert_eq!(
            min_speech_seconds(&config_with("min_speech_seconds", "2.75")),
            2.75
        );
        assert_eq!(
            min_speech_seconds(&config_with("min_speech_seconds", "\"2.75\"")),
            1.0
        );
    }

    #[test]
    fn preserve_all_defaults_false_and_rejects_invalid_values() {
        assert!(!preserve_all(&config(None)));
        assert!(preserve_all(&config_with("preserve_all", "true")));
        assert!(!preserve_all(&config_with("preserve_all", "1")));
    }

    #[test]
    fn parakeet_cpp_device_reads_only_its_device_key() {
        assert_eq!(parakeet_cpp_device(&config(None)), None);
        assert_eq!(
            parakeet_cpp_device(&config_with_parakeet_cpp("\"gpu\"")),
            Some("gpu".to_owned())
        );
        assert_eq!(parakeet_cpp_device(&config_with_parakeet_cpp("1")), None);

        let config = read_config(
            r#"{"parakeet":{"device":"cpu","timeout_sec":10},"parakeet-cpp":{"timeout_sec":20}}"#,
        );
        assert_eq!(parakeet_cpp_device(&config), None);
    }

    fn config(confidential_audio: Option<&str>) -> JournalConfigRead {
        let field = confidential_audio
            .map(|value| format!("\"confidential_audio\":{value}"))
            .unwrap_or_default();
        read_config(&format!("{{{field}}}"))
    }

    fn config_with(key: &str, value: &str) -> JournalConfigRead {
        read_config(&format!("{{\"{key}\":{value}}}"))
    }

    fn config_with_parakeet_cpp(device: &str) -> JournalConfigRead {
        read_config(&format!("{{\"parakeet-cpp\":{{\"device\":{device}}}}}"))
    }

    fn read_config(transcribe: &str) -> JournalConfigRead {
        let temporary = tempfile::tempdir().unwrap();
        let config_directory = temporary.path().join("config");
        fs::create_dir_all(&config_directory).unwrap();
        fs::write(
            config_directory.join("journal.json"),
            format!("{{\"transcribe\":{transcribe}}}"),
        )
        .unwrap();
        read_transcribe_config(temporary.path()).unwrap()
    }
}
