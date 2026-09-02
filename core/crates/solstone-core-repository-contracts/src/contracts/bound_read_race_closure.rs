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
const MCP_ENDPOINT: &str = "solstone-core-mcp-endpoint";
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
            if matches!(feature.value().as_str(), "test-hooks" | "full-tests") {
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

pub(crate) fn production_file(source: &str) -> Result<File, String> {
    let mut file = syn::parse_file(source).map_err(|error| format!("parse source: {error}"))?;
    file.items = production_items(file.items)?;
    Ok(file)
}

fn production_items(items: Vec<SynItem>) -> Result<Vec<SynItem>, String> {
    let mut production = Vec::new();
    for mut item in items {
        if test_item(item_attributes(&item))? || !production_item_enabled(item_attributes(&item))? {
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
    targets: &'a [&'a str],
    aliases: BTreeMap<String, String>,
    context: Vec<String>,
    calls: Vec<StructuralCall>,
    errors: Vec<String>,
}

impl<'ast> Visit<'ast> for CallVisitor<'_> {
    fn visit_item_fn(&mut self, function: &'ast ItemFn) {
        self.context.push(function.sig.ident.to_string());
        syn::visit::visit_item_fn(self, function);
        self.context.pop();
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        self.context.push(function.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, function);
        self.context.pop();
    }

    fn visit_trait_item_fn(&mut self, function: &'ast syn::TraitItemFn) {
        self.context.push(function.sig.ident.to_string());
        syn::visit::visit_trait_item_fn(self, function);
        self.context.pop();
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        self.context.push(format!("const {}", item.ident));
        syn::visit::visit_item_const(self, item);
        self.context.pop();
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        self.context.push(format!("static {}", item.ident));
        syn::visit::visit_item_static(self, item);
        self.context.pop();
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        collect_target_aliases(&item.tree, self.targets, &mut self.aliases);
        syn::visit::visit_item_use(self, item);
    }

    fn visit_macro(&mut self, item: &'ast syn::Macro) {
        if self
            .targets
            .iter()
            .any(|target| tokens_contain_ident(item.tokens.clone(), target))
        {
            self.errors.push(format!(
                "unclassified target-bearing production macro in {}",
                self.context.last().map(String::as_str).unwrap_or("item")
            ));
        }
        syn::visit::visit_macro(self, item);
    }

    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        let tokens = attribute.meta.to_token_stream();
        if self
            .targets
            .iter()
            .any(|target| tokens_contain_ident(tokens.clone(), target))
        {
            self.errors.push(format!(
                "unclassified target-bearing production attribute in {}",
                self.context.last().map(String::as_str).unwrap_or("item")
            ));
        }
        syn::visit::visit_attribute(self, attribute);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Some(name) = expression_name(&call.func) {
            let callee = if self.targets.iter().any(|target| *target == name) {
                Some(name)
            } else {
                self.aliases.get(&name).cloned()
            };
            if let Some(callee) = callee {
                self.calls.push(StructuralCall {
                    function: self
                        .context
                        .last()
                        .cloned()
                        .unwrap_or_else(|| "item".to_owned()),
                    callee,
                    first_argument: call.args.first().and_then(expression_identifier),
                    filename: call.args.iter().nth(1).and_then(os_str_literal),
                });
            }
        }
        syn::visit::visit_expr_call(self, call);
    }
}

fn collect_target_aliases(
    tree: &syn::UseTree,
    targets: &[&str],
    aliases: &mut BTreeMap<String, String>,
) {
    match tree {
        syn::UseTree::Name(name) => {
            let value = name.ident.to_string();
            if targets.iter().any(|target| *target == value) {
                aliases.insert(value.clone(), value);
            }
        }
        syn::UseTree::Rename(rename) => {
            let source = rename.ident.to_string();
            if targets.iter().any(|target| *target == source) {
                aliases.insert(rename.rename.to_string(), source);
            }
        }
        syn::UseTree::Path(path) => collect_target_aliases(&path.tree, targets, aliases),
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                collect_target_aliases(tree, targets, aliases);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

fn tokens_contain_ident(tokens: proc_macro2::TokenStream, target: &str) -> bool {
    tokens.into_iter().any(|token| match token {
        proc_macro2::TokenTree::Ident(ident) => ident == target,
        proc_macro2::TokenTree::Group(group) => tokens_contain_ident(group.stream(), target),
        _ => false,
    })
}

fn calls_in_production(file: &File, targets: &[&str]) -> Result<Vec<StructuralCall>, String> {
    let mut visitor = CallVisitor {
        targets,
        aliases: BTreeMap::new(),
        context: Vec::new(),
        calls: Vec::new(),
        errors: Vec::new(),
    };
    visitor.visit_file(file);
    if visitor.errors.is_empty() {
        Ok(visitor.calls)
    } else {
        Err(visitor.errors.join("; "))
    }
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

fn compact(tokens: &impl ToTokens) -> String {
    tokens
        .to_token_stream()
        .to_string()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn local_parts(statement: &Stmt) -> Option<(String, &Expr)> {
    let Stmt::Local(local) = statement else {
        return None;
    };
    let syn::Pat::Ident(binding) = &local.pat else {
        return None;
    };
    Some((
        binding.ident.to_string(),
        local.init.as_ref()?.expr.as_ref(),
    ))
}

fn statement_expression(statement: &Stmt) -> Option<&Expr> {
    let Stmt::Expr(expression, _) = statement else {
        return None;
    };
    Some(expression)
}

#[derive(Clone, Debug)]
struct OperationCall {
    callee: String,
    arguments: Vec<String>,
}

#[derive(Clone, Debug)]
struct MethodCall {
    method: String,
    receiver: String,
    arguments: Vec<String>,
}

#[derive(Default)]
struct OperationVisitor {
    calls: Vec<OperationCall>,
    methods: Vec<MethodCall>,
}

impl<'ast> Visit<'ast> for OperationVisitor {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        self.calls.push(OperationCall {
            callee: compact(&call.func),
            arguments: call.args.iter().map(compact).collect(),
        });
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.methods.push(MethodCall {
            method: call.method.to_string(),
            receiver: compact(&call.receiver),
            arguments: call.args.iter().map(compact).collect(),
        });
        syn::visit::visit_expr_method_call(self, call);
    }
}

fn operation_calls(tokens: &impl ToTokens) -> OperationVisitor {
    let parsed: Expr = syn::parse2(tokens.to_token_stream())
        .unwrap_or_else(|error| panic!("operation expression reparses: {error}"));
    let mut visitor = OperationVisitor::default();
    visitor.visit_expr(&parsed);
    visitor
}

fn calls_named<'a>(operations: &'a OperationVisitor, name: &str) -> Vec<&'a OperationCall> {
    operations
        .calls
        .iter()
        .filter(|call| call.callee.split("::").last() == Some(name))
        .collect()
}

fn exact_call(
    expression: &Expr,
    name: &str,
    arguments: &[String],
    stage: &str,
) -> Result<(), String> {
    let operations = operation_calls(expression);
    let calls = calls_named(&operations, name);
    if calls.len() != 1 || calls[0].arguments != arguments {
        return Err(format!(
            "{stage} needs one {name} call with {arguments:?}, found {calls:?}"
        ));
    }
    Ok(())
}

fn direct_call<'ast>(expression: &'ast Expr, name: &str) -> Option<&'ast syn::ExprCall> {
    let Expr::Call(call) = without_wrappers(expression) else {
        return None;
    };
    (expression_name(&call.func).as_deref() == Some(name)).then_some(call)
}

fn require_direct_call(
    statement: &Stmt,
    name: &str,
    arguments: &[String],
    stage: &str,
) -> Result<(), String> {
    let expression = statement_expression(statement)
        .ok_or_else(|| format!("{stage} must be an expression statement"))?;
    let call = direct_call(expression, name)
        .ok_or_else(|| format!("{stage} must directly call {name}"))?;
    let found: Vec<_> = call.args.iter().map(compact).collect();
    if found != arguments {
        return Err(format!(
            "{stage} {name} arguments are {found:?}, not {arguments:?}"
        ));
    }
    Ok(())
}

fn bit_or_terms(expression: &Expr, terms: &mut Vec<String>) -> Result<(), String> {
    match without_wrappers(expression) {
        Expr::Binary(binary) if matches!(binary.op, syn::BinOp::BitOr(_)) => {
            bit_or_terms(&binary.left, terms)?;
            bit_or_terms(&binary.right, terms)
        }
        Expr::Path(path) => {
            let Some(last) = path.path.segments.last() else {
                return Err("empty open-flag path".to_owned());
            };
            terms.push(last.ident.to_string());
            Ok(())
        }
        other => Err(format!(
            "open flags contain an unclassified expression {}",
            compact(other)
        )),
    }
}

fn validate_nofollow_fstatat(
    expression: &Expr,
    directory: &str,
    name: &str,
    stage: &str,
) -> Result<(), String> {
    exact_call(
        expression,
        "fstatat",
        &[
            directory.to_owned(),
            name.to_owned(),
            "AtFlags::AT_SYMLINK_NOFOLLOW".to_owned(),
        ],
        stage,
    )
}

fn validate_openat(expression: &Expr, directory: &str, name: &str) -> Result<(), String> {
    let operations = operation_calls(expression);
    let calls = calls_named(&operations, "openat");
    if calls.len() != 1 || calls[0].arguments.len() != 4 {
        return Err(format!(
            "Open needs one four-argument openat, found {calls:?}"
        ));
    }
    let call = calls[0];
    if call.arguments[0] != directory || call.arguments[1] != name {
        return Err("openat must use the bound directory and name".to_owned());
    }
    let flags: Expr =
        syn::parse_str(&call.arguments[2]).map_err(|error| format!("parse open flags: {error}"))?;
    let mut terms = Vec::new();
    bit_or_terms(&flags, &mut terms)?;
    let found: BTreeSet<_> = terms.iter().map(String::as_str).collect();
    let expected = BTreeSet::from(["O_RDONLY", "O_NONBLOCK", "O_NOFOLLOW", "O_CLOEXEC"]);
    if terms.len() != 4 || found != expected {
        return Err(format!(
            "openat flags must be exactly O_RDONLY|O_NONBLOCK|O_NOFOLLOW|O_CLOEXEC, found {terms:?}"
        ));
    }
    if call.arguments[3] != "Mode::empty()" {
        return Err("openat mode must be Mode::empty()".to_owned());
    }
    Ok(())
}

fn closure_argument_name(pattern: &syn::Pat) -> Option<String> {
    match pattern {
        syn::Pat::Ident(binding) => Some(binding.ident.to_string()),
        syn::Pat::Type(typed) => closure_argument_name(&typed.pat),
        _ => None,
    }
}

fn validate_require_regular(expression: &Expr) -> Result<(), String> {
    let Expr::Closure(closure) = without_wrappers(expression) else {
        return Err("require_regular must remain a closure".to_owned());
    };
    let status = closure
        .inputs
        .first()
        .and_then(closure_argument_name)
        .ok_or_else(|| "require_regular needs one status argument".to_owned())?;
    if closure.inputs.len() != 1 {
        return Err("require_regular needs exactly one argument".to_owned());
    }
    let expected =
        format!("SFlag::from_bits_truncate({status}.st_mode)&SFlag::S_IFMT==SFlag::S_IFREG");
    let Expr::Block(block) = without_wrappers(&closure.body) else {
        return Err("require_regular body must be a block".to_owned());
    };
    let [Stmt::Expr(Expr::If(conditional), _)] = block.block.stmts.as_slice() else {
        return Err("require_regular must be one explicit if/else".to_owned());
    };
    if compact(&conditional.cond) != expected {
        return Err("require_regular must test status mode for S_IFREG".to_owned());
    }
    if conditional.else_branch.is_none()
        || !compact(&conditional.then_branch).contains("Ok(())")
        || !compact(
            conditional
                .else_branch
                .as_ref()
                .expect("else exists")
                .1
                .as_ref(),
        )
        .contains("Err(")
    {
        return Err("require_regular must return Ok only for regular files".to_owned());
    }
    Ok(())
}

fn validate_same_identity(expression: &Expr) -> Result<(), String> {
    let Expr::Closure(closure) = without_wrappers(expression) else {
        return Err("same_identity must remain a closure".to_owned());
    };
    if closure.inputs.len() != 2 {
        return Err("same_identity needs status and expected arguments".to_owned());
    }
    let status = closure_argument_name(&closure.inputs[0])
        .ok_or_else(|| "same_identity status binding is unclassified".to_owned())?;
    let expected = closure_argument_name(&closure.inputs[1])
        .ok_or_else(|| "same_identity expected binding is unclassified".to_owned())?;
    let required = format!("({status}.st_dev,{status}.st_ino)=={expected}");
    if compact(&closure.body) != required {
        return Err("same_identity must compare status (dev, ino) to expected".to_owned());
    }
    Ok(())
}

fn validate_identity_guard(
    statement: &Stmt,
    helper: &str,
    status: &str,
    expected: &str,
    stage: &str,
) -> Result<(), String> {
    let Some(Expr::If(conditional)) = statement_expression(statement) else {
        return Err(format!("{stage} identity check must be a top-level if"));
    };
    if conditional.else_branch.is_some() {
        return Err(format!(
            "{stage} identity failure may not have an else path"
        ));
    }
    let Expr::Unary(unary) = without_wrappers(&conditional.cond) else {
        return Err(format!("{stage} identity condition must reject inequality"));
    };
    if !matches!(unary.op, syn::UnOp::Not(_)) {
        return Err(format!("{stage} identity condition must be negated"));
    }
    let call = direct_call(&unary.expr, helper)
        .ok_or_else(|| format!("{stage} identity guard must call {helper}"))?;
    let arguments: Vec<_> = call.args.iter().map(compact).collect();
    if arguments != [format!("&{status}"), expected.to_owned()] {
        return Err(format!(
            "{stage} identity guard uses {arguments:?}, not {status}/{expected}"
        ));
    }
    let [Stmt::Expr(Expr::Return(returned), _)] = conditional.then_branch.stmts.as_slice() else {
        return Err(format!("{stage} identity mismatch must return immediately"));
    };
    let Some(result) = returned.expr.as_deref() else {
        return Err(format!("{stage} identity mismatch must return Err"));
    };
    if direct_call(result, "Err").is_none() {
        return Err(format!("{stage} identity mismatch must return Err"));
    }
    Ok(())
}

fn validate_expected_identity(expression: &Expr, initial: &str) -> Result<(), String> {
    let Expr::Tuple(tuple) = without_wrappers(expression) else {
        return Err("expected identity must be a (dev, ino) tuple".to_owned());
    };
    let fields: Vec<_> = tuple.elems.iter().map(compact).collect();
    if fields != [format!("{initial}.st_dev"), format!("{initial}.st_ino")] {
        return Err("expected identity must capture initial (dev, ino)".to_owned());
    }
    Ok(())
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

    let statements = &function.block.stmts;
    if statements.len() != 29 {
        return Err(format!(
            "bound reader must remain one classified 29-statement linear protocol, found {}",
            statements.len()
        ));
    }
    let checkpoint_positions: Vec<_> = top.iter().map(|(index, _)| *index).collect();
    if checkpoint_positions != [6, 10, 12, 18, 20, 24] {
        return Err(format!(
            "authoritative operations no longer occupy the linear protocol: {checkpoint_positions:?}"
        ));
    }

    let arguments: Vec<_> = function
        .sig
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            syn::FnArg::Typed(typed) => closure_argument_name(&typed.pat),
            syn::FnArg::Receiver(_) => None,
        })
        .collect();
    if arguments.len() != 2 {
        return Err("bound reader needs exactly directory and name arguments".to_owned());
    }
    let directory = &arguments[0];
    let name = &arguments[1];

    for statement in statements {
        if let Some((binding, _)) = local_parts(statement)
            && matches!(
                binding.as_str(),
                "openat" | "fstatat" | "fstat" | "read_to_end"
            )
        {
            return Err(format!("operation name {binding} may not be shadowed"));
        }
    }

    let (path, path_expression) = local_parts(&statements[2])
        .ok_or_else(|| "protocol must bind the display path first".to_owned())?;
    if compact(path_expression) != format!("Path::new({name})") {
        return Err("display path must derive from the bound name".to_owned());
    }
    let (checkpoint_error, checkpoint_expression) = local_parts(&statements[3])
        .ok_or_else(|| "protocol must bind checkpoint_error".to_owned())?;
    let Expr::Closure(checkpoint_closure) = without_wrappers(checkpoint_expression) else {
        return Err("checkpoint_error must remain a closure".to_owned());
    };
    let primitive = checkpoint_closure
        .inputs
        .first()
        .and_then(closure_argument_name)
        .ok_or_else(|| "checkpoint_error primitive is unclassified".to_owned())?;
    if checkpoint_closure.inputs.len() != 1 {
        return Err("checkpoint_error needs one primitive".to_owned());
    }
    exact_call(
        &checkpoint_closure.body,
        "checkpoint",
        std::slice::from_ref(&primitive),
        "checkpoint_error",
    )?;
    if !compact(&checkpoint_closure.body).contains(&format!("io_error({path},")) {
        return Err("checkpoint errors must remain attributed to the bound path".to_owned());
    }

    let (require_regular, require_regular_expression) = local_parts(&statements[4])
        .ok_or_else(|| "protocol must bind require_regular".to_owned())?;
    validate_require_regular(require_regular_expression)?;
    let (same_identity, same_identity_expression) =
        local_parts(&statements[5]).ok_or_else(|| "protocol must bind same_identity".to_owned())?;
    validate_same_identity(same_identity_expression)?;

    for (index, primitive) in [
        (6, "InitialNameObserve"),
        (10, "Open"),
        (12, "OpenedHandleObserve"),
        (18, "Read"),
        (20, "FinalHandleObserve"),
        (24, "FinalNameObserve"),
    ] {
        require_direct_call(
            &statements[index],
            &checkpoint_error,
            &[format!("BoundReadPrimitive::{primitive}")],
            primitive,
        )?;
    }

    let (initial, initial_expression) = local_parts(&statements[7])
        .ok_or_else(|| "InitialNameObserve must bind initial status".to_owned())?;
    let Expr::Match(initial_match) = without_wrappers(initial_expression) else {
        return Err("initial fstatat must explicitly classify ENOENT".to_owned());
    };
    validate_nofollow_fstatat(&initial_match.expr, directory, name, "initial observation")?;
    if !is_enoent_none_arm(initial_match) {
        return Err("only the initial ENOENT arm may return Ok(None)".to_owned());
    }
    require_direct_call(
        &statements[8],
        &require_regular,
        &[format!("&{initial}")],
        "initial regular-file check",
    )?;
    let (expected, expected_expression) = local_parts(&statements[9])
        .ok_or_else(|| "protocol must capture expected identity".to_owned())?;
    validate_expected_identity(expected_expression, &initial)?;

    let (descriptor, open_expression) =
        local_parts(&statements[11]).ok_or_else(|| "Open must bind a descriptor".to_owned())?;
    let Expr::Match(open_match) = without_wrappers(open_expression) else {
        return Err("openat must explicitly map failure to Err".to_owned());
    };
    validate_openat(&open_match.expr, directory, name)?;

    let (opened, opened_expression) = local_parts(&statements[13])
        .ok_or_else(|| "OpenedHandleObserve must bind opened status".to_owned())?;
    exact_call(
        opened_expression,
        "fstat",
        &[format!("&{descriptor}")],
        "opened-handle observation",
    )?;
    require_direct_call(
        &statements[14],
        &require_regular,
        &[format!("&{opened}")],
        "opened-handle regular-file check",
    )?;
    validate_identity_guard(
        &statements[15],
        &same_identity,
        &opened,
        &expected,
        "opened-handle",
    )?;

    let (file, file_expression) = local_parts(&statements[16])
        .ok_or_else(|| "opened descriptor must become a File".to_owned())?;
    let file_operations = operation_calls(file_expression);
    let from_calls = calls_named(&file_operations, "from");
    if from_calls.len() != 1
        || from_calls[0].callee != "std::fs::File::from"
        || from_calls[0].arguments != [descriptor.clone()]
    {
        return Err("File must consume the opened descriptor exactly once".to_owned());
    }
    let (bytes, bytes_expression) = local_parts(&statements[17])
        .ok_or_else(|| "protocol must bind the returned byte buffer".to_owned())?;
    if compact(bytes_expression) != "Vec::new()" {
        return Err("returned byte buffer must start empty".to_owned());
    }
    let read_expression = statement_expression(&statements[19])
        .ok_or_else(|| "Read must be followed by a descriptor method call".to_owned())?;
    let read_operations = operation_calls(read_expression);
    let reads: Vec<_> = read_operations
        .methods
        .iter()
        .filter(|call| call.method == "read_to_end")
        .collect();
    if reads.len() != 1
        || reads[0].receiver != *file
        || reads[0].arguments != [format!("&mut{bytes}")]
    {
        return Err("Read must fill the returned buffer through the opened File".to_owned());
    }

    let (final_handle, final_handle_expression) = local_parts(&statements[21])
        .ok_or_else(|| "FinalHandleObserve must bind final handle status".to_owned())?;
    exact_call(
        final_handle_expression,
        "fstat",
        &[format!("&{file}")],
        "final-handle observation",
    )?;
    require_direct_call(
        &statements[22],
        &require_regular,
        &[format!("&{final_handle}")],
        "final-handle regular-file check",
    )?;
    validate_identity_guard(
        &statements[23],
        &same_identity,
        &final_handle,
        &expected,
        "final-handle",
    )?;

    let (final_name, final_name_expression) = local_parts(&statements[25])
        .ok_or_else(|| "FinalNameObserve must bind final name status".to_owned())?;
    validate_nofollow_fstatat(
        final_name_expression,
        directory,
        name,
        "final-name observation",
    )?;
    require_direct_call(
        &statements[26],
        &require_regular,
        &[format!("&{final_name}")],
        "final-name regular-file check",
    )?;
    validate_identity_guard(
        &statements[27],
        &same_identity,
        &final_name,
        &expected,
        "final-name",
    )?;

    let success = statement_expression(&statements[28])
        .ok_or_else(|| "protocol must end in Ok(Some(bytes))".to_owned())?;
    if compact(success) != format!("Ok(Some({bytes}))") {
        return Err("protocol must return only the descriptor-filled bytes".to_owned());
    }

    let mut whole = OperationVisitor::default();
    whole.visit_block(&function.block);
    for (operation, expected_count) in [("openat", 1), ("fstatat", 2), ("fstat", 2)] {
        let count = calls_named(&whole, operation).len();
        if count != expected_count {
            return Err(format!(
                "protocol needs exactly {expected_count} live {operation} calls, found {count}"
            ));
        }
    }
    let path_reads = whole
        .calls
        .iter()
        .filter(|call| {
            matches!(
                call.callee.as_str(),
                "fs::read"
                    | "std::fs::read"
                    | "fs::read_to_string"
                    | "std::fs::read_to_string"
                    | "File::open"
                    | "std::fs::File::open"
            )
        })
        .count();
    if path_reads != 0 {
        return Err("path-based reread or second open is forbidden".to_owned());
    }
    if whole
        .methods
        .iter()
        .filter(|call| call.method == "read_to_end")
        .count()
        != 1
    {
        return Err("protocol needs exactly one descriptor read".to_owned());
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

fn mutate_token_occurrence(
    function: &ItemFn,
    from: &str,
    to: &str,
    occurrence: usize,
    expected_count: usize,
) -> ItemFn {
    let mut source = function.to_token_stream().to_string();
    let positions: Vec<_> = source.match_indices(from).map(|(index, _)| index).collect();
    assert_eq!(
        positions.len(),
        expected_count,
        "fixture target {from:?} occurs exactly {expected_count} times in {source}"
    );
    let position = positions[occurrence];
    source.replace_range(position..position + from.len(), to);
    syn::parse_str(&source).unwrap_or_else(|error| panic!("mutated fixture reparses: {error}"))
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
    assert!(
        !closure.has(MCP_ENDPOINT, "test-hooks"),
        "{build} activates {MCP_ENDPOINT}/test-hooks"
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
    let mut raw = calls_in_production(&config, &["read_bytes_bound"])
        .expect("journal-config target calls classify");
    raw.extend(
        calls_in_production(&committed, &["read_bytes_bound"])
            .expect("sol-link target calls classify"),
    );
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
    )
    .expect("wrapper calls classify");
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

fn enum_variants(file: &File, name: &str) -> Result<BTreeSet<String>, String> {
    let enumeration = file
        .items
        .iter()
        .find_map(|item| match item {
            SynItem::Enum(enumeration) if enumeration.ident == name => Some(enumeration),
            _ => None,
        })
        .ok_or_else(|| format!("enum {name} exists"))?;
    Ok(enumeration
        .variants
        .iter()
        .map(|variant| variant.ident.to_string())
        .collect())
}

fn function_tokens(file: &File, name: &str) -> Result<String, String> {
    file.items
        .iter()
        .find_map(|item| match item {
            SynItem::Fn(function) if function.sig.ident == name => Some(compact(function)),
            _ => None,
        })
        .ok_or_else(|| format!("function {name} exists"))
}

fn macro_tokens(file: &File, name: &str) -> Result<String, String> {
    file.items
        .iter()
        .find_map(|item| match item {
            SynItem::Macro(item) if item.ident.as_ref().is_some_and(|ident| ident == name) => {
                Some(compact(&item.mac.tokens))
            }
            _ => None,
        })
        .ok_or_else(|| format!("macro {name} exists"))
}

fn path_ident(expression: &Expr) -> Option<String> {
    let Expr::Path(path) = without_wrappers(expression) else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn process_row_macro(expression: &Expr) -> Result<(String, String, String), String> {
    let Expr::Macro(expression) = expression else {
        return Err(format!(
            "matrix row is not a macro: {}",
            compact(expression)
        ));
    };
    let category = expression
        .mac
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
        .ok_or_else(|| "matrix macro has no name".to_owned())?;
    let arguments = Punctuated::<Expr, Token![,]>::parse_terminated
        .parse2(expression.mac.tokens.clone())
        .map_err(|error| format!("parse {category} row: {error}"))?;
    if arguments.len() != 3 {
        return Err(format!("{category} row needs id, race, and leaf"));
    }
    let race = path_ident(&arguments[1]).ok_or_else(|| "row race is not an ident".to_owned())?;
    let leaf = path_ident(&arguments[2]).ok_or_else(|| "row leaf is not an ident".to_owned())?;
    Ok((category, race, leaf))
}

fn analyze_process_matrix(source: &str) -> Result<(), String> {
    let file = syn::parse_file(source).map_err(|error| format!("parse process target: {error}"))?;
    let races = BTreeSet::from([
        "FifoSubstitutionBeforeOpen".to_owned(),
        "UnixSocketSubstitutionBeforeOpen".to_owned(),
        "RegularReplacementBeforeOpen".to_owned(),
        "DisappearanceBeforeOpen".to_owned(),
        "RegularReplacementAfterOpen".to_owned(),
        "DisappearanceAfterOpen".to_owned(),
    ]);
    if enum_variants(&file, "RaceClass")? != races {
        return Err(
            "RaceClass must be exactly six filesystem races including Unix socket".to_owned(),
        );
    }
    let donors = BTreeSet::from([
        "InitialFifo".to_owned(),
        "InitialSocket".to_owned(),
        "FifoSubstitution".to_owned(),
        "SocketSubstitution".to_owned(),
        "DisappearanceBeforeOpen".to_owned(),
        "RegularReplacementBeforeOpen".to_owned(),
        "DisappearanceAfterOpen".to_owned(),
        "RegularReplacementAfterOpen".to_owned(),
    ]);
    if enum_variants(&file, "Donor")? != donors {
        return Err("Donor must contain exactly eight adversarial raw-reader rows".to_owned());
    }
    if enum_variants(&file, "Control")?
        != BTreeSet::from(["Missing".to_owned(), "Unchanged".to_owned()])
    {
        return Err("Control must contain exactly missing and unchanged".to_owned());
    }

    let rows = file
        .items
        .iter()
        .find_map(|item| match item {
            SynItem::Const(item) if item.ident == "ROWS" => Some(item),
            _ => None,
        })
        .ok_or_else(|| "ROWS const exists".to_owned())?;
    let syn::Type::Array(row_type) = rows.ty.as_ref() else {
        return Err("ROWS must have an exact array type".to_owned());
    };
    if compact(&row_type.len) != "70" {
        return Err("ROWS type denominator must be 70".to_owned());
    }
    let Expr::Array(array) = rows.expr.as_ref() else {
        return Err("ROWS must be an array literal".to_owned());
    };
    if array.elems.len() != 70 {
        return Err("ROWS literal denominator must be 70".to_owned());
    }
    let fixed = [
        "RowKind::Donor(Donor::InitialFifo)",
        "RowKind::Donor(Donor::InitialSocket)",
        "RowKind::Donor(Donor::FifoSubstitution)",
        "RowKind::Donor(Donor::SocketSubstitution)",
        "RowKind::Donor(Donor::DisappearanceBeforeOpen)",
        "RowKind::Donor(Donor::RegularReplacementBeforeOpen)",
        "RowKind::Donor(Donor::DisappearanceAfterOpen)",
        "RowKind::Donor(Donor::RegularReplacementAfterOpen)",
        "RowKind::Control(Control::Missing)",
        "RowKind::Control(Control::Unchanged)",
    ];
    for (index, expected) in fixed.iter().enumerate() {
        if !compact(&array.elems[index]).contains(expected) {
            return Err(format!("matrix row {index} must be {expected}"));
        }
    }

    let leaves = [
        "ConfigJournalJson",
        "CaCertificatePem",
        "CaPrivatePem",
        "LinkStatePrimary",
        "CaStateFallback",
    ];
    let cross_product: BTreeSet<_> = races
        .iter()
        .flat_map(|race| leaves.iter().map(move |leaf| format!("{race}/{leaf}")))
        .collect();
    let mut direct = BTreeSet::new();
    let mut endpoint = BTreeSet::new();
    for expression in array.elems.iter().skip(10) {
        let (category, race, leaf) = process_row_macro(expression)?;
        let value = format!("{race}/{leaf}");
        let inserted = match category.as_str() {
            "direct" => direct.insert(value),
            "endpoint" => endpoint.insert(value),
            _ => return Err(format!("unexpected matrix category {category}")),
        };
        if !inserted {
            return Err(format!("duplicate {category} matrix row {race}/{leaf}"));
        }
    }
    if direct != cross_product || endpoint != cross_product {
        return Err(
            "direct and endpoint rows must each be the exact five-leaf/six-race cross product"
                .to_owned(),
        );
    }

    let direct_macro = macro_tokens(&file, "direct")?;
    for required in [
        "kind:RowKind::Direct",
        "race:RaceClass::$race",
        "leaf:Leaf::$leaf",
    ] {
        if !direct_macro.contains(required) {
            return Err(format!("direct macro must preserve {required}"));
        }
    }
    let endpoint_macro = macro_tokens(&file, "endpoint")?;
    for required in [
        "kind:RowKind::EndpointTwin",
        "race:RaceClass::$race",
        "leaf:Leaf::$leaf",
    ] {
        if !endpoint_macro.contains(required) {
            return Err(format!("endpoint macro must preserve {required}"));
        }
    }

    let dispatch = function_tokens(&file, "execute_row")?;
    for required in [
        "RowKind::Donor(donor)=>execute_donor(donor)",
        "RowKind::Control(control)=>execute_control(control)",
        "RowKind::Direct{leaf,race}=>execute_direct(leaf,race)",
        "RowKind::EndpointTwin{leaf,race}=>execute_endpoint(leaf,race)",
    ] {
        if !dispatch.contains(required) {
            return Err(format!("row dispatch must preserve {required}"));
        }
    }
    let race_dispatch = function_tokens(&file, "exercise_race")?;
    let socket = race_dispatch
        .split("RaceClass::UnixSocketSubstitutionBeforeOpen=>")
        .nth(1)
        .and_then(|tail| tail.split("RaceClass::").next())
        .ok_or_else(|| "Unix-socket race arm exists".to_owned())?;
    if !socket.contains("run_with_bound_read_barrier")
        || !socket.contains("BoundReadPrimitive::Open")
        || !socket.contains("replace_with_socket(&target)")
        || socket.contains("run_with_bound_read_fault")
        || socket.contains("mkfifo")
    {
        return Err(
            "Unix-socket race arm must install a socket at the intended Open barrier".to_owned(),
        );
    }

    let direct_ordinals = function_tokens(&file, "direct_ordinal")?;
    for required in [
        "Leaf::ConfigJournalJson|Leaf::CaCertificatePem=>1",
        "Leaf::CaPrivatePem=>2",
        "Leaf::LinkStatePrimary|Leaf::CaStateFallback=>3",
    ] {
        if !direct_ordinals.contains(required) {
            return Err(format!("direct ordinal map must preserve {required}"));
        }
    }
    let endpoint_ordinals = function_tokens(&file, "endpoint_ordinal")?;
    for required in [
        "Leaf::ConfigJournalJson=>1",
        "Leaf::CaCertificatePem=>2",
        "Leaf::CaPrivatePem=>3",
        "Leaf::LinkStatePrimary|Leaf::CaStateFallback=>4",
    ] {
        if !endpoint_ordinals.contains(required) {
            return Err(format!("endpoint ordinal map must preserve {required}"));
        }
    }

    let timeout = file
        .items
        .iter()
        .find_map(|item| match item {
            SynItem::Const(item) if item.ident == "ROW_TIMEOUT" => Some(compact(&item.expr)),
            _ => None,
        })
        .ok_or_else(|| "ROW_TIMEOUT exists".to_owned())?;
    if timeout != "Duration::from_secs(5)" {
        return Err("row timeout must be exactly five seconds".to_owned());
    }
    let detached = function_tokens(&file, "run_detached")?;
    for required in [
        "receiver.recv_timeout(ROW_TIMEOUT)",
        "Ok(result)=>matchworker.join()",
        "Err(mpsc::RecvTimeoutError::Timeout)=>{drop(worker);",
        "Err(mpsc::RecvTimeoutError::Disconnected)=>{drop(worker);",
    ] {
        if !detached.contains(required) {
            return Err(format!("bounded worker lifecycle must preserve {required}"));
        }
    }
    if function_tokens(
        &file,
        "initial_device_control_is_outside_the_70_row_denominator",
    )?
    .is_empty()
    {
        return Err("separate optional-device control exists".to_owned());
    }
    let linux_device_import = file
        .items
        .iter()
        .find(|item| {
            matches!(item, SynItem::Use(_))
                && compact(item).contains("makedev")
                && compact(item).contains("mknod")
        })
        .ok_or_else(|| "Linux-only device fixture import exists".to_owned())?;
    let expected_device_cfg = "#[cfg(target_os=\"linux\")]";
    if !compact(linux_device_import).starts_with(expected_device_cfg) {
        return Err("device-fixture imports must remain Linux-only".to_owned());
    }
    Ok(())
}

fn replace_source_once(source: &str, from: &str, to: &str) -> String {
    assert_eq!(
        source.matches(from).count(),
        1,
        "source mutation target {from:?} occurs once"
    );
    source.replacen(from, to, 1)
}

fn test_names(document: &DocumentMut) -> BTreeSet<String> {
    document
        .get("test")
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flatten()
        .filter_map(|table| table.get("name").and_then(Item::as_str))
        .map(str::to_owned)
        .collect()
}

fn table_strings(table: &Table, field: &str) -> Vec<String> {
    table
        .get(field)
        .and_then(Item::as_array)
        .unwrap_or_else(|| panic!("{field} array exists"))
        .iter()
        .filter_map(toml_edit::Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[test]
fn bound_read_callers_cover_the_five_committed_leaf_paths() {
    assert_eq!(resolved_leaves(), expected_leaves());
}

#[test]
fn bound_read_process_matrix_has_exact_runtime_semantics() {
    let source = read_repo_file(
        "core/crates/solstone-core-mcp-endpoint/tests/mcp_endpoint_bound_leaf_process.rs",
    );
    analyze_process_matrix(&source).expect("70-row process matrix remains exact");

    let duplicate_fifo = replace_source_once(
        &source,
        "\"direct-unix-socket-before-open-config\",\n        UnixSocketSubstitutionBeforeOpen",
        "\"direct-unix-socket-before-open-config\",\n        FifoSubstitutionBeforeOpen",
    );
    assert!(
        analyze_process_matrix(&duplicate_fifo).is_err(),
        "Unix-socket row diverted to FIFO must reject"
    );

    let diverted_dispatch = replace_source_once(
        &source,
        "RowKind::Direct { leaf, race } => execute_direct(leaf, race),",
        "RowKind::Direct { leaf, race } => execute_endpoint(leaf, race),",
    );
    assert!(
        analyze_process_matrix(&diverted_dispatch).is_err(),
        "direct rows dispatched as endpoints must reject"
    );

    let wrong_ordinal = replace_source_once(
        &source,
        "Leaf::CaPrivatePem => 2,",
        "Leaf::CaPrivatePem => 1,",
    );
    assert!(
        analyze_process_matrix(&wrong_ordinal).is_err(),
        "wrong direct leaf ordinal must reject"
    );

    let unbounded = replace_source_once(
        &source,
        "const ROW_TIMEOUT: Duration = Duration::from_secs(5);",
        "const ROW_TIMEOUT: Duration = Duration::from_secs(6);",
    );
    assert!(
        analyze_process_matrix(&unbounded).is_err(),
        "changed timeout must reject"
    );
}

#[test]
fn bound_read_process_target_is_the_single_registered_boundary_suite() {
    for retired in [
        "core/crates/solstone-core-journal-config/tests/bound_config_socket_boundary.rs",
        "core/crates/solstone-core-journal-io/tests/bound_read_socket_boundaries.rs",
        "core/crates/solstone-core-sol-link/tests/committed_bound_socket_boundaries.rs",
    ] {
        assert!(
            !repository_root().join(retired).exists(),
            "retired duplicate target {retired} stays absent"
        );
    }
    let journal_io = parse_manifest("core/crates/solstone-core-journal-io/Cargo.toml");
    assert!(!test_names(&journal_io).contains("bound_read_socket_boundaries"));
    let sol_link = parse_manifest("core/crates/solstone-core-sol-link/Cargo.toml");
    assert!(!test_names(&sol_link).contains("committed_bound_socket_boundaries"));

    let suites = parse_manifest("core/ci/suites.toml");
    let suite_tables = suites
        .get("suites")
        .and_then(Item::as_array_of_tables)
        .expect("suite registry is an array of tables");
    for legacy in [
        "solstone-core-journal-config::bound_config_socket_boundary",
        "solstone-core-journal-io::bound_read_socket_boundaries",
        "solstone-core-sol-link::committed_bound_socket_boundaries",
    ] {
        assert!(
            !suite_tables
                .iter()
                .any(|table| table.get("id").and_then(Item::as_str) == Some(legacy)),
            "legacy suite {legacy} stays absent"
        );
    }
    let process: Vec<_> = suite_tables
        .iter()
        .filter(|table| {
            table.get("id").and_then(Item::as_str)
                == Some("solstone-core-mcp-endpoint::mcp_endpoint_bound_leaf_process")
        })
        .collect();
    assert_eq!(process.len(), 1, "one process suite remains registered");
    let process = process[0];
    for (field, expected) in [
        ("package", "solstone-core-mcp-endpoint"),
        ("target", "mcp_endpoint_bound_leaf_process"),
        ("set", "component"),
        ("timeout", "standard"),
        ("runtime", "none"),
    ] {
        assert_eq!(process.get(field).and_then(Item::as_str), Some(expected));
    }
    assert_eq!(table_strings(process, "areas"), ["journal", "trust"]);
    assert_eq!(table_strings(process, "platforms"), ["linux", "macos"]);
    assert_eq!(table_strings(process, "prerequisites"), ["cargo-cache"]);
    assert_eq!(table_strings(process, "required_features"), ["test-hooks"]);
    assert_eq!(
        process.get("default_full").and_then(Item::as_bool),
        Some(true)
    );
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
fn bound_reader_protocol_rejects_operation_and_dataflow_mutations() {
    let reader = canonical_bound_reader();

    let without_nonblock = mutate_token_occurrence(&reader, "OFlag :: O_NONBLOCK | ", "", 0, 1);
    assert_rejected(
        "open without O_NONBLOCK",
        &without_nonblock,
        "openat flags must be exactly",
    );

    for flag in ["O_RDONLY", "O_NOFOLLOW", "O_CLOEXEC"] {
        let fixture = mutate_token_occurrence(
            &reader,
            &format!("OFlag :: {flag}"),
            "OFlag :: O_DIRECTORY",
            0,
            1,
        );
        assert_rejected(
            &format!("open without {flag}"),
            &fixture,
            "openat flags must be exactly",
        );
    }

    for occurrence in 0..2 {
        let fixture = mutate_token_occurrence(
            &reader,
            "AtFlags :: AT_SYMLINK_NOFOLLOW",
            "AtFlags :: empty ()",
            occurrence,
            2,
        );
        assert_rejected(
            &format!("fstatat without nofollow {occurrence}"),
            &fixture,
            if occurrence == 0 {
                "initial observation"
            } else {
                "final-name observation"
            },
        );
    }

    let permissive_regular =
        mutate_token_occurrence(&reader, "== SFlag :: S_IFREG", "!= SFlag :: S_IFREG", 0, 1);
    assert_rejected(
        "permissive regular helper",
        &permissive_regular,
        "require_regular must test",
    );

    let dev_only_identity = mutate_token_occurrence(
        &reader,
        "(status . st_dev , status . st_ino) == expected",
        "status . st_dev == expected . 0",
        0,
        1,
    );
    assert_rejected(
        "dev-only identity helper",
        &dev_only_identity,
        "same_identity must compare",
    );

    let wrong_open_name = mutate_token_occurrence(
        &reader,
        "openat (directory , name ,",
        "openat (directory , path ,",
        0,
        1,
    );
    assert_rejected(
        "openat wrong name",
        &wrong_open_name,
        "bound directory and name",
    );

    let wrong_opened_handle =
        mutate_token_occurrence(&reader, "fstat (& fd)", "fstat (directory)", 0, 1);
    assert_rejected(
        "opened fstat wrong descriptor",
        &wrong_opened_handle,
        "opened-handle observation",
    );

    let tautological_opened_identity = mutate_token_occurrence(
        &reader,
        "same_identity (& opened , expected)",
        "same_identity (& opened , (opened . st_dev , opened . st_ino))",
        0,
        1,
    );
    assert_rejected(
        "opened identity diverted",
        &tautological_opened_identity,
        "opened-handle identity guard",
    );

    let mut alternate_bytes = reader.clone();
    alternate_bytes.block.stmts[28] = Stmt::Expr(syn::parse_quote!(Ok(Some(Vec::new()))), None);
    assert_rejected(
        "alternate returned bytes",
        &reparse(&alternate_bytes),
        "descriptor-filled bytes",
    );

    let mut path_reread = reader.clone();
    path_reread
        .block
        .stmts
        .insert(28, syn::parse_quote!(let _again = std::fs::read(path)?;));
    assert_rejected(
        "path reread",
        &reparse(&path_reread),
        "29-statement linear protocol",
    );
}

#[test]
fn bound_reader_protocol_accepts_semantic_local_renaming() {
    let reader = canonical_bound_reader();
    let mut source = reader.to_token_stream().to_string();
    for (from, to) in [
        ("initial", "first_status"),
        ("expected", "bound_identity"),
        ("opened", "opened_status"),
        ("final_handle", "last_handle_status"),
        ("final_name", "last_name_status"),
        ("bytes", "contents"),
    ] {
        source = source.replace(from, to);
    }
    let renamed: ItemFn = syn::parse_str(&source).expect("renamed reader reparses");
    analyze_bound_read(&renamed).expect("semantic identifier renaming remains accepted");
}

#[test]
fn production_classifier_discovers_post_test_module_callers() {
    let file = production_file(
        "#[cfg(test)] mod tests { fn fixture() { read_bytes_bound(directory, name); } }\n\
         fn after_tests() { read_bytes_bound(directory, name); }",
    )
    .expect("fixture classifies");
    assert_eq!(
        calls_in_production(&file, &["read_bytes_bound"]).expect("calls classify"),
        [StructuralCall {
            function: "after_tests".to_owned(),
            callee: "read_bytes_bound".to_owned(),
            first_argument: Some("directory".to_owned()),
            filename: None,
        }]
    );
}

#[test]
fn production_classifier_excludes_tests_and_fails_closed_on_macro_callers() {
    let test_file = production_file(
        "#[cfg(test)] mod tests { fn fixture() { read_bytes_bound(directory, name); } }",
    )
    .expect("test fixture classifies");
    assert!(
        calls_in_production(&test_file, &["read_bytes_bound"])
            .expect("test-only calls classify")
            .is_empty(),
        "in-test caller must be excluded"
    );
    let macro_file =
        production_file("fn fixture() { some_macro!(read_bytes_bound(directory, name)); }")
            .expect("macro fixture classifies");
    let macro_error = calls_in_production(&macro_file, &["read_bytes_bound"])
        .expect_err("production target-bearing macro must fail closed");
    assert!(macro_error.contains("target-bearing production macro"));
}

#[test]
fn production_classifier_visits_named_modules_aliases_and_executable_bodies() {
    let file = production_file(
        "use crate::read_bytes_bound as rb;
         mod tests { pub fn production_named_tests_module() { read_bytes_bound(directory, name); } }
         struct Reader;
         impl Reader { fn method() { rb(directory, name); } }
         trait DefaultRead { fn method() { read_bytes_bound(directory, name); } }
         const CONST_READ: fn() = || { read_bytes_bound(directory, name); };
         static STATIC_READ: fn() = || { read_bytes_bound(directory, name); };
         fn nested() {
             let closure = || read_bytes_bound(directory, name);
             let future = async { read_bytes_bound(directory, name); };
         }
         #[cfg(test)] mod excluded { fn fixture() { read_bytes_bound(directory, name); } }",
    )
    .expect("body-shape fixture classifies");
    let calls =
        calls_in_production(&file, &["read_bytes_bound"]).expect("all production bodies classify");
    assert_eq!(calls.len(), 7, "every enabled executable body is visited");
    assert!(
        calls.iter().all(|call| call.callee == "read_bytes_bound"),
        "aliased calls resolve to the canonical target"
    );

    let attribute_file =
        production_file("#[contract(read_bytes_bound(directory, name))] fn fixture() {}")
            .expect("attribute fixture parses");
    let attribute_error = calls_in_production(&attribute_file, &["read_bytes_bound"])
        .expect_err("target-bearing production attribute must fail closed");
    assert!(attribute_error.contains("target-bearing production attribute"));
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
    let committed = syn::parse_file(&read_repo_file(
        "core/crates/solstone-core-sol-link/src/committed.rs",
    ))
    .expect("committed source parses");
    let test_items = committed
        .items
        .iter()
        .find_map(|item| match item {
            SynItem::Mod(module) if module.ident == "tests" => {
                module.content.as_ref().map(|(_, items)| items)
            }
            _ => None,
        })
        .expect("committed test module is inline");
    let hook_import = test_items
        .iter()
        .find(|item| {
            matches!(item, SynItem::Use(_))
                && compact(item).contains("run_with_two_bound_read_barriers")
        })
        .expect("hook-only committed import exists");
    assert!(
        !production_item_enabled(item_attributes(hook_import))
            .expect("hook import cfg classifies without test-hooks"),
        "ordinary Unix package tests must not compile hook-only imports"
    );
    let hook_test = test_items
        .iter()
        .find(|item| {
            matches!(item, SynItem::Fn(function) if function.sig.ident == "bound_reader_rejects_regular_certificate_replacement_after_open")
        })
        .expect("hook-only committed unit test exists");
    assert!(
        !production_item_enabled(item_attributes(hook_test))
            .expect("hook test cfg classifies without test-hooks"),
        "ordinary Unix package tests must exclude the hook-only test"
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
            "solstone-core-generate-wire".to_owned(),
            "solstone-core-mcp-endpoint".to_owned(),
        ]),
        "the process target reaches exactly its declared hooks and the audited Callosum wire closure"
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
