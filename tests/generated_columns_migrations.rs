//! Migration folds must preserve schema metadata when a generated-column
//! change is rejected, and must not retain stale generated/index references.

use roundhouse::emit::shared::schema_sql::render_schema_statements;
use roundhouse::ingest::{ingest_migration, ingest_schema};
use roundhouse::schema::{Column, ColumnType, GeneratedColumnStorage, Schema, Table};
use roundhouse::Symbol;

fn generated_schema() -> Schema {
    ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.string "nickname"
    t.string "obsolete"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || last_name", stored: true
  end
end
"#,
        "db/schema.rb",
    )
    .expect("generated-column schema should ingest")
}

fn ordinary_display_schema() -> Schema {
    ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.string "display_name", default: "legacy", null: false
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ordinary-column schema should ingest")
}

fn indexed_generated_schema() -> Schema {
    ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || last_name", stored: true
    t.index ["display_name"], name: "index_people_on_display_name"
  end
end
"#,
        "db/schema.rb",
    )
    .expect("indexed generated-column schema should ingest")
}

fn apply_migration(schema: &mut Schema, body: &str) -> Result<(), String> {
    ingest_migration(body.as_bytes(), "db/migrate/change_people.rb", schema)
        .map_err(|error| error.to_string())
}

fn table<'a>(schema: &'a Schema, name: &str) -> &'a Table {
    schema
        .tables
        .get(&Symbol::from(name))
        .unwrap_or_else(|| panic!("missing table {name}"))
}

fn column<'a>(table: &'a Table, name: &str) -> &'a Column {
    table
        .columns
        .iter()
        .find(|column| column.name.as_str() == name)
        .unwrap_or_else(|| panic!("missing column {name} on {}", table.name.as_str()))
}

fn one_operation(operation: &str) -> String {
    format!(
        "class ChangePeople < ActiveRecord::Migration[8.1]\n  def change\n    {operation}\n  end\nend\n"
    )
}

#[test]
fn invalid_generated_replacements_leave_the_original_table_unchanged() {
    for (mut schema, old_kind) in [
        (generated_schema(), "generated"),
        (ordinary_display_schema(), "ordinary"),
    ] {
        let original = schema.clone();
        let migration = one_operation(
            r#"add_column :people, :display_name, :string, as: "missing_column || ' replacement'", stored: true"#,
        );
        let error = apply_migration(&mut schema, &migration)
            .expect_err("invalid generated replacement must be rejected");
        assert!(error.contains("generated column"), "{error}");
        assert!(error.contains("people.display_name"), "{error}");
        assert_eq!(
            schema, original,
            "the {old_kind} field and full table metadata must survive"
        );

        let existing = column(table(&schema, "people"), "display_name");
        if old_kind == "generated" {
            let generated = existing
                .generated
                .as_ref()
                .expect("original generated field");
            assert_eq!(generated.expression, "first_name || ' ' || last_name");
            assert_eq!(generated.storage, GeneratedColumnStorage::Stored);
        } else {
            assert!(existing.generated.is_none());
            assert_eq!(existing.default.as_deref(), Some("legacy"));
            assert!(!existing.nullable);
        }
    }
}

#[test]
fn source_rename_or_removal_that_invalidates_an_expression_is_atomic() {
    let migrations = [
        (
            "rename_column",
            one_operation(
                "rename_column :people, :first_name, :given_name\n    add_column :people, :first_name, :string",
            ),
        ),
        (
            "remove_column",
            one_operation(
                "remove_column :people, :first_name\n    add_column :people, :first_name, :string",
            ),
        ),
    ];

    for (verb, migration) in migrations {
        let mut schema = generated_schema();
        let original = schema.clone();
        let error = apply_migration(&mut schema, &migration)
            .expect_err("a source-column change must not retarget a generated expression");
        assert!(error.contains("generated column"), "{error}");
        assert!(error.contains(verb), "{error}");
        assert!(error.contains("people.display_name"), "{error}");
        assert!(error.contains("unknown column"), "{error}");
        assert!(error.contains("first_name"), "{error}");
        assert_eq!(
            schema, original,
            "rejected migration must keep original table and SQL"
        );
    }
}

#[test]
fn indexed_generated_outputs_cannot_be_removed_or_renamed() {
    for operation in [
        "remove_column :people, :display_name",
        "rename_column :people, :display_name, :label",
    ] {
        let mut schema = indexed_generated_schema();
        let original = schema.clone();
        let error = apply_migration(&mut schema, &one_operation(operation))
            .expect_err("changing an indexed generated output must fail explicitly");
        assert!(error.contains("generated column"), "{error}");
        assert!(error.contains("people.display_name"), "{error}");
        assert_eq!(
            schema, original,
            "rejected migration must retain its column and index"
        );
    }
}

#[test]
fn unindexed_generated_outputs_can_be_renamed_or_removed() {
    let mut renamed = generated_schema();
    apply_migration(
        &mut renamed,
        &one_operation("rename_column :people, :display_name, :label"),
    )
    .expect("an unindexed generated output can be renamed safely");
    let renamed_table = table(&renamed, "people");
    assert!(renamed_table
        .columns
        .iter()
        .all(|column| column.name.as_str() != "display_name"));
    let label = column(renamed_table, "label");
    let generated = label
        .generated
        .as_ref()
        .expect("renamed generated metadata");
    assert_eq!(generated.expression, "first_name || ' ' || last_name");
    assert_eq!(generated.storage, GeneratedColumnStorage::Stored);
    let ddl = render_schema_statements(&renamed);
    assert!(
        ddl[0].contains("label TEXT GENERATED ALWAYS AS (first_name || ' ' || last_name) STORED")
    );
    assert!(!ddl[0].contains("display_name"));

    let mut removed = generated_schema();
    apply_migration(
        &mut removed,
        &one_operation("remove_column :people, :display_name"),
    )
    .expect("an unindexed generated output can be removed safely");
    assert!(table(&removed, "people")
        .columns
        .iter()
        .all(|column| column.name.as_str() != "display_name"));
    let removed_ddl = render_schema_statements(&removed);
    assert!(!removed_ddl[0].contains("display_name"));
}

#[test]
fn unrelated_ordinary_column_changes_preserve_generated_metadata() {
    let mut schema = generated_schema();
    apply_migration(
        &mut schema,
        &one_operation(
            "rename_column :people, :nickname, :handle\n    remove_column :people, :obsolete\n    change_column :people, :first_name, :text, null: false",
        ),
    )
    .expect("ordinary column changes remain supported");

    let people = table(&schema, "people");
    assert!(people
        .columns
        .iter()
        .any(|column| column.name.as_str() == "handle"));
    assert!(people
        .columns
        .iter()
        .all(|column| column.name.as_str() != "nickname"));
    assert!(people
        .columns
        .iter()
        .all(|column| column.name.as_str() != "obsolete"));
    let first_name = column(people, "first_name");
    assert!(matches!(&first_name.col_type, ColumnType::Text));
    assert!(!first_name.nullable);

    let generated = column(people, "display_name")
        .generated
        .as_ref()
        .expect("unrelated ordinary changes must preserve generated metadata");
    assert_eq!(generated.expression, "first_name || ' ' || last_name");
    assert_eq!(generated.storage, GeneratedColumnStorage::Stored);
}
