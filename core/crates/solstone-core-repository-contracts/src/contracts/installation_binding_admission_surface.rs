// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::{FnArg, ImplItem, Item, ReturnType, Type, Visibility};

const TYPE_NAME: &str = "InstallationBindingAdmission";

#[derive(Debug, Eq, PartialEq)]
struct PublicSurface {
    public_fields: Vec<String>,
    public_methods: Vec<String>,
    trait_implementations: Vec<String>,
    binding_signature: Option<String>,
    struct_count: usize,
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn type_is_target(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == TYPE_NAME)
}

fn public_surface(source: &str) -> PublicSurface {
    let syntax = syn::parse_file(source).expect("installation identity source parses");
    let mut surface = PublicSurface {
        public_fields: Vec::new(),
        public_methods: Vec::new(),
        trait_implementations: Vec::new(),
        binding_signature: None,
        struct_count: 0,
    };
    for item in syntax.items {
        match item {
            Item::Struct(item) if item.ident == TYPE_NAME => {
                surface.struct_count += 1;
                for field in item.fields {
                    if matches!(field.vis, Visibility::Public(_)) {
                        surface.public_fields.push(
                            field
                                .ident
                                .map_or_else(|| "<tuple>".to_owned(), |ident| ident.to_string()),
                        );
                    }
                }
            }
            Item::Impl(implementation) if type_is_target(&implementation.self_ty) => {
                if let Some((_, path, _)) = implementation.trait_ {
                    surface.trait_implementations.push(
                        path.segments
                            .last()
                            .expect("trait path has a segment")
                            .ident
                            .to_string(),
                    );
                    continue;
                }
                for member in implementation.items {
                    let ImplItem::Fn(function) = member else {
                        continue;
                    };
                    if !matches!(function.vis, Visibility::Public(_)) {
                        continue;
                    }
                    let name = function.sig.ident.to_string();
                    if name == "binding" {
                        let receiver = function
                            .sig
                            .inputs
                            .first()
                            .and_then(|argument| match argument {
                                FnArg::Receiver(receiver) => Some(receiver.to_token_stream()),
                                FnArg::Typed(_) => None,
                            })
                            .expect("binding has a receiver");
                        let output = match function.sig.output {
                            ReturnType::Default => panic!("binding must return the binding"),
                            ReturnType::Type(_, output) => output.to_token_stream(),
                        };
                        surface.binding_signature = Some(format!("{} -> {}", receiver, output));
                    }
                    surface.public_methods.push(name);
                }
            }
            _ => {}
        }
    }
    surface.public_fields.sort();
    surface.public_methods.sort();
    surface.trait_implementations.sort();
    surface
}

fn assert_exact_read_only_surface(source: &str) {
    let surface = public_surface(source);
    assert_eq!(
        surface.struct_count, 1,
        "retained admission type count drifted"
    );
    assert!(
        surface.public_fields.is_empty(),
        "retained admission exposed public fields: {:?}",
        surface.public_fields
    );
    assert_eq!(
        surface.public_methods,
        ["binding"],
        "retained admission public methods drifted"
    );
    assert!(
        surface.trait_implementations.is_empty(),
        "retained admission exposed a trait surface: {:?}",
        surface.trait_implementations
    );
    assert_eq!(
        surface.binding_signature.as_deref(),
        Some("& self -> & InstallationBinding"),
        "binding must remain an immutable borrow"
    );
}

#[test]
fn retained_installation_binding_admission_has_one_exact_read_only_surface() {
    let source = fs::read_to_string(
        repository_root().join("core/crates/solstone-core-installation-identity/src/lib.rs"),
    )
    .expect("read installation identity source");
    assert_exact_read_only_surface(&source);
}

#[test]
#[should_panic(expected = "exposed public fields")]
fn public_field_falsification_reddens_the_contract() {
    assert_exact_read_only_surface(
        "pub struct InstallationBindingAdmission { pub raw: usize }\n\
         impl InstallationBindingAdmission { pub fn binding(&self) -> &InstallationBinding { todo!() } }",
    );
}

#[test]
#[should_panic(expected = "public methods drifted")]
fn mutation_method_falsification_reddens_the_contract() {
    assert_exact_read_only_surface(
        "pub struct InstallationBindingAdmission { raw: usize }\n\
         impl InstallationBindingAdmission {\n\
           pub fn binding(&self) -> &InstallationBinding { todo!() }\n\
           pub fn commit(&mut self) {}\n\
         }",
    );
}

#[test]
#[should_panic(expected = "trait surface")]
fn trait_escape_falsification_reddens_the_contract() {
    assert_exact_read_only_surface(
        "pub struct InstallationBindingAdmission { raw: usize }\n\
         impl InstallationBindingAdmission { pub fn binding(&self) -> &InstallationBinding { todo!() } }\n\
         impl AsMut<InstallationBinding> for InstallationBindingAdmission {\n\
           fn as_mut(&mut self) -> &mut InstallationBinding { todo!() }\n\
         }",
    );
}
