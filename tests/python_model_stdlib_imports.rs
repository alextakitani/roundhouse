//! A Python model method that lowers to the `re` module gets
//! `import re` in `app/v2/models.py`.
//!
//! `String#sub`/`#gsub` render as `re.sub(...)` and a regexp literal
//! as `re.compile(...)`. The overlay's model writer picks imports by
//! scanning the emitted body, and `re` was not on its list: the tree
//! transpiled clean and the method raised `NameError: name 're' is not
//! defined` when called. This overlays such methods onto real-blog,
//! emits Python, and runs the emitted model test.
//!
//!     cargo test --test python_model_stdlib_imports

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

use roundhouse::project::BuildTarget;

#[test]
fn model_regexp_methods_import_re() {
    let (emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\
             \n  def self.sub_probe\n    \"hello\".sub(\"l\", \"L\")\n  end\n\
             \n  def self.gsub_probe\n    \"hello\".gsub(/l+/, \"L\")\n  end\n",
        )
        .write(
            "test/models/probe_test.rb",
            "require \"test_helper\"\n\n\
             class ProbeTest < ActiveSupport::TestCase\n  \
               test \"regexp model methods\" do\n    \
                 assert_equal \"heLlo\", Article.sub_probe\n    \
                 assert_equal \"heLo\", Article.gsub_probe\n  \
               end\nend\n",
        )
        .emit(BuildTarget::Python);
    assert!(errors.is_empty(), "errors: {errors:?}");

    let models = std::fs::read_to_string(emitted.join("app/v2/models.py")).expect("models.py");
    assert!(models.contains("\nimport re\n"), "models.py lacks `import re`:\n{models}");

    let output = Command::new("python3")
        .args(["-m", "unittest", "tests.test_probe"])
        .current_dir(&emitted)
        .output()
        .expect("run python3");
    assert!(
        output.status.success(),
        "emitted probe test failed in {}:\n{}\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
