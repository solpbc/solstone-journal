// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use ort::ep::{CPU, CoreML, ExecutionProviderDispatch};
use ort::session::Session;
use ort::value::{TensorElementType, ValueType};

use crate::{SpeakerExecutionProvider, SpeakerOnnxError};

pub(crate) fn open_session(
    model_path: &Path,
    providers: &[SpeakerExecutionProvider],
) -> Result<Session, SpeakerOnnxError> {
    if providers.is_empty() {
        return Err(SpeakerOnnxError::EmptyProviderPlan);
    }
    let dispatches = provider_dispatches(providers)?;
    Ok(Session::builder()?
        .with_execution_providers(dispatches)?
        .commit_from_file(model_path)?)
}

fn provider_dispatches(
    providers: &[SpeakerExecutionProvider],
) -> Result<Vec<ExecutionProviderDispatch>, SpeakerOnnxError> {
    let mut dispatches = Vec::with_capacity(providers.len());
    for provider in providers {
        match provider {
            SpeakerExecutionProvider::CoreMl => {
                if !cfg!(target_vendor = "apple") {
                    return Err(SpeakerOnnxError::ProviderUnavailable { provider: "coreml" });
                }
                dispatches.push(CoreML::default().build());
            }
            SpeakerExecutionProvider::Cpu => {
                dispatches.push(CPU::default().build());
            }
        }
    }
    Ok(dispatches)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExpectedDim {
    Any,
    Exact(i64),
}

pub(crate) fn expect_tensor(
    label: &str,
    name: &str,
    value_type: &ValueType,
    expected_name: &str,
    expected_shape: &[ExpectedDim],
) -> Result<(), SpeakerOnnxError> {
    if name != expected_name {
        return Err(SpeakerOnnxError::InvalidModelIo {
            detail: format!("{label} name {name:?} is not {expected_name:?}"),
        });
    }
    let ValueType::Tensor { ty, shape, .. } = value_type else {
        return Err(SpeakerOnnxError::InvalidModelIo {
            detail: format!("{label} {name:?} is not a tensor"),
        });
    };
    if *ty != TensorElementType::Float32 {
        return Err(SpeakerOnnxError::InvalidModelIo {
            detail: format!("{label} {name:?} is {ty}, not float32"),
        });
    }
    if shape.len() != expected_shape.len() {
        return Err(SpeakerOnnxError::InvalidModelIo {
            detail: format!("{label} {name:?} shape {shape} has wrong rank"),
        });
    }
    for (index, (actual, expected)) in shape.iter().zip(expected_shape).enumerate() {
        match expected {
            ExpectedDim::Any => {}
            ExpectedDim::Exact(value) if actual == value => {}
            ExpectedDim::Exact(value) => {
                return Err(SpeakerOnnxError::InvalidModelIo {
                    detail: format!("{label} {name:?} dim {index} is {actual}, not {value}"),
                });
            }
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn coreml_provider_open_is_rejected_on_non_apple_builds() {
        if cfg!(target_vendor = "apple") {
            return;
        }
        let error = provider_dispatches(&[SpeakerExecutionProvider::CoreMl]).unwrap_err();
        assert_eq!(
            error,
            SpeakerOnnxError::ProviderUnavailable { provider: "coreml" }
        );
    }
}
