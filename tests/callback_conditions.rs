//! `if:` / `unless:` on a model callback. Ingest used to reject any
//! callback carrying one, so the callback was dropped entirely: a
//! `before_validation :assign_color, on: :create, if: -> { color.blank? }`
//! never ran, the column stayed blank, and the record failed validation.
//! A zero-arity lambda body is spliced into a guard (Rails
//! `instance_exec`s it with `self` the record); a Symbol is the predicate
//! method called on the record.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :color\n    t.string :name\n  end\nend\n";

const WIDGET: &str = r##"class Widget < ApplicationRecord
  before_validation :assign_color, on: :create, if: -> { color.blank? }
  before_save :shout, unless: :loud?
  before_destroy :noop, if: :frozen?

  private
    def assign_color
      self.color = "#000000"
    end

    def loud?
      name == "LOUD"
    end

    def shout
      self.name = name.to_s.upcase
    end

    def frozen?
      false
    end
end
"##;

fn emitted() -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        ("app/models/widget.rb", WIDGET),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.ends_with("widget.rb"))
        .expect("widget.rb emitted")
        .content
        .clone()
}

#[test]
fn a_lambda_if_condition_guards_the_callback() {
    let src = emitted();
    assert!(
        src.contains("assign_color"),
        "the callback must not be dropped:\n{src}"
    );
    assert!(
        src.contains("ActiveSupport.blank?(self.color)"),
        "the lambda condition must guard the callback:\n{src}"
    );
}

#[test]
fn a_symbol_unless_condition_negates_the_predicate() {
    let src = emitted();
    // `before_save :shout, unless: :loud?` → `unless self.loud?`.
    assert!(
        src.contains("shout") && src.contains("loud?"),
        "the unless callback and its predicate must survive:\n{src}"
    );
    // `before_destroy :noop, if: :frozen?` — the method exists and is
    // guarded by the predicate.
    assert!(src.contains("frozen?"), "{src}");
}
