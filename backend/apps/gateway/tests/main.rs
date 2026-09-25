mod architecture;
mod bootstrap;

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use syn::{Attribute, Item, Meta, parse::Parser, visit::Visit};

#[test]
fn app_tree_matches_frozen_terminal_manifest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        rust_files(&root.join("src")),
        BTreeSet::from([
            PathBuf::from("bootstrap.rs"),
            PathBuf::from("lib.rs"),
            PathBuf::from("main.rs"),
        ]),
    );
    assert_eq!(
        rust_files(&root.join("tests")),
        BTreeSet::from([
            PathBuf::from("architecture.rs"),
            PathBuf::from("bootstrap.rs"),
            PathBuf::from("main.rs"),
        ]),
    );
}

#[test]
fn cargo_library_root_is_conventional_lib() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("read app Cargo.toml");
    assert!(!manifest.contains("[lib]"));
    assert!(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/lib.rs")
            .is_file()
    );
}

#[test]
fn workspace_production_files_have_no_hidden_modules_or_test_hooks() {
    for member in architecture::WORKSPACE_MEMBERS {
        let root = architecture::backend_root().join(member).join("src");
        for relative in rust_files(&root) {
            let path = root.join(&relative);
            let source = fs::read_to_string(&path).expect("read production source");
            assert!(
                !source.contains("include!("),
                "{} uses include!",
                path.display()
            );
            let syntax = syn::parse_file(&source).expect("parse production source");
            assert!(
                !has_unaudited_test_hooks(member, &relative, &syntax),
                "{} has a production test/path hook",
                path.display(),
            );
            for item in &syntax.items {
                if let Item::Mod(module) = item {
                    assert!(
                        module.content.is_none()
                            || is_audited_private_test(member, &relative, item),
                        "{} has an inline module",
                        path.display()
                    );
                }
            }
        }
    }
}

#[test]
fn bootstrap_owns_only_bundle_wiring() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let bootstrap = fs::read_to_string(root.join("bootstrap.rs")).expect("read bootstrap");
    for forbidden in [
        "sqlx::",
        "redis::",
        "Repository",
        "impl Provider",
        "impl ExecutionService",
        "tokio::spawn",
        "access_token",
        "refresh_token",
    ] {
        assert!(
            !bootstrap.contains(forbidden),
            "bootstrap owns `{forbidden}`"
        );
    }
    assert!(bootstrap.lines().count() <= 300);
}

fn rust_files(root: &Path) -> BTreeSet<PathBuf> {
    let mut result = BTreeSet::new();
    collect_rust_files(root, root, &mut result);
    result
}

fn collect_rust_files(root: &Path, current: &Path, result: &mut BTreeSet<PathBuf>) {
    for entry in fs::read_dir(current).expect("read app tree") {
        let path = entry.expect("read app tree entry").path();
        if path.is_dir() {
            collect_rust_files(root, &path, result);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            result.insert(
                path.strip_prefix(root)
                    .expect("relative app path")
                    .to_path_buf(),
            );
        }
    }
}

fn is_path_or_test_cfg(attribute: &Attribute) -> bool {
    if attribute.path().is_ident("path") {
        return true;
    }
    if !attribute.path().is_ident("cfg") && !attribute.path().is_ident("cfg_attr") {
        return false;
    }
    let Meta::List(list) = &attribute.meta else {
        return false;
    };
    list.tokens
        .to_string()
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|segment| segment == "test" || segment.starts_with("test_"))
}

fn is_exact_test_cfg(attribute: &Attribute) -> bool {
    attribute.path().is_ident("cfg")
        && attribute
            .parse_args::<syn::Ident>()
            .is_ok_and(|condition| condition == "test")
}

// These tests need private clock/lease state; never expose production APIs for them.
fn is_audited_private_test(member: &str, relative: &Path, item: &Item) -> bool {
    let attrs = item_attrs(item);
    if !attrs.iter().any(is_exact_test_cfg)
        || attrs.iter().any(|attr| {
            attr.path().is_ident("path")
                || attr.path().is_ident("cfg_attr")
                || (attr.path().is_ident("cfg") && !is_exact_test_cfg(attr))
        })
    {
        return false;
    }
    match (member, relative.to_str(), item) {
        // These tests inspect private queue ownership and monotonic deadlines without
        // exposing test-only hooks in the production admission API.
        (
            "crates/gateway-core",
            Some(
                "runtime/account_concurrency.rs" | "engine/key_wait.rs" | "engine/capacity_wait.rs",
            ),
            Item::Mod(module),
        ) => {
            module.ident == "tests"
                && module.content.is_some()
                && matches!(module.vis, syn::Visibility::Inherited)
        }
        ("crates/providers/openai", Some("transport/websocket/pool/mod.rs"), Item::Mod(module)) => {
            (module.ident == "tests" || module.ident == "lifecycle_tests")
                && module.content.is_none()
                && matches!(module.vis, syn::Visibility::Inherited)
        }
        // Protocol authorization, bounded attachment parsing and immutable replay
        // are private implementation details, not production test APIs.
        (
            "crates/providers/openai",
            Some(
                "provider/excel.rs"
                | "transport/excel/request.rs"
                | "transport/excel/replay.rs"
                | "transport/excel/tools.rs"
                | "transport/excel/images.rs"
                | "transport/excel/image_relay.rs"
                | "transport/excel/stream.rs",
            ),
            Item::Mod(module),
        ) => {
            module.ident == "tests"
                && module.content.is_some()
                && matches!(module.vis, syn::Visibility::Inherited)
        }
        ("crates/providers/openai", Some("transport/excel/mod.rs"), Item::Mod(module)) => {
            module.ident == "tests"
                && module.content.is_none()
                && matches!(module.vis, syn::Visibility::Inherited)
        }
        (
            "crates/providers/openai",
            Some("transport/websocket/coordinator.rs"),
            Item::Fn(function),
        ) => {
            function.sig.ident
                == "shared_opening_deadline_does_not_double_charge_account_capacity_wait"
                && function
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("test"))
                && matches!(function.vis, syn::Visibility::Inherited)
        }
        _ => false,
    }
}

fn has_unaudited_test_hooks(member: &str, relative: &Path, syntax: &syn::File) -> bool {
    let mut visitor = TestHookVisitor::default();
    for attribute in &syntax.attrs {
        visitor.visit_attribute(attribute);
    }
    for item in &syntax.items {
        if !is_audited_private_test(member, relative, item) {
            visitor.visit_item(item);
        }
    }
    visitor.found
}

#[derive(Default)]
struct TestHookVisitor {
    found: bool,
}

impl<'ast> Visit<'ast> for TestHookVisitor {
    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        self.found |= is_path_or_test_cfg(attribute);
    }

    fn visit_macro(&mut self, item: &'ast syn::Macro) {
        self.found |= macro_has_test_hooks
            .parse2(item.tokens.clone())
            .unwrap_or(true);
    }
}

// Syn keeps macro bodies opaque; inspect groups without interpreting string literals.
fn macro_has_test_hooks(input: syn::parse::ParseStream<'_>) -> syn::Result<bool> {
    let mut found = false;
    while !input.is_empty() {
        if input.peek(syn::Token![#]) && input.fork().call(single_macro_attribute).is_ok() {
            found |= is_path_or_test_cfg(&input.call(single_macro_attribute)?);
        } else if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            found |= macro_has_test_hooks(&content)?;
        } else if input.peek(syn::token::Brace) {
            let content;
            syn::braced!(content in input);
            found |= macro_has_test_hooks(&content)?;
        } else if input.peek(syn::token::Bracket) {
            let content;
            syn::bracketed!(content in input);
            found |= macro_has_test_hooks(&content)?;
        } else {
            input.step(|cursor| {
                cursor
                    .token_tree()
                    .map(|(_, next)| ((), next))
                    .ok_or_else(|| cursor.error("expected macro token"))
            })?;
        }
    }
    Ok(found)
}

fn single_macro_attribute(input: syn::parse::ParseStream<'_>) -> syn::Result<Attribute> {
    let pound_token = input.parse()?;
    let style = if input.peek(syn::Token![!]) {
        syn::AttrStyle::Inner(input.parse()?)
    } else {
        syn::AttrStyle::Outer
    };
    let content;
    let bracket_token = syn::bracketed!(content in input);
    Ok(Attribute {
        pound_token,
        style,
        bracket_token,
        meta: content.parse()?,
    })
}

#[test]
fn private_test_scanner_rejects_nested_hooks_outside_audited_top_level_items() {
    let member = "crates/gateway-core";
    let path = Path::new("runtime/account_concurrency.rs");
    for source in [
        "#![cfg(test)] fn work() {}",
        "impl Handle { #[cfg(test)] fn test_hook() {} }",
        "trait Port { #[cfg(test)] fn test_hook(); }",
        "fn work() { #[cfg(test)] fn test_hook() {} }",
        "fn work() { #[cfg(test)] let conditional = 1; }",
        "fn work() { #[cfg(test)] mod tests {} }",
        "impl Handle { #[cfg_attr(test, inline)] fn test_hook() {} }",
        "fn work() { #[path = \"other.rs\"] mod tests; }",
        "#[cfg(test)] mod tests {} impl Handle { #[cfg(test)] fn test_hook() {} }",
    ] {
        let syntax = syn::parse_file(source).unwrap();
        assert!(has_unaudited_test_hooks(member, path, &syntax), "{source}");
    }
    let syntax = syn::parse_file(
        "#[cfg(test)] mod tests { #[test] fn boundary() {} }
         impl Handle { fn work() {} }",
    )
    .unwrap();
    assert!(!has_unaudited_test_hooks(member, path, &syntax));
    let syntax =
        syn::parse_file("fn work() { let text = \"#[cfg(test)] is documentation, not a hook\"; }")
            .unwrap();
    assert!(!has_unaudited_test_hooks(member, path, &syntax));
}

#[test]
fn private_test_scanner_rejects_macro_hooks_without_rejecting_literal_documentation() {
    let member = "crates/gateway-core";
    let path = Path::new("runtime/account_concurrency.rs");
    for source in [
        "macro_rules! add_hook { () => { #[cfg(test)] pub fn hidden_hook() {} }; } add_hook!();",
        "generated! { impl Handle { #[cfg(test)] fn hidden_hook() {} } }",
        "generated!([nested(#[cfg(test)] fn hidden_hook() {})]);",
        "generated! { #[path = \"hidden.rs\"] mod hidden; }",
        "generated! { #[cfg_attr(test, inline)] fn hidden_hook() {} }",
        "macro_rules! emit {
            (#[$attr:meta] # $name:ident) => { #[$attr] pub fn $name() {} };
         }
         emit!(#[cfg(test)] # hidden_hook);",
        "generated!(#[cfg(test)] # hidden_hook);",
        "generated!(#![cfg(test)] # hidden_hook);",
    ] {
        let syntax = syn::parse_file(source).unwrap();
        assert!(has_unaudited_test_hooks(member, path, &syntax), "{source}");
    }
    for source in [
        "generated! { impl Handle { fn work() {} } }",
        r##"println!("Example: #[cfg(test)]");"##,
        r###"println!(r#"Example: #[cfg(test)]"#);"###,
    ] {
        let syntax = syn::parse_file(source).unwrap();
        assert!(!has_unaudited_test_hooks(member, path, &syntax), "{source}");
    }
}

#[test]
fn private_test_allowlist_requires_exact_owner_name_and_test_only_gate() {
    let owner = "crates/providers/openai";
    let path = Path::new("transport/websocket/pool/mod.rs");
    for name in ["tests", "lifecycle_tests"] {
        let item: Item = syn::parse_str(&format!("#[cfg(test)] mod {name};")).unwrap();
        assert!(is_audited_private_test(owner, path, &item));
        assert!(!is_audited_private_test("crates/gateway-api", path, &item));
        assert!(!is_audited_private_test(
            owner,
            Path::new("transport/other.rs"),
            &item
        ));
    }
    for source in [
        "mod tests;",
        "#[cfg(test)] mod unexpected;",
        "#[cfg(test)] pub mod tests;",
        "#[cfg(any(test, feature = \"production\"))] mod tests;",
        "#[cfg_attr(feature = \"production\", cfg(test))] mod tests;",
        "#[cfg(test)] #[path = \"other.rs\"] mod tests;",
        "#[cfg(test)] #[cfg_attr(test, path = \"other.rs\")] mod tests;",
        "#[cfg(test)] #[cfg_attr(unix, path = \"other.rs\")] mod tests;",
        "#[cfg(test)] mod tests {}",
        "#[cfg(test)] fn tests() {}",
    ] {
        let item: Item = syn::parse_str(source).unwrap();
        assert!(!is_audited_private_test(owner, path, &item), "{source}");
    }
}

#[test]
fn affinity_behavior_tests_do_not_gain_private_state_exceptions() {
    let member = "crates/providers/openai";
    for relative in [
        "credential/affinity.rs",
        "credential/affinity/mod.rs",
        "credential/affinity/tests.rs",
    ] {
        let path = Path::new(relative);
        for source in [
            "#[cfg(test)] mod tests;",
            "#[cfg(test)] mod tests {}",
            "#[cfg(test)] pub mod tests;",
            "#[cfg(test)] pub(crate) fn affinity_test_api() {}",
            "#[cfg(test)] #[test] fn affinity_test() {}",
            "#[cfg(any(test, feature = \"production\"))] mod tests;",
            "#[cfg(test)] #[path = \"tests.rs\"] mod tests;",
            "#[cfg_attr(test, path = \"tests.rs\")] mod tests;",
            "mod production { #[cfg(test)] fn affinity_test() {} }",
            "generated! { #[cfg(test)] pub fn affinity_test_api() {} }",
        ] {
            let syntax = syn::parse_file(source).unwrap();
            assert!(
                has_unaudited_test_hooks(member, path, &syntax),
                "{relative}: {source}"
            );
            for item in &syntax.items {
                assert!(
                    !is_audited_private_test(member, path, item),
                    "{relative}: {source}"
                );
            }
        }
    }
}

#[test]
fn private_inline_tests_do_not_allow_other_production_modules_or_functions() {
    let item: Item = syn::parse_str("#[cfg(test)] mod tests {}").unwrap();
    let core_path = Path::new("runtime/account_concurrency.rs");
    assert!(is_audited_private_test(
        "crates/gateway-core",
        core_path,
        &item
    ));
    assert!(!is_audited_private_test(
        "crates/gateway-core",
        Path::new("runtime/mod.rs"),
        &item,
    ));
    assert!(is_audited_private_test(
        "crates/providers/openai",
        Path::new("transport/excel/replay.rs"),
        &item,
    ));
    assert!(!is_audited_private_test(
        "crates/providers/openai",
        Path::new("provider/mod.rs"),
        &item,
    ));

    let path = Path::new("transport/websocket/coordinator.rs");
    let name = "shared_opening_deadline_does_not_double_charge_account_capacity_wait";
    let source = format!("#[cfg(test)] #[test] fn {name}() {{}}");
    let item: Item = syn::parse_str(&source).unwrap();
    assert!(is_audited_private_test(
        "crates/providers/openai",
        path,
        &item
    ));
    for source in [
        format!("#[cfg(test)] fn {name}() {{}}"),
        "#[cfg(test)] #[test] fn other_test() {}".to_owned(),
        format!("#[cfg(test)] #[test] pub fn {name}() {{}}"),
    ] {
        let item: Item = syn::parse_str(&source).unwrap();
        assert!(
            !is_audited_private_test("crates/providers/openai", path, &item),
            "{source}"
        );
    }
}

#[test]
fn excel_private_tests_require_exact_owner_and_do_not_expose_test_apis() {
    let owner = "crates/providers/openai";
    for relative in [
        "provider/excel.rs",
        "transport/excel/request.rs",
        "transport/excel/replay.rs",
        "transport/excel/tools.rs",
        "transport/excel/images.rs",
        "transport/excel/image_relay.rs",
        "transport/excel/stream.rs",
        "transport/excel/mod.rs",
    ] {
        let path = Path::new(relative);
        let declaration = if relative.ends_with("/mod.rs") {
            "#[cfg(test)] mod tests;"
        } else {
            "#[cfg(test)] mod tests {}"
        };
        let item: Item = syn::parse_str(declaration).unwrap();
        assert!(is_audited_private_test(owner, path, &item));
        assert!(!is_audited_private_test("crates/gateway-core", path, &item));
        for source in [
            "#[cfg(test)] pub(crate) mod tests {}",
            "#[cfg(test)] pub mod tests;",
            "#[cfg(test)] mod other {}",
            "#[cfg(test)] pub fn test_api() {}",
            "#[cfg_attr(test, path = \"fixture.rs\")] mod tests;",
        ] {
            let item: Item = syn::parse_str(source).unwrap();
            assert!(
                !is_audited_private_test(owner, path, &item),
                "{relative}: {source}"
            );
        }
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}
