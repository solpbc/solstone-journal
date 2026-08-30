// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep descriptor-bound read hardening, caller coverage, and feature gates aligned.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{Attribute, Expr, File, Item as SynItem, ItemFn, Lit, Meta, Stmt, Token};
use toml_edit::{DocumentMut, Item, Table};

const JOURNAL_IO: &str = "solstone-core-journal-io";
const JOURNAL_IO_TEST_HOOKS: &str = "solstone-core-journal-io/test-hooks";
const SOL_LINK: &str = "solstone-core-sol-link";
const SOL_LINK_TEST_HOOKS: &str = "solstone-core-sol-link/test-hooks";
const OPTIONAL_JOURNAL_IO_TEST_HOOKS: &str = "solstone-core-journal-io?/test-hooks";
const PROTOCOL: [&str; 6] = [
    "InitialNameObserve",
    "Open",
    "OpenedHandleObserve",
    "Read",
    "FinalHandleObserve",
    "FinalNameObserve",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn read_repo_file(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn item_attributes(item: &SynItem) -> &[Attribute] {
    match item {
        SynItem::Const(item) => &item.attrs,
        SynItem::Enum(item) => &item.attrs,
        SynItem::ExternCrate(item) => &item.attrs,
        SynItem::Fn(item) => &item.attrs,
        SynItem::ForeignMod(item) => &item.attrs,
        SynItem::Impl(item) => &item.attrs,
        SynItem::Macro(item) => &item.attrs,
        SynItem::Mod(item) => &item.attrs,
        SynItem::Static(item) => &item.attrs,
        SynItem::Struct(item) => &item.attrs,
        SynItem::Trait(item) => &item.attrs,
        SynItem::TraitAlias(item) => &item.attrs,
        SynItem::Type(item) => &item.attrs,
        SynItem::Union(item) => &item.attrs,
        SynItem::Use(item) => &item.attrs,
        SynItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn cfg_predicate(meta: &Meta) -> Result<bool, String> {
    match meta {
        Meta::Path(path) if path.is_ident("test") => Ok(false),
        Meta::Path(path) if path.is_ident("unix") => Ok(true),
        Meta::Path(path) if path.is_ident("windows") => Ok(false),
        Meta::List(list) if list.path.is_ident("all") || list.path.is_ident("any") => {
            let predicates = Punctuated::<Meta, Token![,]>::parse_terminated
                .parse2(list.tokens.clone())
                .map_err(|error| format!("parse cfg predicate: {error}"))?;
            let initial = list.path.is_ident("all");
            predicates.iter().try_fold(initial, |value, predicate| {
                let next = cfg_predicate(predicate)?;
                Ok(if list.path.is_ident("all") {
                    value && next
                } else {
                    value || next
                })
            })
        }
        Meta::List(list) if list.path.is_ident("not") => {
            let predicates = Punctuated::<Meta, Token![,]>::parse_terminated
                .parse2(list.tokens.clone())
                .map_err(|error| format!("parse cfg(not(...)): {error}"))?;
            if predicates.len() != 1 {
                return Err(format!(
                    "cfg(not(...)) needs one predicate: {}",
                    list.to_token_stream()
                ));
            }
            Ok(!cfg_predicate(predicates.first().expect("one predicate"))?)
        }
        Meta::NameValue(value) if value.path.is_ident("feature") => {
            let Expr::Lit(literal) = &value.value else {
                return Err("cfg feature value is not a string literal".to_owned());
            };
            let Lit::Str(feature) = &literal.lit else {
                return Err("cfg feature value is not a string literal".to_owned());
            };
            if feature.value() == "test-hooks" {
                Ok(false)
            } else {
                Err(format!(
                    "production classifier cannot resolve feature {:?}",
                    feature.value()
                ))
            }
        }
        _ => Err(format!(
            "production classifier cannot resolve cfg predicate {}",
            meta.to_token_stream()
        )),
    }
}

fn cfg_attr_effect(attribute: &Attribute) -> Result<(bool, bool), String> {
    let arguments = attribute
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map_err(|error| format!("parse cfg_attr: {error}"))?;
    let Some(condition) = arguments.first() else {
        return Err("cfg_attr has no condition".to_owned());
    };
    if !cfg_predicate(condition)? {
        return Ok((true, false));
    }
    let mut enabled = true;
    let mut test_attribute = false;
    for applied in arguments.iter().skip(1) {
        match applied {
            Meta::Path(path) if path.is_ident("test") => test_attribute = true,
            Meta::List(list) if list.path.is_ident("cfg") => {
                let predicate = list
                    .parse_args::<Meta>()
                    .map_err(|error| format!("parse cfg_attr cfg: {error}"))?;
                enabled &= cfg_predicate(&predicate)?;
            }
            Meta::List(list) if list.path.is_ident("cfg_attr") => {
                return Err(format!(
                    "production classifier cannot resolve nested cfg_attr {}",
                    list.to_token_stream()
                ));
            }
            _ => {}
        }
    }
    Ok((enabled, test_attribute))
}

fn production_item_enabled(attributes: &[Attribute]) -> Result<bool, String> {
    let mut enabled = true;
    for attribute in attributes {
        if attribute.path().is_ident("cfg") {
            let predicate = attribute
                .parse_args::<Meta>()
                .map_err(|error| format!("parse cfg: {error}"))?;
            enabled &= cfg_predicate(&predicate)?;
        } else if attribute.path().is_ident("cfg_attr") {
            enabled &= cfg_attr_effect(attribute)?.0;
        }
    }
    Ok(enabled)
}

fn test_item(attributes: &[Attribute]) -> Result<bool, String> {
    for attribute in attributes {
        if attribute.path().is_ident("test")
            || (attribute.path().is_ident("cfg_attr") && cfg_attr_effect(attribute)?.1)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn production_file(source: &str) -> Result<File, String> {
    let mut file = syn::parse_file(source).map_err(|error| format!("parse source: {error}"))?;
    file.items = production_items(file.items)?;
    Ok(file)
}

fn production_items(items: Vec<SynItem>) -> Result<Vec<SynItem>, String> {
    let mut production = Vec::new();
    for mut item in items {
        let test_named_module = matches!(
            &item,
            SynItem::Mod(module) if module.ident == "tests" || module.ident == "bound_tests"
        );
        if test_named_module
            || test_item(item_attributes(&item))?
            || !production_item_enabled(item_attributes(&item))?
        {
            continue;
        }
        if let SynItem::Mod(module) = &mut item
            && let Some((_, nested)) = &mut module.content
        {
            *nested = production_items(std::mem::take(nested))?;
        }
        production.push(item);
    }
    Ok(production)
}

fn production_functions(file: &File) -> Vec<&ItemFn> {
    fn collect<'ast>(items: &'ast [SynItem], functions: &mut Vec<&'ast ItemFn>) {
        for item in items {
            match item {
                SynItem::Fn(function) => functions.push(function),
                SynItem::Mod(module) => {
                    if let Some((_, nested)) = &module.content {
                        collect(nested, functions);
                    }
                }
                _ => {}
            }
        }
    }

    let mut functions = Vec::new();
    collect(&file.items, &mut functions);
    functions
}

fn production_function(source: &str, name: &str) -> ItemFn {
    let file = production_file(source).unwrap_or_else(|error| panic!("classify source: {error}"));
    production_functions(&file)
        .into_iter()
        .find(|function| function.sig.ident == name)
        .cloned()
        .unwrap_or_else(|| panic!("production function {name} exists"))
}

fn without_wrappers(expression: &Expr) -> &Expr {
    match expression {
        Expr::Group(group) => without_wrappers(&group.expr),
        Expr::Paren(parenthesized) => without_wrappers(&parenthesized.expr),
        Expr::Try(tried) => without_wrappers(&tried.expr),
        _ => expression,
    }
}

fn expression_name(expression: &Expr) -> Option<String> {
    let Expr::Path(path) = without_wrappers(expression) else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn call_name(expression: &Expr) -> Option<String> {
    let Expr::Call(call) = without_wrappers(expression) else {
        return None;
    };
    expression_name(&call.func)
}

fn expression_identifier(expression: &Expr) -> Option<String> {
    match without_wrappers(expression) {
        Expr::Reference(reference) => expression_identifier(&reference.expr),
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

fn os_str_literal(expression: &Expr) -> Option<String> {
    let Expr::Call(call) = without_wrappers(expression) else {
        return None;
    };
    let Expr::Path(path) = without_wrappers(&call.func) else {
        return None;
    };
    if path.path.segments.last()?.ident != "new"
        || path.path.segments.iter().rev().nth(1)?.ident != "OsStr"
    {
        return None;
    }
    let Expr::Lit(literal) = call.args.first()? else {
        return None;
    };
    let Lit::Str(value) = &literal.lit else {
        return None;
    };
    Some(value.value())
}

#[derive(Debug, Eq, PartialEq)]
struct StructuralCall {
    function: String,
    callee: String,
    first_argument: Option<String>,
    filename: Option<String>,
}

struct CallVisitor<'a> {
    function: &'a str,
    targets: &'a [&'a str],
    calls: Vec<StructuralCall>,
}

impl<'ast> Visit<'ast> for CallVisitor<'_> {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Some(callee) = expression_name(&call.func)
            && self.targets.iter().any(|target| *target == callee)
        {
            self.calls.push(StructuralCall {
                function: self.function.to_owned(),
                callee,
                first_argument: call.args.first().and_then(expression_identifier),
                filename: call.args.iter().nth(1).and_then(os_str_literal),
            });
        }
        syn::visit::visit_expr_call(self, call);
    }
}

fn calls_in_production(file: &File, targets: &[&str]) -> Vec<StructuralCall> {
    production_functions(file)
        .into_iter()
        .flat_map(|function| {
            let name = function.sig.ident.to_string();
            let mut visitor = CallVisitor {
                function: &name,
                targets,
                calls: Vec::new(),
            };
            visitor.visit_block(&function.block);
            visitor.calls
        })
        .collect()
}

fn checkpoint(call: &syn::ExprCall) -> Option<&'static str> {
    if !matches!(
        expression_name(&call.func).as_deref(),
        Some("checkpoint") | Some("checkpoint_error")
    ) {
        return None;
    }
    let Expr::Path(path) = without_wrappers(call.args.first()?) else {
        return None;
    };
    if path.path.segments.iter().rev().nth(1)?.ident != "BoundReadPrimitive" {
        return None;
    }
    match path.path.segments.last()?.ident.to_string().as_str() {
        "InitialNameObserve" => Some("InitialNameObserve"),
        "Open" => Some("Open"),
        "OpenedHandleObserve" => Some("OpenedHandleObserve"),
        "Read" => Some("Read"),
        "FinalHandleObserve" => Some("FinalHandleObserve"),
        "FinalNameObserve" => Some("FinalNameObserve"),
        _ => None,
    }
}

fn top_level_checkpoint(statement: &Stmt) -> Option<&'static str> {
    let Stmt::Expr(expression, _) = statement else {
        return None;
    };
    let Expr::Call(call) = without_wrappers(expression) else {
        return None;
    };
    checkpoint(call)
}

struct CheckpointVisitor {
    found: Vec<&'static str>,
}

impl<'ast> Visit<'ast> for CheckpointVisitor {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Some(primitive) = checkpoint(call) {
            self.found.push(primitive);
        }
        syn::visit::visit_expr_call(self, call);
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Success {
    None,
    Some,
}

fn success(call: &syn::ExprCall) -> Option<Success> {
    if expression_name(&call.func).as_deref() != Some("Ok") || call.args.len() != 1 {
        return None;
    }
    match without_wrappers(call.args.first()?) {
        Expr::Path(path) if path.path.is_ident("None") => Some(Success::None),
        Expr::Call(call) if expression_name(&call.func).as_deref() == Some("Some") => {
            Some(Success::Some)
        }
        _ => None,
    }
}

struct SuccessVisitor {
    found: Vec<Success>,
}

impl<'ast> Visit<'ast> for SuccessVisitor {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Some(result) = success(call) {
            self.found.push(result);
        }
        syn::visit::visit_expr_call(self, call);
    }
}

fn direct_success(statement: &Stmt) -> Option<Success> {
    let Stmt::Expr(expression, _) = statement else {
        return None;
    };
    let expression = match expression {
        Expr::Return(returned) => returned.expr.as_deref()?,
        expression => expression,
    };
    let Expr::Call(call) = without_wrappers(expression) else {
        return None;
    };
    success(call)
}

fn local_match_call<'ast>(statement: &'ast Stmt, name: &str) -> Option<&'ast syn::ExprMatch> {
    let Stmt::Local(local) = statement else {
        return None;
    };
    let Expr::Match(matched) = without_wrappers(local.init.as_ref()?.expr.as_ref()) else {
        return None;
    };
    (call_name(&matched.expr).as_deref() == Some(name)).then_some(matched)
}

fn is_enoent_pattern(pattern: &syn::Pat) -> bool {
    let syn::Pat::TupleStruct(outer) = pattern else {
        return false;
    };
    let Some(syn::Pat::Path(inner)) = outer.elems.first() else {
        return false;
    };
    outer
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Err")
        && inner
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "ENOENT")
}

fn is_enoent_none_arm(matched: &syn::ExprMatch) -> bool {
    matched.arms.iter().any(|arm| {
        is_enoent_pattern(&arm.pat)
            && matches!(
                without_wrappers(&arm.body),
                Expr::Return(returned)
                    if returned.expr.as_deref().is_some_and(|expression| {
                        let Expr::Call(call) = without_wrappers(expression) else {
                            return false;
                        };
                        success(call) == Some(Success::None)
                    })
            )
    })
}

fn analyze_bound_read(function: &ItemFn) -> Result<(), String> {
    let top: Vec<_> = function
        .block
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(index, statement)| {
            top_level_checkpoint(statement).map(|value| (index, value))
        })
        .collect();
    let protocol: Vec<_> = top.iter().map(|(_, primitive)| *primitive).collect();
    if protocol != PROTOCOL {
        return Err(format!(
            "normal-flow checkpoints are {protocol:?}, not {PROTOCOL:?}"
        ));
    }
    let mut all = CheckpointVisitor { found: Vec::new() };
    all.visit_block(&function.block);
    if all.found != PROTOCOL {
        return Err(format!(
            "checkpoint calls must occur exactly once on normal flow: {:?}",
            all.found
        ));
    }

    let initial_match = function
        .block
        .stmts
        .get(top[0].0 + 1)
        .and_then(|statement| local_match_call(statement, "fstatat"))
        .ok_or_else(|| "InitialNameObserve must immediately precede fstatat".to_owned())?;
    if !is_enoent_none_arm(initial_match) {
        return Err("only the initial ENOENT arm may return Ok(None)".to_owned());
    }
    if function
        .block
        .stmts
        .get(top[1].0 + 1)
        .and_then(|statement| local_match_call(statement, "openat"))
        .is_none()
    {
        return Err("Open must immediately precede openat".to_owned());
    }

    let mut successes = SuccessVisitor { found: Vec::new() };
    successes.visit_block(&function.block);
    if successes
        .found
        .iter()
        .filter(|value| **value == Success::None)
        .count()
        != 1
    {
        return Err("exactly one Ok(None) success path is allowed".to_owned());
    }
    if successes
        .found
        .iter()
        .filter(|value| **value == Success::Some)
        .count()
        != 1
    {
        return Err("exactly one Ok(Some(...)) success path is allowed".to_owned());
    }
    let success_position = function
        .block
        .stmts
        .iter()
        .position(|statement| direct_success(statement) == Some(Success::Some))
        .ok_or_else(|| "Ok(Some(...)) must be a top-level return".to_owned())?;
    // This is linear top-level program order, deliberately not full CFG dominance.
    if success_position <= top.last().expect("six checkpoints").0 {
        return Err("Ok(Some(...)) must follow FinalNameObserve".to_owned());
    }
    Ok(())
}

fn canonical_bound_reader() -> ItemFn {
    production_function(
        &read_repo_file("core/crates/solstone-core-journal-io/src/readers.rs"),
        "read_bytes_bound",
    )
}

fn reparse(function: &ItemFn) -> ItemFn {
    syn::parse_str(&function.to_token_stream().to_string())
        .unwrap_or_else(|error| panic!("reparse fixture: {error}"))
}

fn remove_checkpoint(function: &mut ItemFn, primitive: &str) -> Stmt {
    let position = function
        .block
        .stmts
        .iter()
        .position(|statement| top_level_checkpoint(statement) == Some(primitive))
        .unwrap_or_else(|| panic!("{primitive} checkpoint exists"));
    function.block.stmts.remove(position)
}

fn assert_rejected(name: &str, function: &ItemFn, reason: &str) {
    let error = analyze_bound_read(function).expect_err(&format!("{name} must reject"));
    assert!(
        error.contains(reason),
        "{name} rejected for the wrong reason: {error}"
    );
}

#[derive(Clone, Debug)]
struct Dependency {
    package: String,
    optional: bool,
    default_features: bool,
    features: BTreeSet<String>,
}

impl Dependency {
    fn merge(&mut self, other: &Self) {
        assert_eq!(
            self.package, other.package,
            "one dependency alias names one package"
        );
        self.optional &= other.optional;
        self.default_features |= other.default_features;
        self.features.extend(other.features.iter().cloned());
    }
}

#[derive(Debug)]
struct Package {
    features: BTreeMap<String, Vec<String>>,
    dependencies: BTreeMap<String, Dependency>,
    dev_dependencies: BTreeMap<String, Dependency>,
}

impl Package {
    fn dependencies(&self, include_dev: bool) -> BTreeMap<String, Dependency> {
        let mut dependencies = self.dependencies.clone();
        if include_dev {
            for (alias, dependency) in &self.dev_dependencies {
                dependencies
                    .entry(alias.clone())
                    .and_modify(|existing| existing.merge(dependency))
                    .or_insert_with(|| dependency.clone());
            }
        }
        dependencies
    }
}

#[derive(Debug)]
struct ManifestGraph {
    packages: BTreeMap<String, Package>,
}

fn dependency_field<'a>(item: &'a Item, key: &str) -> Option<&'a toml_edit::Value> {
    item.as_inline_table()
        .and_then(|table| table.get(key))
        .or_else(|| item.as_table()?.get(key)?.as_value())
}

fn dependency_bool(item: &Item, key: &str) -> Option<bool> {
    dependency_field(item, key).and_then(toml_edit::Value::as_bool)
}

fn dependency_string(item: &Item, key: &str) -> Option<String> {
    dependency_field(item, key)
        .and_then(toml_edit::Value::as_str)
        .map(str::to_owned)
}

fn dependency_strings(item: &Item, key: &str) -> BTreeSet<String> {
    dependency_field(item, key)
        .and_then(toml_edit::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml_edit::Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn dependency_from_item(
    alias: &str,
    item: &Item,
    workspace_dependencies: &BTreeMap<String, Dependency>,
) -> Result<Dependency, String> {
    let mut dependency = if dependency_bool(item, "workspace") == Some(true) {
        workspace_dependencies
            .get(alias)
            .cloned()
            .ok_or_else(|| format!("workspace dependency {alias} is missing"))?
    } else {
        Dependency {
            package: alias.to_owned(),
            optional: false,
            default_features: true,
            features: BTreeSet::new(),
        }
    };
    if let Some(package) = dependency_string(item, "package") {
        dependency.package = package;
    }
    if let Some(optional) = dependency_bool(item, "optional") {
        dependency.optional = optional;
    }
    if let Some(default_features) = dependency_bool(item, "default-features") {
        dependency.default_features = default_features;
    }
    dependency
        .features
        .extend(dependency_strings(item, "features"));
    Ok(dependency)
}

fn extend_dependencies(
    destination: &mut BTreeMap<String, Dependency>,
    table: Option<&Table>,
    workspace_dependencies: &BTreeMap<String, Dependency>,
) -> Result<(), String> {
    let Some(table) = table else {
        return Ok(());
    };
    for (alias, item) in table.iter() {
        let dependency = dependency_from_item(alias, item, workspace_dependencies)?;
        destination
            .entry(alias.to_owned())
            .and_modify(|current| current.merge(&dependency))
            .or_insert(dependency);
    }
    Ok(())
}

fn package_features(document: &DocumentMut) -> BTreeMap<String, Vec<String>> {
    document
        .get("features")
        .and_then(Item::as_table)
        .map(|features| {
            features
                .iter()
                .filter_map(|(name, item)| {
                    item.as_array().map(|members| {
                        (
                            name.to_owned(),
                            members
                                .iter()
                                .filter_map(toml_edit::Value::as_str)
                                .map(str::to_owned)
                                .collect(),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn unix_dependency_table<'a>(document: &'a DocumentMut, kind: &str) -> Option<&'a Table> {
    document
        .get("target")
        .and_then(Item::as_table)
        .and_then(|target| target.get("cfg(unix)"))
        .and_then(Item::as_table)
        .and_then(|target| target.get(kind))
        .and_then(Item::as_table)
}

fn package_from_manifest(
    relative: &str,
    document: &DocumentMut,
    workspace_dependencies: &BTreeMap<String, Dependency>,
) -> Result<(String, Package), String> {
    let name = document
        .get("package")
        .and_then(Item::as_table)
        .and_then(|package| package.get("name"))
        .and_then(Item::as_str)
        .ok_or_else(|| format!("{relative} has no package name"))?
        .to_owned();
    let mut dependencies = BTreeMap::new();
    extend_dependencies(
        &mut dependencies,
        document.get("dependencies").and_then(Item::as_table),
        workspace_dependencies,
    )?;
    extend_dependencies(
        &mut dependencies,
        unix_dependency_table(document, "dependencies"),
        workspace_dependencies,
    )?;
    let mut dev_dependencies = BTreeMap::new();
    extend_dependencies(
        &mut dev_dependencies,
        document.get("dev-dependencies").and_then(Item::as_table),
        workspace_dependencies,
    )?;
    extend_dependencies(
        &mut dev_dependencies,
        unix_dependency_table(document, "dev-dependencies"),
        workspace_dependencies,
    )?;
    Ok((
        name,
        Package {
            features: package_features(document),
            dependencies,
            dev_dependencies,
        },
    ))
}

fn manifest_graph(
    workspace_source: &str,
    overrides: &BTreeMap<String, String>,
) -> Result<ManifestGraph, String> {
    let workspace = workspace_source
        .parse::<DocumentMut>()
        .map_err(|error| format!("parse workspace: {error}"))?;
    let workspace_dependencies = workspace
        .get("workspace")
        .and_then(Item::as_table)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Item::as_table)
        .map(|table| {
            table
                .iter()
                .map(|(alias, item)| {
                    dependency_from_item(alias, item, &BTreeMap::new())
                        .map(|dependency| (alias.to_owned(), dependency))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let members = workspace
        .get("workspace")
        .and_then(Item::as_table)
        .and_then(|workspace| workspace.get("members"))
        .and_then(Item::as_array)
        .ok_or_else(|| "workspace members are missing".to_owned())?;
    let mut packages = BTreeMap::new();
    for member in members.iter().filter_map(toml_edit::Value::as_str) {
        let relative = format!("core/{member}/Cargo.toml");
        let source = overrides
            .get(&relative)
            .cloned()
            .unwrap_or_else(|| read_repo_file(&relative));
        let document = source
            .parse::<DocumentMut>()
            .map_err(|error| format!("parse {relative}: {error}"))?;
        let (name, package) = package_from_manifest(&relative, &document, &workspace_dependencies)?;
        if packages.insert(name.clone(), package).is_some() {
            return Err(format!("workspace package {name} occurs more than once"));
        }
    }
    Ok(ManifestGraph { packages })
}

fn graph() -> ManifestGraph {
    manifest_graph(&read_repo_file("core/Cargo.toml"), &BTreeMap::new())
        .unwrap_or_else(|error| panic!("read workspace graph: {error}"))
}

#[derive(Default, Debug)]
struct ActivePackage {
    defaults: bool,
    features: BTreeSet<String>,
}

#[derive(Default, Debug)]
struct Closure {
    packages: BTreeMap<String, ActivePackage>,
    edges: BTreeSet<(String, String)>,
}

impl Closure {
    fn request(
        &mut self,
        graph: &ManifestGraph,
        package: &str,
        defaults: bool,
        features: impl IntoIterator<Item = String>,
    ) -> bool {
        if !graph.packages.contains_key(package) {
            return false;
        }
        let active = self.packages.entry(package.to_owned()).or_default();
        let before = (active.defaults, active.features.len());
        active.defaults |= defaults;
        active.features.extend(features);
        before != (active.defaults, active.features.len())
    }

    fn activate_edge(
        &mut self,
        graph: &ManifestGraph,
        owner: &str,
        alias: &str,
        include_dev: bool,
    ) -> Result<bool, String> {
        let dependency = graph
            .packages
            .get(owner)
            .ok_or_else(|| format!("unknown package {owner}"))?
            .dependencies(include_dev)
            .get(alias)
            .cloned()
            .ok_or_else(|| format!("{owner} has no dependency {alias}"))?;
        let mut changed = self.edges.insert((owner.to_owned(), alias.to_owned()));
        changed |= self.request(
            graph,
            &dependency.package,
            dependency.default_features,
            dependency.features,
        );
        Ok(changed)
    }

    fn has(&self, package: &str, feature: &str) -> bool {
        self.packages
            .get(package)
            .is_some_and(|active| active.features.contains(feature))
    }

    fn hook_packages(&self) -> BTreeSet<String> {
        self.packages
            .iter()
            .filter_map(|(name, active)| {
                active
                    .features
                    .contains("test-hooks")
                    .then_some(name.clone())
            })
            .collect()
    }
}

fn activate_feature_member(
    closure: &mut Closure,
    graph: &ManifestGraph,
    owner: &str,
    member: &str,
    include_dev: bool,
) -> Result<bool, String> {
    let dependencies = graph
        .packages
        .get(owner)
        .expect("active package exists")
        .dependencies(include_dev);
    if let Some(alias) = member.strip_prefix("dep:") {
        return closure.activate_edge(graph, owner, alias, include_dev);
    }
    if let Some((alias, feature)) = member.split_once("?/") {
        if !closure
            .edges
            .contains(&(owner.to_owned(), alias.to_owned()))
        {
            return Ok(false);
        }
        let dependency = dependencies
            .get(alias)
            .ok_or_else(|| format!("{owner} has no weak dependency {alias}"))?;
        return Ok(closure.request(graph, &dependency.package, false, [feature.to_owned()]));
    }
    if let Some((alias, feature)) = member.split_once('/') {
        let dependency = dependencies
            .get(alias)
            .ok_or_else(|| format!("{owner} has no dependency {alias}"))?;
        let mut changed = closure.activate_edge(graph, owner, alias, include_dev)?;
        changed |= closure.request(graph, &dependency.package, false, [feature.to_owned()]);
        return Ok(changed);
    }
    let package = graph.packages.get(owner).expect("active package exists");
    if package.features.contains_key(member) {
        return Ok(closure.request(graph, owner, false, [member.to_owned()]));
    }
    if dependencies
        .get(member)
        .is_some_and(|dependency| dependency.optional)
    {
        return closure.activate_edge(graph, owner, member, include_dev);
    }
    Err(format!(
        "{owner} feature member {member:?} is not resolvable"
    ))
}

fn resolve(
    graph: &ManifestGraph,
    root: &str,
    root_defaults: bool,
    root_features: &[&str],
    include_dev: bool,
) -> Result<Closure, String> {
    let mut closure = Closure::default();
    closure.request(
        graph,
        root,
        root_defaults,
        root_features.iter().map(|feature| (*feature).to_owned()),
    );
    loop {
        let mut changed = false;
        let active: Vec<_> = closure.packages.keys().cloned().collect();
        for owner in active {
            let package = graph
                .packages
                .get(&owner)
                .ok_or_else(|| format!("unknown active package {owner}"))?;
            let dependencies = package.dependencies(include_dev);
            for (alias, dependency) in &dependencies {
                if !dependency.optional {
                    changed |= closure.activate_edge(graph, &owner, alias, include_dev)?;
                }
            }
            if closure
                .packages
                .get(&owner)
                .is_some_and(|active| active.defaults)
                && package.features.contains_key("default")
            {
                changed |= closure.request(graph, &owner, false, ["default".to_owned()]);
            }
            let features: Vec<_> = closure
                .packages
                .get(&owner)
                .expect("active package remains active")
                .features
                .iter()
                .cloned()
                .collect();
            for feature in features {
                if let Some(members) = package.features.get(&feature) {
                    for member in members {
                        changed |= activate_feature_member(
                            &mut closure,
                            graph,
                            &owner,
                            member,
                            include_dev,
                        )?;
                    }
                } else if dependencies
                    .get(&feature)
                    .is_some_and(|dependency| dependency.optional)
                {
                    changed |= closure.activate_edge(graph, &owner, &feature, include_dev)?;
                } else {
                    return Err(format!("{owner} requested unknown feature {feature:?}"));
                }
            }
        }
        if !changed {
            return Ok(closure);
        }
    }
}

fn assert_no_hooks(closure: &Closure, build: &str) {
    assert!(
        !closure.has(JOURNAL_IO, "test-hooks"),
        "{build} activates {JOURNAL_IO_TEST_HOOKS}"
    );
    assert!(
        !closure.has(SOL_LINK, "test-hooks"),
        "{build} activates {SOL_LINK_TEST_HOOKS}"
    );
}

fn expected_leaves() -> BTreeSet<String> {
    [
        "config/journal.json",
        "ca/cert.pem",
        "ca/private.pem",
        "link/state.json",
        "ca/state.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn resolved_leaves() -> BTreeSet<String> {
    let config = production_file(&read_repo_file(
        "core/crates/solstone-core-journal-config/src/read.rs",
    ))
    .unwrap_or_else(|error| panic!("classify journal-config source: {error}"));
    let committed = production_file(&read_repo_file(
        "core/crates/solstone-core-sol-link/src/committed.rs",
    ))
    .unwrap_or_else(|error| panic!("classify sol-link source: {error}"));
    let mut raw = calls_in_production(&config, &["read_bytes_bound"]);
    raw.extend(calls_in_production(&committed, &["read_bytes_bound"]));
    assert_eq!(
        raw.len(),
        3,
        "three production bound-byte call expressions remain"
    );
    assert_eq!(
        raw.iter()
            .map(|call| call.function.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "read_journal_config_bound",
            "read_required_bound_file",
            "read_optional_bound_string",
        ])
    );
    assert_eq!(
        raw.iter()
            .find(|call| call.function == "read_journal_config_bound")
            .expect("config call exists")
            .filename
            .as_deref(),
        Some("journal.json")
    );

    let mut leaves = BTreeSet::from(["config/journal.json".to_owned()]);
    let wrappers = calls_in_production(
        &committed,
        &["read_required_bound_file", "read_optional_bound_string"],
    );
    assert_eq!(
        wrappers.len(),
        4,
        "four wrapper calls fan out from three raw calls"
    );
    for call in wrappers {
        let leaf = match (
            call.function.as_str(),
            call.callee.as_str(),
            call.first_argument.as_deref(),
            call.filename.as_deref(),
        ) {
            (
                "load_committed_identity_bound",
                "read_required_bound_file",
                Some("ca"),
                Some("cert.pem"),
            ) => "ca/cert.pem",
            (
                "load_committed_identity_bound",
                "read_required_bound_file",
                Some("ca"),
                Some("private.pem"),
            ) => "ca/private.pem",
            (
                "read_state_bound",
                "read_optional_bound_string",
                Some("link"),
                Some("state.json"),
            ) => "link/state.json",
            ("read_state_bound", "read_optional_bound_string", Some("ca"), Some("state.json")) => {
                "ca/state.json"
            }
            found => panic!("unmapped bound-read wrapper call: {found:?}"),
        };
        assert!(
            leaves.insert(leaf.to_owned()),
            "duplicate bound leaf {leaf}"
        );
    }
    leaves
}

fn parse_manifest(relative: &str) -> DocumentMut {
    read_repo_file(relative)
        .parse::<DocumentMut>()
        .unwrap_or_else(|error| panic!("parse {relative}: {error}"))
}

fn feature_members(document: &DocumentMut, feature: &str) -> Option<Vec<String>> {
    document
        .get("features")
        .and_then(Item::as_table)
        .and_then(|features| features.get(feature))
        .and_then(Item::as_array)
        .map(|members| {
            members
                .iter()
                .filter_map(toml_edit::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
}

fn target_dev_dependencies(document: &DocumentMut) -> &Table {
    document["target"]["cfg(unix)"]["dev-dependencies"]
        .as_table()
        .expect("Unix dev-dependencies table exists")
}

fn inline_feature_members(item: &Item) -> Vec<String> {
    item.as_inline_table()
        .and_then(|dependency| dependency.get("features"))
        .and_then(toml_edit::Value::as_array)
        .expect("dependency feature array exists")
        .iter()
        .filter_map(toml_edit::Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn test_table<'a>(document: &'a DocumentMut, name: &str) -> &'a Table {
    document
        .get("test")
        .and_then(Item::as_array_of_tables)
        .expect("test target array exists")
        .iter()
        .find(|table| table.get("name").and_then(Item::as_str) == Some(name))
        .unwrap_or_else(|| panic!("{name} test target exists"))
}

#[test]
fn bound_read_callers_cover_the_five_committed_leaf_paths() {
    assert_eq!(resolved_leaves(), expected_leaves());
}

#[test]
fn bound_reader_retains_the_six_step_identity_stability_protocol() {
    analyze_bound_read(&canonical_bound_reader()).expect("six-step protocol remains structural");
}

#[test]
fn bound_reader_protocol_rejects_missing_checkpoints() {
    let reader = canonical_bound_reader();
    for primitive in PROTOCOL {
        let mut fixture = reader.clone();
        remove_checkpoint(&mut fixture, primitive);
        assert_rejected(primitive, &reparse(&fixture), "normal-flow checkpoints");
    }
}

#[test]
fn bound_reader_protocol_rejects_order_and_success_mutations() {
    let reader = canonical_bound_reader();

    let mut initial_after_fstat = reader.clone();
    let initial = remove_checkpoint(&mut initial_after_fstat, "InitialNameObserve");
    let expected = initial_after_fstat
        .block
        .stmts
        .iter()
        .position(|statement| matches!(statement, Stmt::Local(local) if matches!(&local.pat, syn::Pat::Ident(ident) if ident.ident == "expected")))
        .expect("expected identity binding exists");
    initial_after_fstat
        .block
        .stmts
        .insert(expected + 1, initial);
    assert_rejected(
        "InitialNameObserve after fstatat",
        &reparse(&initial_after_fstat),
        "InitialNameObserve must immediately precede",
    );

    let mut open_after_openat = reader.clone();
    let open = remove_checkpoint(&mut open_after_openat, "Open");
    let fd = open_after_openat
        .block
        .stmts
        .iter()
        .position(|statement| matches!(statement, Stmt::Local(local) if matches!(&local.pat, syn::Pat::Ident(ident) if ident.ident == "fd")))
        .expect("open descriptor binding exists");
    open_after_openat.block.stmts.insert(fd + 1, open);
    assert_rejected(
        "Open after openat",
        &reparse(&open_after_openat),
        "Open must immediately precede",
    );

    let mut final_after_success = reader.clone();
    let final_name = remove_checkpoint(&mut final_after_success, "FinalNameObserve");
    let success = final_after_success
        .block
        .stmts
        .iter()
        .position(|statement| direct_success(statement) == Some(Success::Some))
        .expect("success return exists");
    let Stmt::Expr(_, semicolon) = &mut final_after_success.block.stmts[success] else {
        panic!("success is an expression statement");
    };
    *semicolon = Some(Token![;](proc_macro2::Span::call_site()));
    final_after_success
        .block
        .stmts
        .insert(success + 1, final_name);
    assert_rejected(
        "FinalNameObserve after success",
        &reparse(&final_after_success),
        "Ok(Some(...)) must follow FinalNameObserve",
    );

    let mut early_success = reader.clone();
    let final_handle = early_success
        .block
        .stmts
        .iter()
        .position(|statement| top_level_checkpoint(statement) == Some("FinalHandleObserve"))
        .expect("final-handle checkpoint exists");
    early_success
        .block
        .stmts
        .insert(final_handle, syn::parse_quote!(return Ok(Some(bytes));));
    assert_rejected(
        "early Ok(Some(bytes))",
        &reparse(&early_success),
        "exactly one Ok(Some(...))",
    );
}

#[test]
fn bound_reader_protocol_rejects_comment_and_dead_code_decoys() {
    let reader = canonical_bound_reader();
    let mut comment = reader.clone();
    remove_checkpoint(&mut comment, "Read");
    let comment = syn::parse_str::<ItemFn>(&format!(
        "{}\n// checkpoint_error(BoundReadPrimitive::Read)?;",
        comment.to_token_stream()
    ))
    .expect("comment fixture reparses");
    assert_rejected(
        "comment-only checkpoint",
        &comment,
        "normal-flow checkpoints",
    );

    let mut dead = reader.clone();
    remove_checkpoint(&mut dead, "Read");
    let success = dead
        .block
        .stmts
        .iter()
        .position(|statement| direct_success(statement) == Some(Success::Some))
        .expect("success return exists");
    dead.block.stmts.insert(
        success,
        syn::parse_quote!(if false {
            checkpoint_error(BoundReadPrimitive::Read)?;
        }),
    );
    assert_rejected(
        "dead-code checkpoint",
        &reparse(&dead),
        "normal-flow checkpoints",
    );
}

#[test]
fn production_classifier_discovers_post_test_module_callers() {
    let file = production_file(
        "#[cfg(test)] mod tests { fn fixture() { read_bytes_bound(directory, name); } }\n\
         fn after_tests() { read_bytes_bound(directory, name); }",
    )
    .expect("fixture classifies");
    assert_eq!(
        calls_in_production(&file, &["read_bytes_bound"]),
        [StructuralCall {
            function: "after_tests".to_owned(),
            callee: "read_bytes_bound".to_owned(),
            first_argument: Some("directory".to_owned()),
            filename: None,
        }]
    );
}

#[test]
fn production_classifier_excludes_test_and_macro_token_callers() {
    let test_file = production_file(
        "#[cfg(test)] mod tests { fn fixture() { read_bytes_bound(directory, name); } }",
    )
    .expect("test fixture classifies");
    assert!(
        calls_in_production(&test_file, &["read_bytes_bound"]).is_empty(),
        "in-test caller must be excluded"
    );
    let macro_file =
        production_file("fn fixture() { some_macro!(read_bytes_bound(directory, name)); }")
            .expect("macro fixture classifies");
    assert!(
        calls_in_production(&macro_file, &["read_bytes_bound"]).is_empty(),
        "macro token text is not a direct ExprCall"
    );
}

#[test]
fn production_classifier_rejects_unknown_cfg_predicates() {
    let Err(error) = production_file("#[cfg(feature = \"unknown\")] fn fixture() {}") else {
        panic!("unknown cfg must remain ambiguous");
    };
    assert!(error.contains("cannot resolve feature"));
}

#[test]
fn bound_read_feature_closures_and_leaf_process_target_remain_narrow() {
    let core = parse_manifest("core/crates/solstone-core/Cargo.toml");
    assert_eq!(
        feature_members(&core, "test-hooks"),
        Some(vec!["solstone-core-system/test-hooks".to_owned()])
    );
    assert_eq!(
        feature_members(&core, "journal-mcp-endpoint"),
        Some(vec!["dep:solstone-core-mcp-endpoint".to_owned()])
    );

    let endpoint = parse_manifest("core/crates/solstone-core-mcp-endpoint/Cargo.toml");
    assert_eq!(
        feature_members(&endpoint, "test-hooks"),
        Some(vec![
            JOURNAL_IO_TEST_HOOKS.to_owned(),
            SOL_LINK_TEST_HOOKS.to_owned(),
        ])
    );
    let leaf_process = test_table(&endpoint, "mcp_endpoint_bound_leaf_process");
    assert_eq!(
        leaf_process.get("path").and_then(Item::as_str),
        Some("tests/mcp_endpoint_bound_leaf_process.rs")
    );
    let required = leaf_process
        .get("required-features")
        .and_then(Item::as_array)
        .expect("leaf process required features exist")
        .iter()
        .filter_map(toml_edit::Value::as_str)
        .collect::<Vec<_>>();
    assert_eq!(required, ["test-hooks"]);

    let sol_link = parse_manifest("core/crates/solstone-core-sol-link/Cargo.toml");
    assert_eq!(
        feature_members(&sol_link, "test-hooks"),
        Some(vec![OPTIONAL_JOURNAL_IO_TEST_HOOKS.to_owned()])
    );

    let journal_config = parse_manifest("core/crates/solstone-core-journal-config/Cargo.toml");
    assert!(
        journal_config.get("features").is_none(),
        "journal-config must not expose a feature table"
    );
    let dependency = target_dev_dependencies(&journal_config)
        .get("solstone-core-journal-io")
        .expect("journal-io is a Unix dev dependency");
    assert_eq!(inline_feature_members(dependency), ["test-hooks"]);

    let graph = graph();
    let ordinary =
        resolve(&graph, "solstone-core", true, &[], false).expect("ordinary root closure resolves");
    assert_no_hooks(&ordinary, "ordinary root build");
    let no_defaults = resolve(&graph, "solstone-core", false, &[], false)
        .expect("no-default root closure resolves");
    assert_no_hooks(&no_defaults, "root --no-default-features build");
    let endpoint_feature = resolve(
        &graph,
        "solstone-core",
        true,
        &["journal-mcp-endpoint"],
        false,
    )
    .expect("journal-mcp-endpoint root closure resolves");
    assert_no_hooks(&endpoint_feature, "root journal-mcp-endpoint build");

    let endpoint_target = resolve(
        &graph,
        "solstone-core-mcp-endpoint",
        true,
        &["test-hooks"],
        true,
    )
    .expect("leaf-process target closure resolves");
    assert_eq!(
        endpoint_target.hook_packages(),
        BTreeSet::from([
            JOURNAL_IO.to_owned(),
            SOL_LINK.to_owned(),
            "solstone-core-mcp-endpoint".to_owned(),
        ]),
        "the process target reaches exactly its own and its two declared hook features"
    );

    assert!(
        !ordinary.has(SOL_LINK, "default"),
        "the workspace sol-link pin disables its default features"
    );
    let workspace_source = read_repo_file("core/Cargo.toml");
    let unguarded_source = workspace_source.replacen(
        "solstone-core-sol-link = { path = \"crates/solstone-core-sol-link\", default-features = false }",
        "solstone-core-sol-link = { path = \"crates/solstone-core-sol-link\" }",
        1,
    );
    assert_ne!(
        unguarded_source, workspace_source,
        "default-features guard fixture must mutate the workspace pin"
    );
    let unguarded = manifest_graph(&unguarded_source, &BTreeMap::new())
        .expect("unguarded workspace fixture resolves");
    assert!(
        resolve(&unguarded, "solstone-core", true, &[], false)
            .expect("unguarded root closure resolves")
            .has(SOL_LINK, "default"),
        "removing default-features = false must activate sol-link defaults"
    );

    let shell_relative = "core/crates/solstone-core-convey-shell/Cargo.toml";
    let shell_source = read_repo_file(shell_relative);
    let hooked_shell_source = shell_source.replacen(
        "host = [",
        "host = [\n    \"solstone-core-sol-link/test-hooks\",",
        1,
    );
    assert_ne!(
        hooked_shell_source, shell_source,
        "convey-shell host fixture must add the hook edge"
    );
    let normal_shell = resolve(&graph, "solstone-core-convey-shell", true, &[], false)
        .expect("normal convey-shell closure resolves");
    assert_no_hooks(&normal_shell, "normal convey-shell build");
    let mut overrides = BTreeMap::new();
    overrides.insert(shell_relative.to_owned(), hooked_shell_source);
    let hooked_shell = manifest_graph(&workspace_source, &overrides)
        .expect("hooked convey-shell workspace fixture resolves");
    let hooked_shell = resolve(
        &hooked_shell,
        "solstone-core-convey-shell",
        true,
        &[],
        false,
    )
    .expect("hooked convey-shell closure resolves");
    assert!(
        hooked_shell.has(SOL_LINK, "test-hooks") && hooked_shell.has(JOURNAL_IO, "test-hooks"),
        "the mutated convey-shell host edge must be detected reaching bound-read hooks (proving closure-resolution catches this leak shape)"
    );
}
