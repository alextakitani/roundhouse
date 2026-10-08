//! Column-option forms that cannot be inspected must fail closed, so a
//! generated column cannot silently enter the schema as an ordinary column.

use roundhouse::ingest::{ingest_migration, ingest_schema};
use roundhouse::schema::Schema;

fn schema_error(source: &str) -> String {
    ingest_schema(source.as_bytes(), "db/schema.rb")
        .expect_err("unresolved generated-column options must be rejected")
        .to_string()
}

fn migration_error(source: &str) -> String {
    let mut schema = Schema::default();
    ingest_migration(
        source.as_bytes(),
        "db/migrate/add_generated_label.rb",
        &mut schema,
    )
    .expect_err("unresolved generated-column options must be rejected")
    .to_string()
}

#[test]
fn schema_column_option_splats_do_not_become_ordinary_columns() {
    for splat in [
        "**options",
        r#"**{ as: "first_name || ' ' || last_name", stored: true }"#,
    ] {
        let source = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.string "display_name", {splat}
  end
end
"#
        );
        let error = schema_error(&source);
        assert!(error.contains("db/schema.rb"), "{error}");
        assert!(error.contains("display_name"), "{error}");
        assert!(error.contains("keyword splats"), "{error}");
    }
}

#[test]
fn migration_column_option_splats_do_not_become_ordinary_columns() {
    for splat in [
        "**options",
        r#"**{ as: "first_name || ' ' || last_name", stored: true }"#,
    ] {
        let source = format!(
            r#"class AddDisplayName < ActiveRecord::Migration[8.1]
  def change
    add_column :people, :display_name, :string, {splat}
  end
end
"#
        );
        let error = migration_error(&source);
        assert!(
            error.contains("db/migrate/add_generated_label.rb"),
            "{error}"
        );
        assert!(error.contains("people.display_name"), "{error}");
        assert!(error.contains("keyword splats"), "{error}");
    }
}

#[test]
fn braced_generated_options_are_not_ignored_as_positional_hashes() {
    let schema_source = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "display_name", { as: "first_name", stored: true }
  end
end
"#;
    let error = schema_error(schema_source);
    assert!(error.contains("db/schema.rb"), "{error}");
    assert!(error.contains("generated column options"), "{error}");

    let migration_source = r#"class AddDisplayName < ActiveRecord::Migration[8.1]
  def change
    add_column :people, :display_name, :string, { as: "first_name", stored: true }
  end
end
"#;
    let error = migration_error(migration_source);
    assert!(
        error.contains("db/migrate/add_generated_label.rb"),
        "{error}"
    );
    assert!(error.contains("generated column options"), "{error}");
}
