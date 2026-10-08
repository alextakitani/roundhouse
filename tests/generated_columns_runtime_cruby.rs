//! A generated-column construct is supported only when the emitted model
//! runs cleanly against real SQLite DDL and reads the database-computed
//! value. Rails 8.1's SQLite oracle keeps update values stale until explicit
//! reload, while create's RETURNING hydrates the generated field before hooks.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            include_str!("support/generated_columns_schema.rb"),
        )
        .write(
            "app/models/person.rb",
            include_str!("support/generated_columns_person.rb"),
        )
        .write(
            "app/models/virtual_person.rb",
            include_str!("support/generated_columns_virtual_person.rb"),
        )
        .write(
            "app/models/constant_person.rb",
            include_str!("support/generated_columns_constant_person.rb"),
        )
}

#[test]
fn emitted_cruby_models_read_and_persist_generated_columns_like_rails() {
    let run = app().run_ruby(include_str!("support/generated_columns_contract.rb"));
    run.assert_passes();
    assert!(
        run.stdout
            .contains("generated column create/update/reload contract passed")
    );
}

#[test]
fn direct_bulk_writes_of_generated_keys_preserve_database_rejection() {
    app()
        .run_ruby(
            r##"
person = Person.create!(first_name: "Ada", last_name: "Lovelace")
cases = [
  ["update_all", "UPDATE", -> { Person.where(id: person.id).update_all(display_name: "forged-update-all") }],
  ["upsert_all", "INSERT", -> { Person.upsert_all([{ first_name: "Bulk", last_name: "Upsert", display_name: "forged-upsert-all" }]) }],
]
cases.each do |name, verb, operation|
  error = nil
  attempted_sql = Db.capture_sql do
    begin
      operation.call
    rescue StandardError => failure
      error = failure
    end
  end
  raise "#{name} silently accepted an explicit generated value" if error.nil?
  raise "#{name} failed for the wrong reason: #{error.class}: #{error.message}" unless error.message.downcase.include?("generated column")
  raise "#{name} did not reach SQLite with the explicit generated key: #{attempted_sql.inspect}" unless attempted_sql.any? { |sql| sql.include?(verb) && sql.include?("display_name") }
end
puts "explicit bulk generated-column writes remain rejected"
"##,
        )
        .assert_passes();
}

#[test]
fn model_insert_all_with_generated_key_is_a_located_compile_error() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let model = r#"class Person < ApplicationRecord
  def unsafe_generated_insert
    Person.insert_all([{ first_name: "Bulk", last_name: "Insert", display_name: "forged" }])
  end
end
"#;
    let (_emitted, app, diagnostics) = app()
        .write("app/models/person.rb", model)
        .emit_with_app(BuildTarget::Ruby);

    let diagnostic = diagnostics.iter().find(|diagnostic| {
        if diagnostic.severity != Severity::Error
            || !diagnostic.message.to_lowercase().contains("generated")
        {
            return false;
        }
        roundhouse::ide::source(&app, diagnostic.span.file).is_some_and(|source| {
            Path::new(&source.path).ends_with("app/models/person.rb")
                && source
                    .text
                    .lines()
                    .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                    .is_some_and(|line| line.contains("Person.insert_all"))
        })
    });
    let diagnostic = diagnostic.expect(
        "app-method insert_all of a generated value must not lower to repeated saves and pass with zero errors",
    );
    assert!(
        !diagnostic.span.is_synthetic(),
        "guard must retain a source location: {diagnostic:?}"
    );
    let source = roundhouse::ide::source(&app, diagnostic.span.file).expect("guard source");
    assert!(
        Path::new(&source.path).ends_with("app/models/person.rb"),
        "{}",
        source.path
    );
}

#[test]
fn generated_instance_writes_are_guarded_at_their_source_calls() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let model = r#"class Person < ApplicationRecord
  def explicit_generated_update
    self.update_column(:display_name, "forged")
  end

  def explicit_generated_update_with_block
    self.update_column(:display_name, "forged") { 1 }
  end

  def implicit_generated_update
    update_column(:display_name, "forged")
  end

  def dynamic_generated_update(column_name)
    update_column(column_name, "forged")
  end

  def generated_touch
    touch(:display_name)
  end

  def dynamic_generated_touch(column_name)
    touch(column_name)
  end
end
"#;
    let (_emitted, app, diagnostics) = app()
        .write("app/models/person.rb", model)
        .emit_with_app(BuildTarget::Ruby);

    let assert_guard = |path: &str, call: &str| {
        let diagnostic = diagnostics.iter().find(|diagnostic| {
            if diagnostic.severity != Severity::Error
                || !diagnostic.message.to_lowercase().contains("generated")
            {
                return false;
            }
            roundhouse::ide::source(&app, diagnostic.span.file).is_some_and(|source| {
                Path::new(&source.path).ends_with(path)
                    && source
                        .text
                        .lines()
                        .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                        .is_some_and(|line| line.contains(call))
            })
        });
        let diagnostic = diagnostic.unwrap_or_else(|| panic!("missing located generated-write guard for {path}: {call}; diagnostics: {diagnostics:?}"));
        assert!(
            !diagnostic.span.is_synthetic(),
            "guard must retain a source location: {diagnostic:?}"
        );
    };

    assert_guard("app/models/person.rb", "self.update_column(:display_name");
    assert_guard(
        "app/models/person.rb",
        "self.update_column(:display_name, \"forged\") { 1 }",
    );
    assert_guard("app/models/person.rb", "    update_column(:display_name");
    assert_guard("app/models/person.rb", "    update_column(column_name");
    assert_guard("app/models/person.rb", "    touch(:display_name");
    assert_guard("app/models/person.rb", "    touch(column_name");
}

#[test]
fn typed_controller_receiver_is_guarded_and_ordinary_update_column_is_allowed() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let controller = r#"class GeneratedColumnWritesController < ApplicationController
  def unsafe_update
    person = Person.find(1)
    person.update_column(:display_name, "forged")
  end
end
"#;
    let (_emitted, analyzed_app, diagnostics) = app()
        .write(
            "app/controllers/generated_column_writes_controller.rb",
            controller,
        )
        .emit_with_app(BuildTarget::Ruby);
    let diagnostic = diagnostics.iter().find(|diagnostic| {
        if diagnostic.severity != Severity::Error
            || !diagnostic.message.to_lowercase().contains("generated")
        {
            return false;
        }
        roundhouse::ide::source(&analyzed_app, diagnostic.span.file).is_some_and(|source| {
            Path::new(&source.path)
                .ends_with("app/controllers/generated_column_writes_controller.rb")
                && source
                    .text
                    .lines()
                    .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                    .is_some_and(|line| line.contains("person.update_column(:display_name"))
        })
    });
    let diagnostic = diagnostic
        .expect("typed model variable update must have a source-located generated-column guard");
    assert!(
        !diagnostic.span.is_synthetic(),
        "guard must retain a source location: {diagnostic:?}"
    );

    let ordinary_controller = r#"class GeneratedColumnWritesController < ApplicationController
  def safe_update
    person = Person.find(1)
    person.update_column(:first_name, "Grace")
  end
end
"#;
    let (_emitted, _app, diagnostics) = app()
        .write(
            "app/controllers/generated_column_writes_controller.rb",
            ordinary_controller,
        )
        .emit_with_app(BuildTarget::Ruby);
    assert!(
        diagnostics.is_empty(),
        "ordinary-column update_column should remain supported: {diagnostics:?}"
    );
}

#[test]
fn union_typed_receiver_is_guarded_when_any_model_branch_has_generated_columns() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let mut schema = include_str!("support/generated_columns_schema.rb").to_string();
    let schema_end = schema.rfind("end\n").expect("schema's outer end");
    schema.insert_str(
        schema_end,
        "  create_table \"plain_people\", force: :cascade do |t|\n    t.string \"name\"\n  end\n\n",
    );
    let controller = r#"class GeneratedColumnWritesController < ApplicationController
  def unsafe_union_write(use_generated, column_name)
    person = use_generated ? Person.find(1) : PlainPerson.find(1)
    person.update_column(:display_name, "forged")
  end

  def unsafe_union_dynamic_write(use_generated, column_name)
    person = use_generated ? Person.find(1) : PlainPerson.find(1)
    person.update_column(column_name, "forged")
  end
end
"#;
    let (_emitted, analyzed_app, diagnostics) = app()
        .write("db/schema.rb", &schema)
        .write(
            "app/models/plain_person.rb",
            "class PlainPerson < ApplicationRecord\nend\n",
        )
        .write(
            "app/controllers/generated_column_writes_controller.rb",
            controller,
        )
        .emit_with_app(BuildTarget::Ruby);

    for call in [
        "person.update_column(:display_name",
        "person.update_column(column_name",
    ] {
        let diagnostic = diagnostics.iter().find(|diagnostic| {
            if diagnostic.severity != Severity::Error
                || !diagnostic.message.to_lowercase().contains("generated")
            {
                return false;
            }
            roundhouse::ide::source(&analyzed_app, diagnostic.span.file).is_some_and(|source| {
                Path::new(&source.path)
                    .ends_with("app/controllers/generated_column_writes_controller.rb")
                    && source
                        .text
                        .lines()
                        .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                        .is_some_and(|line| line.contains(call))
            })
        });
        let diagnostic = diagnostic.unwrap_or_else(|| {
            panic!("missing union generated-write guard for {call}; diagnostics: {diagnostics:?}")
        });
        assert!(
            !diagnostic.span.is_synthetic(),
            "guard must retain a source location: {diagnostic:?}"
        );
    }
}

#[test]
fn generated_bulk_writes_in_view_and_test_roots_are_located() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let roots = [
        (
            "app/views/people/index.html.erb",
            "<% Person.insert_all([{ first_name: \"Bulk\", last_name: \"View\", display_name: \"forged\" }]) %>\n",
        ),
        (
            "test/models/person_generated_column_test.rb",
            "class PersonGeneratedColumnTest < ActiveSupport::TestCase\n  test \"bulk write\" do\n    Person.insert_all([{ first_name: \"Bulk\", last_name: \"Test\", display_name: \"forged\" }])\n  end\nend\n",
        ),
    ];

    for (path, source_text) in roots {
        let (_emitted, analyzed_app, diagnostics) = app()
            .write(path, source_text)
            .emit_with_app(BuildTarget::Ruby);
        let diagnostic = diagnostics.iter().find(|diagnostic| {
            if diagnostic.severity != Severity::Error
                || !diagnostic.message.to_lowercase().contains("generated")
            {
                return false;
            }
            roundhouse::ide::source(&analyzed_app, diagnostic.span.file).is_some_and(|source| {
                Path::new(&source.path).ends_with(path)
                    && source
                        .text
                        .lines()
                        .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                        .is_some_and(|line| line.contains("Person.insert_all"))
            })
        });
        let diagnostic = diagnostic.unwrap_or_else(|| {
            panic!("missing located generated-write guard for {path}; diagnostics: {diagnostics:?}")
        });
        assert!(
            !diagnostic.span.is_synthetic(),
            "guard must retain a source location: {diagnostic:?}"
        );
    }
}

fn app_with_association_touch(person_body: &str, association: &str) -> emit_and_run::Overlay {
    let mut schema = include_str!("support/generated_columns_schema.rb").to_string();
    schema = schema.replacen(
        "    t.string \"last_name\"\n",
        "    t.string \"last_name\"\n    t.datetime \"last_seen_at\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n",
        1,
    );
    let schema_end = schema.rfind("end\n").expect("schema's outer end");
    schema.insert_str(
        schema_end,
        "  create_table \"comments\", force: :cascade do |t|\n    t.integer \"person_id\"\n    t.integer \"notifiable_id\"\n    t.string \"notifiable_type\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n",
    );
    app()
        .write("db/schema.rb", &schema)
        .write(
            "app/models/person.rb",
            &format!("class Person < ApplicationRecord\n{person_body}end\n"),
        )
        .write(
            "app/models/comment.rb",
            &format!("class Comment < ApplicationRecord\n  {association}\nend\n"),
        )
}

fn assert_association_touch_guard(
    app: &roundhouse::App,
    diagnostics: &[roundhouse::diagnostic::Diagnostic],
    expected_source_line: &str,
) {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;

    let diagnostic = diagnostics.iter().find(|diagnostic| {
        if diagnostic.severity != Severity::Error
            || !diagnostic
                .message
                .to_lowercase()
                .contains("generated column")
        {
            return false;
        }
        roundhouse::ide::source(app, diagnostic.span.file).is_some_and(|source| {
            Path::new(&source.path).ends_with("app/models/comment.rb")
                && source
                    .text
                    .lines()
                    .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                    .is_some_and(|line| line.contains(expected_source_line))
        })
    });
    let diagnostic = diagnostic.unwrap_or_else(|| {
        panic!(
            "missing source-located generated association-touch guard for {expected_source_line}; diagnostics: {diagnostics:?}"
        )
    });
    assert!(
        !diagnostic.span.is_synthetic(),
        "association guard must retain the source declaration span: {diagnostic:?}"
    );
}

#[test]
fn association_touch_guards_generated_targets_and_preserves_ordinary_columns() {
    use roundhouse::project::BuildTarget;

    let (_emitted, analyzed_app, diagnostics) = app_with_association_touch(
        "",
        "belongs_to :author, class_name: \"Person\", foreign_key: :person_id, touch: :display_name",
    )
    .emit_with_app(BuildTarget::Ruby);
    assert_association_touch_guard(&analyzed_app, &diagnostics, "touch: :display_name");

    let (_emitted, _analyzed_app, diagnostics) = app_with_association_touch(
        "",
        "belongs_to :author, class_name: \"Person\", foreign_key: :person_id, touch: :last_seen_at",
    )
    .emit_with_app(BuildTarget::Ruby);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != roundhouse::diagnostic::Severity::Error),
        "ordinary belongs_to touch should remain supported: {diagnostics:?}"
    );
}

#[test]
fn polymorphic_association_touch_guards_resolved_and_unresolved_generated_targets() {
    use roundhouse::dialect::Association;
    use roundhouse::project::BuildTarget;

    let association = "belongs_to :notifiable, polymorphic: true, touch: :display_name";
    let (_emitted, analyzed_app, diagnostics) =
        app_with_association_touch("  has_many :comments, as: :notifiable\n", association)
            .emit_with_app(BuildTarget::Ruby);
    let comment = analyzed_app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Comment")
        .expect("Comment model");
    let Association::BelongsTo {
        polymorphic_targets,
        ..
    } = comment
        .associations()
        .find(|association| association.name().as_str() == "notifiable")
        .expect("notifiable association")
    else {
        panic!("expected polymorphic belongs_to");
    };
    assert_eq!(
        polymorphic_targets
            .iter()
            .map(|target| target.0.as_str())
            .collect::<Vec<_>>(),
        vec!["Person"],
        "resolved inverse target must exercise the resolved-target guard branch"
    );
    assert_association_touch_guard(&analyzed_app, &diagnostics, "touch: :display_name");

    let (_emitted, analyzed_app, diagnostics) =
        app_with_association_touch("", association).emit_with_app(BuildTarget::Ruby);
    let comment = analyzed_app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Comment")
        .expect("Comment model");
    let Association::BelongsTo {
        polymorphic_targets,
        ..
    } = comment
        .associations()
        .find(|association| association.name().as_str() == "notifiable")
        .expect("notifiable association")
    else {
        panic!("expected polymorphic belongs_to");
    };
    assert!(
        polymorphic_targets.is_empty(),
        "no inverse or type literal should exercise the unresolved-target guard branch"
    );
    assert_association_touch_guard(&analyzed_app, &diagnostics, "touch: :display_name");

    let (_emitted, _analyzed_app, diagnostics) = app_with_association_touch(
        "  has_many :comments, as: :notifiable\n",
        "belongs_to :notifiable, polymorphic: true, touch: :last_seen_at",
    )
    .emit_with_app(BuildTarget::Ruby);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != roundhouse::diagnostic::Severity::Error),
        "ordinary polymorphic touch should remain supported: {diagnostics:?}"
    );
}
