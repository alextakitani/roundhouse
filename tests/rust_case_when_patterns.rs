//! Rust renders `case`/`when` as a `match`. Only literal, binding and
//! wildcard patterns have a `match` form; a range, class or endless-range
//! `when` used to come out as `_`, so the first such arm won for every
//! input (`case -3 when 0..3 then "low" ... else "neg"` returned "low").
//! Those shapes are reported unsupported instead, as Python and
//! TypeScript already do.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::emit::diagnostics::scope;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{BuildTarget, target_files};

const SKIP: &[&str] = &["tmp", "log", "storage", "node_modules", ".git"];

fn read_tree(root: &Path, dir: &Path, out: &mut HashMap<PathBuf, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let rel = path.strip_prefix(root).unwrap().to_path_buf();
        if path.is_dir() {
            if !SKIP.iter().any(|s| rel == Path::new(s)) {
                read_tree(root, &path, out);
            }
        } else {
            out.insert(rel, std::fs::read(&path).unwrap());
        }
    }
}

/// real-blog with `Article.probe(x)` added; returns the emitted
/// `article.rs` and the error diagnostics.
fn emit_rust_with_probe(body: &str) -> (String, Vec<Diagnostic>, String) {
    let root = roundhouse::fixtures::real_blog();
    let mut tree = HashMap::new();
    read_tree(root, root, &mut tree);
    let model = PathBuf::from("app/models/article.rb");
    let source = String::from_utf8(tree[&model].clone()).unwrap().replacen(
        "class Article < ApplicationRecord\n",
        &format!("class Article < ApplicationRecord\n  def self.probe(x)\n    {body}\n  end\n"),
        1,
    );
    tree.insert(model, source.clone().into_bytes());
    let mut app = ingest_app_from_tree(tree).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    let (files, diagnostics) = scope(|| target_files(&app, root, BuildTarget::Rust));
    let article = files
        .unwrap()
        .into_iter()
        .find(|(path, _)| path.ends_with("models/article.rs"))
        .map(|(_, text)| text)
        .unwrap_or_default();
    let errors = diagnostics.into_iter().filter(|d| d.severity == Severity::Error).collect();
    (article, errors, source)
}

#[test]
fn non_literal_when_patterns_are_unsupported_not_wildcards() {
    // Each body pairs with the arm the diagnostic must point at.
    for (body, failing) in [
        ("case x\n    when 0..3 then \"low\"\n    when 4..9 then \"high\"\n    else \"neg\"\n    end", "0..3"),
        ("case x\n    when 0...3 then \"low\"\n    else \"other\"\n    end", "0...3"),
        ("case x\n    when ..0 then \"nonpos\"\n    when 10.. then \"big\"\n    else \"mid\"\n    end", "..0"),
        ("case x\n    when String then \"s\"\n    when Integer then \"i\"\n    else \"o\"\n    end", "String"),
        ("case x\n    when 0 then \"zero\"\n    when 1..5 then \"few\"\n    else \"many\"\n    end", "1..5"),
    ] {
        let (article, errors, source) = emit_rust_with_probe(body);
        assert!(!errors.is_empty(), "rust accepted {body}:\n{article}");
        assert_eq!(errors.len(), 1, "{body}: {errors:?}");
        assert!(
            matches!(&errors[0].kind, DiagnosticKind::Unsupported { construct, target: Some(t), .. }
                if construct.as_str() == "Case" && t.as_str() == "rust"),
            "{body}: {errors:?}"
        );
        let span = errors[0].span;
        assert!(!span.is_synthetic(), "lost source span");
        assert_eq!(&source[span.start as usize..span.end as usize], failing, "{body}: span");
    }
}

#[test]
fn literal_when_patterns_still_emit_a_match() {
    let (article, errors, _) = emit_rust_with_probe(
        "case x\n    when 1, 2 then \"small\"\n    when 3 then \"three\"\n    else \"other\"\n    end",
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert!(article.contains("3 => "), "{article}");
}
