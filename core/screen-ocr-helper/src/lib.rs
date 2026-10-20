// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod protocol;
pub mod recipe;
mod worker;

use std::path::PathBuf;

pub fn run_from_args() -> Result<(), WorkerFailure> {
    let options = WorkerOptions::parse(std::env::args_os().skip(1)).map_err(|_| WorkerFailure)?;
    let recipe = recipe::Recipe::load().map_err(|_| WorkerFailure)?;
    worker::run(options, recipe).map_err(|_| WorkerFailure)
}

#[derive(Debug)]
pub(crate) struct WorkerOptions {
    pub asset_dir: PathBuf,
    pub runtime_root: PathBuf,
    pub runtime_library: PathBuf,
}

impl WorkerOptions {
    fn parse(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Self, WorkerError> {
        let mut asset_dir = None;
        let mut runtime_root = None;
        let mut runtime_library = None;
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--asset-dir") if asset_dir.is_none() => {
                    asset_dir = args.next().map(PathBuf::from);
                }
                Some("--runtime-root") if runtime_root.is_none() => {
                    runtime_root = args.next().map(PathBuf::from);
                }
                Some("--runtime-library") if runtime_library.is_none() => {
                    runtime_library = args.next().map(PathBuf::from);
                }
                _ => return Err(WorkerError::Startup),
            }
        }
        match (asset_dir, runtime_root, runtime_library) {
            (Some(asset_dir), Some(runtime_root), Some(runtime_library)) => Ok(Self {
                asset_dir,
                runtime_root,
                runtime_library,
            }),
            _ => Err(WorkerError::Startup),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerError {
    Startup,
    Protocol,
    FatalRequest,
}

#[derive(Clone, Copy, Debug)]
pub struct WorkerFailure;
