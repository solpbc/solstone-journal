// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    println!("cargo:rerun-if-env-changed=SOLSTONE_RFDETR_COMPILED_EXPECTATION_RS");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let dest = std::path::Path::new(&out_dir).join("rfdetr_compiled_expectation_value.rs");
    match std::env::var("SOLSTONE_RFDETR_COMPILED_EXPECTATION_RS") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            let value = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!("read compiled rf-detr delivery contract {path}: {error}")
            });
            std::fs::write(&dest, value).expect("write compiled rf-detr delivery contract");
        }
        Err(_) => {
            std::fs::write(
                &dest,
                "pub const MACOS_DELIVERY_CONTRACT: Option<CompiledDeliveryContract> = None;\n",
            )
            .expect("write compiled rf-detr delivery contract stub");
        }
    }
}
