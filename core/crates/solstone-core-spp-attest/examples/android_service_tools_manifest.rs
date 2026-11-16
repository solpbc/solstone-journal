// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{env, fs, io, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).map(PathBuf::from).collect();
    let [native, resources, receipt, manifest] = args.as_slice() else {
        return Err(io::Error::other(
            "expected native directory, resource directory, build receipt and output manifest",
        )
        .into());
    };
    fs::create_dir_all(resources)?;
    for rim in solstone_core_spp_attest::nvgpu::production_rims() {
        fs::write(resources.join(rim.filename()), rim.xml())?;
    }
    let rendered =
        solstone_core_spp_attest::android_service_tools::render_android_service_tools_manifest(
            &native.canonicalize()?,
            &resources.canonicalize()?,
            &fs::read(receipt)?,
        )?;
    fs::write(manifest, rendered)?;
    Ok(())
}
