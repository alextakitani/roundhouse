person_sql = nil
person = Person.new(first_name: "Ada", last_name: "Lovelace")
raise "non-null generated value should be nil before persistence" unless person.normalized_first_name.nil?
person_sql = Db.capture_sql do
  person.save!
end
raise "create did not return the generated value: #{person.display_name.inspect}" unless person.display_name == "Ada Lovelace"
raise "non-null generated value was not hydrated after create" unless person.normalized_first_name == "Ada"
raise "after_create did not see the generated value" unless person.generated_after_create == "Ada Lovelace"
raise "after_save did not see the generated value" unless person.generated_after_save == "Ada Lovelace"
insert_sql = person_sql.find { |sql| sql.include?("INSERT INTO") && sql.include?("people") }
raise "missing INSERT: #{person_sql.inspect}" if insert_sql.nil?
insert_columns = insert_sql.split("VALUES").first
raise "generated column was written on create: #{insert_sql}" if insert_columns.include?("display_name")

update_sqls = Db.capture_sql do
  person.first_name = "Grace"
  person.save!
end
update_sql = update_sqls.find { |sql| sql.include?("UPDATE") && sql.include?("people") }
raise "missing source-column UPDATE: #{update_sqls.inspect}" if update_sql.nil?
update_set = update_sql.split("WHERE").first
raise "generated column was written on update: #{update_sql}" if update_set.include?("display_name")
raise "Rails-compatible update cache changed before reload" unless person.display_name == "Ada Lovelace"
raise "non-null generated cache changed before reload" unless person.normalized_first_name == "Ada"
raise "after_update did not see the pre-update generated value" unless person.generated_after_update == "Ada Lovelace"
raise "after_save did not see the pre-update generated value" unless person.generated_after_save == "Ada Lovelace"
person.reload
raise "reload did not return the recomputed value" unless person.display_name == "Grace Lovelace"
raise "reload did not refresh the non-null generated value" unless person.normalized_first_name == "Grace"

coalesced = Person.create!(first_name: "Grace", last_name: nil)
raise "coalesce(NULL, '') should produce an empty suffix: #{coalesced.display_name.inspect}" unless coalesced.display_name == "Grace "
null_concat = Person.create!(first_name: nil, last_name: "Hopper")
raise "NULL concatenation should produce NULL: #{null_concat.display_name.inspect}" unless null_concat.display_name.nil?
raise "after_create missed the NULL result" unless null_concat.generated_after_create.nil?

nil_assignment = Person.create!(first_name: "Grace", last_name: "Hopper", display_name: nil)
raise "explicit nil should be omitted and hydrated from the database" unless nil_assignment.display_name == "Grace Hopper"

forged = Person.create!(first_name: "Alan", last_name: "Turing", display_name: "forged-create")
raise "create should not write a forged generated value" unless forged.display_name == "forged-create"
raise "after_create should see the assigned in-memory value" unless forged.generated_after_create == "forged-create"
forged.reload
raise "reload should replace the forged value with the database value" unless forged.display_name == "Alan Turing"

# Rails 8.1 skips SQL when the generated value is the only changed field.
# Roundhouse's ordinary save path can still write unchanged source columns;
# this check keeps the generated field out of SET without widening scope to
# dirty-column optimization.
person.display_name = "forged-update"
generated_only_save_sql = Db.capture_sql { person.save! }
generated_only_update = generated_only_save_sql.find { |sql| sql.include?("UPDATE") && sql.include?("people") }
if generated_only_update
  generated_only_set = generated_only_update.split("WHERE").first
  raise "generated-only save wrote the generated column: #{generated_only_update}" if generated_only_set.include?("display_name")
end
raise "generated-only save should retain the assigned local value" unless person.display_name == "forged-update"
raise "generated-only save should reload the actual generated value" unless person.reload.display_name == "Grace Lovelace"

constant = nil
default_insert_sql = Db.capture_sql do
  constant = ConstantPerson.create!
end
raise "generated-only table did not return its computed value" unless constant.display_name == "constant"
default_insert = default_insert_sql.find { |sql| sql.include?("INSERT INTO") && sql.include?("constant_people") }
raise "expected an empty-column default-values insert: #{default_insert_sql.inspect}" if default_insert.nil? || !default_insert.include?("DEFAULT VALUES")
constant.display_name = "forged-constant"
constant_update_sql = Db.capture_sql { constant.save! }
raise "generated-only table should not issue an empty UPDATE: #{constant_update_sql.inspect}" if constant_update_sql.any? { |sql| sql.include?("UPDATE") && sql.include?("constant_people") }
raise "generated-only table reload should restore the computed value" unless constant.reload.display_name == "constant"

virtual = VirtualPerson.create!(first_name: "Virtual", last_name: "Value")
raise "virtual generated value missing on create" unless virtual.display_name == "Virtual Value"
virtual.first_name = "Updated Virtual"
virtual.save!
raise "virtual generated value should remain stale until reload" unless virtual.display_name == "Virtual Value"
raise "virtual reload should expose recalculation" unless virtual.reload.display_name == "Updated Virtual Value"

puts "generated column create/update/reload contract passed"
