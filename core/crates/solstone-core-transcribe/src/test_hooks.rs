// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Narrow `#[doc(hidden)]` driver for transport integration tests.

use std::path::Path;
use std::time::Duration;

use solstone_core_spp_ratls::AttestedIo;

use crate::TranscribeError;
use crate::backend::confidential::{hosted_transcribe_transport_error, send_multipart_request};
use crate::backend::parakeet_coreml::{get_model_info_with_helper, transcribe_with_helper};
use crate::backend::parakeet_cpp::{
    HealthState, ParakeetServer, connect, probe_health, transcribe_transport_with_timeout,
};
use crate::speakers::{SpeakersAnalyzeBudget, invoke_speakers_analyze_helper};

#[doc(hidden)]
pub fn recorded_convey_is_up(journal_path: &Path) -> bool {
    crate::args::is_solstone_up(journal_path)
}

#[doc(hidden)]
pub enum ConfidentialMultipart {
    Unreachable { reason: String },
    Received { status: u16, body: Vec<u8> },
}

#[doc(hidden)]
pub fn confidential_multipart_exchange(
    stream: &mut dyn AttestedIo,
    host: &str,
    bearer: Option<&str>,
    wav: &[u8],
    timeout: Duration,
) -> ConfidentialMultipart {
    match send_multipart_request(stream, host, bearer, wav, timeout) {
        Ok(response) => ConfidentialMultipart::Received {
            status: response.status,
            body: response.body,
        },
        Err(error) => {
            let TranscribeError::ConfidentialDeferred { reason, .. } =
                hosted_transcribe_transport_error(error)
            else {
                unreachable!("hosted transport errors are confidential deferrals");
            };
            ConfidentialMultipart::Unreachable { reason }
        }
    }
}

#[doc(hidden)]
pub enum ParakeetHealth {
    Ready,
    NotReady,
}

#[doc(hidden)]
pub fn parakeet_probe_health(base_url: &str, timeout: Duration) -> ParakeetHealth {
    match probe_health(base_url, timeout) {
        HealthState::Ready => ParakeetHealth::Ready,
        HealthState::NotReady => ParakeetHealth::NotReady,
    }
}

#[doc(hidden)]
pub enum ParakeetConnect {
    Ready,
    Deferred { reason: String },
}

#[doc(hidden)]
pub fn parakeet_connect(journal_path: &Path) -> ParakeetConnect {
    match connect(journal_path) {
        Ok(_) => ParakeetConnect::Ready,
        Err(TranscribeError::ParakeetCppDeferred { reason, .. }) => {
            ParakeetConnect::Deferred { reason }
        }
        Err(error) => panic!("unexpected parakeet connect outcome: {error}"),
    }
}

#[doc(hidden)]
pub struct ParakeetWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub probability: f64,
}

#[doc(hidden)]
pub enum ParakeetTranscribe {
    Ok {
        words: Vec<ParakeetWord>,
        text: String,
    },
    Deferred {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

#[doc(hidden)]
pub fn parakeet_transcribe(base_url: &str, wav: &[u8], timeout: Duration) -> ParakeetTranscribe {
    let server = ParakeetServer {
        port: 0,
        base_url: base_url.to_owned(),
        auth_headers: None,
    };
    match transcribe_transport_with_timeout(&server, wav, timeout) {
        Ok(response) => ParakeetTranscribe::Ok {
            words: response
                .words
                .into_iter()
                .map(|word| ParakeetWord {
                    word: word.word,
                    start: word.start,
                    end: word.end,
                    probability: word.probability,
                })
                .collect(),
            text: response.text,
        },
        Err(TranscribeError::ParakeetCppDeferred { reason, .. }) => {
            ParakeetTranscribe::Deferred { reason }
        }
        Err(TranscribeError::ParakeetCppFailure { reason, .. }) => {
            ParakeetTranscribe::Failed { reason }
        }
        Err(error) => panic!("unexpected parakeet transcribe outcome: {error}"),
    }
}

#[doc(hidden)]
pub enum SpeakerInvoke {
    Completed {
        returncode: i32,
        stdout: String,
        stderr: String,
    },
    Failed {
        reason: String,
        native_exit_code: Option<i32>,
    },
}

#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn invoke_speakers_program(
    program: &Path,
    request: &[u8],
    raw_path: &Path,
    timeout: Duration,
    stdout_limit_bytes: usize,
    stderr_limit_bytes: usize,
    terminate_grace: Duration,
    kill_grace: Duration,
) -> SpeakerInvoke {
    let budget = SpeakersAnalyzeBudget {
        timeout,
        stdout_limit_bytes,
        stderr_limit_bytes,
        terminate_grace,
        kill_grace,
    };
    match invoke_speakers_analyze_helper(program, request, raw_path, budget) {
        Ok(completed) => SpeakerInvoke::Completed {
            returncode: completed.returncode,
            stdout: completed.stdout,
            stderr: completed.stderr,
        },
        Err(error) => SpeakerInvoke::Failed {
            reason: error.reason,
            native_exit_code: error.native_exit_code,
        },
    }
}

#[doc(hidden)]
pub struct CoremlWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub probability: f64,
}

#[doc(hidden)]
pub enum CoremlTranscribe {
    Ok {
        words: Vec<CoremlWord>,
        text: String,
    },
    Deferred {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

#[doc(hidden)]
pub fn coreml_transcribe_with_helper(
    audio: &[f32],
    helper: &Path,
    cache_dir: &Path,
    model_version: &str,
    timeout: Duration,
) -> CoremlTranscribe {
    match transcribe_with_helper(audio, helper, cache_dir, model_version, timeout) {
        Ok(response) => CoremlTranscribe::Ok {
            words: response
                .words
                .into_iter()
                .map(|word| CoremlWord {
                    word: word.word,
                    start: word.start,
                    end: word.end,
                    probability: word.probability,
                })
                .collect(),
            text: response.text,
        },
        Err(TranscribeError::ParakeetCoremlDeferred { reason, .. }) => {
            CoremlTranscribe::Deferred { reason }
        }
        Err(TranscribeError::ParakeetCoremlFailure { reason, .. }) => {
            CoremlTranscribe::Failed { reason }
        }
        Err(error) => panic!("unexpected CoreML transcribe outcome: {error}"),
    }
}

#[doc(hidden)]
pub enum CoremlModelInfo {
    Ok {
        model: String,
        device: String,
        compute_type: String,
    },
    Failed {
        reason: String,
    },
}

#[doc(hidden)]
pub fn coreml_get_model_info(
    helper: &Path,
    model_version: &str,
    timeout: Duration,
) -> CoremlModelInfo {
    match get_model_info_with_helper(helper, model_version, timeout) {
        Ok(info) => CoremlModelInfo::Ok {
            model: info.model,
            device: info.device,
            compute_type: info.compute_type,
        },
        Err(TranscribeError::ParakeetCoremlFailure { reason, .. }) => {
            CoremlModelInfo::Failed { reason }
        }
        Err(error) => panic!("unexpected CoreML version-probe outcome: {error}"),
    }
}
