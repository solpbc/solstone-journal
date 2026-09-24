// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Expr, ExprCall, ExprMethodCall, Item, ItemFn, ItemMod};

#[derive(Default)]
struct LedgerRemoveVisitor {
    in_test_context: bool,
    ledger_bindings: Vec<String>,
    violations: Vec<String>,
}

impl LedgerRemoveVisitor {
    fn is_test_attr(attr: &syn::Attribute) -> bool {
        if attr.path().is_ident("test") {
            return true;
        }
        if let syn::Meta::List(meta_list) = &attr.meta
            && meta_list.path.is_ident("cfg")
            && meta_list.tokens.to_string().contains("test")
        {
            return true;
        }
        false
    }

    fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(Self::is_test_attr)
    }

    fn expr_is_authorization_ledger(expr: &Expr) -> bool {
        match expr {
            Expr::Call(ExprCall { func, .. }) => {
                let func_str = quote::quote!(#func).to_string();
                func_str.contains("AuthorizationLedger")
            }
            Expr::Path(path) => {
                let path_str = quote::quote!(#path).to_string();
                path_str.contains("AuthorizationLedger")
            }
            Expr::MethodCall(ExprMethodCall { receiver, .. }) => {
                Self::expr_is_authorization_ledger(receiver)
            }
            _ => false,
        }
    }
}

impl<'ast> Visit<'ast> for LedgerRemoveVisitor {
    fn visit_item_mod(&mut self, item_mod: &'ast ItemMod) {
        let prev_test = self.in_test_context;
        if Self::has_test_attr(&item_mod.attrs) {
            self.in_test_context = true;
        }
        syn::visit::visit_item_mod(self, item_mod);
        self.in_test_context = prev_test;
    }

    fn visit_item_fn(&mut self, item_fn: &'ast ItemFn) {
        let prev_test = self.in_test_context;
        if Self::has_test_attr(&item_fn.attrs) {
            self.in_test_context = true;
        }
        syn::visit::visit_item_fn(self, item_fn);
        self.in_test_context = prev_test;
    }

    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Fn(f) if Self::has_test_attr(&f.attrs) => {}
            Item::Mod(m) if Self::has_test_attr(&m.attrs) => {}
            _ => syn::visit::visit_item(self, item),
        }
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let Some(init) = &local.init
            && Self::expr_is_authorization_ledger(&init.expr)
            && let syn::Pat::Ident(pat_ident) = &local.pat
        {
            self.ledger_bindings.push(pat_ident.ident.to_string());
        }
        syn::visit::visit_local(self, local);
    }

    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        if !self.in_test_context {
            let func_str = quote::quote!(#call).to_string();
            if func_str.contains("AuthorizationLedger") && func_str.contains("remove") {
                self.violations.push(func_str);
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, method_call: &'ast ExprMethodCall) {
        if !self.in_test_context && method_call.method == "remove" {
            let receiver = &method_call.receiver;
            if Self::expr_is_authorization_ledger(receiver) {
                let call_str = quote::quote!(#method_call).to_string();
                self.violations.push(call_str);
            } else if let Expr::Path(syn::ExprPath { path, .. }) = receiver.as_ref() {
                let ident = quote::quote!(#path).to_string();
                if self.ledger_bindings.contains(&ident) {
                    let call_str = quote::quote!(#method_call).to_string();
                    self.violations.push(call_str);
                }
            }
        }
        syn::visit::visit_expr_method_call(self, method_call);
    }
}

fn scan_source(source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|e| format!("syn parse error: {e}"))?;
    let mut visitor = LedgerRemoveVisitor::default();
    visitor.visit_file(&file);
    Ok(visitor.violations)
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if dir_name != "target" && dir_name != "tests" {
                    collect_rs_files(&path, out);
                }
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
}

#[test]
fn planted_violation_detects_direct_authorization_ledger_remove_call() {
    let code = r#"
        fn unpair(root: &Path, cid: &str) {
            AuthorizationLedger::new(root).remove(cid).unwrap();
        }
    "#;
    let violations = scan_source(code).unwrap();
    assert_eq!(violations.len(), 1);
}

#[test]
fn planted_violation_detects_bound_ledger_variable_remove_call() {
    let code = r#"
        fn unpair(root: &Path, cid: &str) {
            let mut ledger = AuthorizationLedger::new(root);
            ledger.remove(cid).unwrap();
        }
    "#;
    let violations = scan_source(code).unwrap();
    assert_eq!(violations.len(), 1);
}

#[test]
fn planted_violation_ignores_test_modules_and_unrelated_removes() {
    let code = r#"
        fn harmless(map: &mut std::collections::HashMap<String, String>) {
            map.remove("key");
        }

        fn get_only(root: &Path, cid: &str) {
            let mut ledger = AuthorizationLedger::new(root);
            let _ = ledger.get(cid);
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            fn test_unpair(root: &Path, cid: &str) {
                AuthorizationLedger::new(root).remove(cid).unwrap();
            }
        }
    "#;
    let violations = scan_source(code).unwrap();
    assert!(
        violations.is_empty(),
        "expected 0 violations: {violations:?}"
    );
}

#[test]
fn only_paired_device_calls_authorization_ledger_remove_in_production() {
    let repo = repository_root();
    let crates_dir = repo.join("core/crates");
    let mut files = Vec::new();
    collect_rs_files(&crates_dir, &mut files);

    let permitted_paired_device =
        repo.join("core/crates/solstone-core-convey-shell/src/paired_device.rs");

    let paired_device_content = fs::read_to_string(&permitted_paired_device).unwrap();
    let paired_violations = scan_source(&paired_device_content).unwrap();
    assert_eq!(
        paired_violations.len(),
        1,
        "paired_device.rs must contain exactly 1 AuthorizationLedger::remove call"
    );

    let mut all_violations = Vec::new();

    for file in files {
        if file == permitted_paired_device {
            continue;
        }
        let content = fs::read_to_string(&file).unwrap_or_default();
        if let Ok(violations) = scan_source(&content) {
            for v in violations {
                all_violations.push(format!("{}: {v}", file.display()));
            }
        }
    }

    assert!(
        all_violations.is_empty(),
        "unauthorized AuthorizationLedger::remove calls found: {all_violations:#?}"
    );
}
