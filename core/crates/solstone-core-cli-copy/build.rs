// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{env, fs, path::PathBuf};

fn main() {
    let resources = "../../../i18n/generated/cli/rust/journal-cli";
    println!("cargo:rerun-if-changed={resources}");
    let mut locales = Vec::new();
    for entry in fs::read_dir(resources).expect("open generated CLI resources") {
        let path = entry.expect("read CLI resource").path();
        if path.extension().is_some_and(|ext| ext == "ftl") {
            locales.push(path.file_stem().unwrap().to_str().unwrap().to_owned());
        }
    }
    locales.sort();
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("locales.rs"), format!("&{locales:?}")).unwrap();
}
