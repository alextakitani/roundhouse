//! A call on an instance answers the instance method when the class
//! object defines a method of the same name.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{Analyzer, diagnose};
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn analyzed(files: &[(&str, &str)], schema: &str) -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> =
        files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    tree.insert(PathBuf::from("db/schema.rb"), schema.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn ret(methods: &[roundhouse::dialect::MethodDef], method: &str, class_side: bool) -> Ty {
    let def = methods
        .iter()
        .find(|m| {
            m.name.as_str() == method
                && matches!(m.receiver, roundhouse::dialect::MethodReceiver::Class) == class_side
        })
        .expect("method");
    match &def.signature {
        Some(Ty::Fn { ret, .. }) => (**ret).clone(),
        _ => def.body.ty.clone().expect("typed body"),
    }
}

const EMPTY_SCHEMA: &str = "ActiveRecord::Schema.define do\nend\n";

const SERVICE: &str = "class Greeter
  def self.call(name)
    new(name).call
  end

  def initialize(name)
    @name = name
  end

  def call
    \"hello \" + @name
  end
end
";

/// `def self.call(...) = new(...).call`, the service-object shape forem
/// writes 107 times: the class-side `call` read the instance `call` as
/// itself and never got past `untyped`.
#[test]
fn a_class_method_calling_the_instance_method_of_its_name_answers_the_instance_return() {
    let app = analyzed(&[("app/services/greeter.rb", SERVICE)], EMPTY_SCHEMA);
    let greeter = app.library_classes.iter().find(|c| c.name.0.as_str() == "Greeter").expect("class");
    assert_eq!(ret(&greeter.methods, "call", true), Ty::Str);
}

#[test]
fn an_explicit_instance_receiver_answers_the_instance_method() {
    let app = analyzed(
        &[
            ("app/services/greeter.rb", SERVICE),
            ("app/services/caller.rb", "class Caller\n  def run\n    Greeter.new(\"x\").call\n  end\nend\n"),
        ],
        EMPTY_SCHEMA,
    );
    let caller = app.library_classes.iter().find(|c| c.name.0.as_str() == "Caller").expect("class");
    assert_eq!(ret(&caller.methods, "run", false), Ty::Str);
}

const POSTS_SCHEMA: &str = "ActiveRecord::Schema.define do
  create_table \"posts\" do |t|
    t.boolean \"active\", null: false
  end
end
";

/// A bare name in an instance method body is the instance's: the `active`
/// column, not the `active` scope.
#[test]
fn a_receiverless_call_in_an_instance_method_answers_the_instance_side() {
    let app = analyzed(
        &[(
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  scope :active, -> { where(active: true) }\n\n  def live?\n    active\n  end\nend\n",
        )],
        POSTS_SCHEMA,
    );
    let post = app.models.iter().find(|m| m.name.0.as_str() == "Post").expect("model");
    let methods: Vec<_> = post.methods().cloned().collect();
    assert_eq!(ret(&methods, "live?", false), Ty::Bool);
}

/// A scope body types with `class_side` false, yet its bare `active` is
/// the scope.
#[test]
fn a_receiverless_call_in_a_scope_body_still_answers_the_scope() {
    let app = analyzed(
        &[(
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  scope :active, -> { where(active: true) }\n  scope :recent_active, -> { active.order(:id) }\nend\n",
        )],
        POSTS_SCHEMA,
    );
    let errors: Vec<String> = diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}
