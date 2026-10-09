//! A constant an initializer assigns is defined at boot, so the app's
//! reads of it are sound; with no home for it in the ingested tree yet,
//! those reads are coverage notes naming the initializer, while an
//! unrelated or namespaced namesake stays an error.

use std::process::Command;

fn check_continue(root: &std::path::Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["check", "--continue"])
        .arg(root)
        .output()
        .expect("spawn roundhouse");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn reads_of_a_constant_an_initializer_assigns_are_coverage_notes() {
    let root = std::env::temp_dir().join(format!("roundhouse-initializer-constants-{}", std::process::id()));
    for (path, source) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("config/initializers/000-stats.rb", "::STATS = Object.new\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n  def index\n    STATS.inspect\n    Nope.inspect\n    Admin::STATS.inspect\n    head :ok\n  end\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: [:index]\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }

    let out = check_continue(&root);
    let line = |needle: &str| out.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no `{needle}` line:\n{out}"));

    let stats = line("constant not supported (all targets): STATS");
    assert!(stats.contains("note[unsupported]"), "{stats}");
    assert!(stats.contains("assigned in config/initializers/000-stats.rb"), "{stats}");

    // Not assigned anywhere: an error of its own.
    let nope = line("constant not supported (all targets): Nope");
    assert!(nope.contains("error[unsupported]"), "{nope}");
    // The initializer assigns the top-level `STATS`, not `Admin::STATS`.
    let admin = line("constant not supported (all targets): Admin::STATS");
    assert!(admin.contains("error[unsupported]"), "{admin}");

    std::fs::remove_dir_all(root).unwrap();
}
