//! An unrelated `comments` reader must retain its own `.new` call.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_library_reader_named_like_an_association_keeps_new() {
    emit_and_run::real_blog()
        .write(
            "app/lib/inbox.rb",
            "class Note\n  attr_reader :text\n  def initialize(text)\n    @text = text\n  end\nend\n\nclass Inbox\n  def comments\n    Note\n  end\n\n  def draft(text)\n    comments.new(text)\n  end\n\n  def explicit_draft(text)\n    self.comments.new(text)\n  end\nend\n",
        )
        .run_ruby("raise 'wrong note' unless Inbox.new.draft('hi').text == 'hi'\nraise 'wrong explicit note' unless Inbox.new.explicit_draft('hi').text == 'hi'\nraise 'wrong external note' unless Inbox.new.comments.new('hi').text == 'hi'")
        .assert_passes();
}

#[test]
fn model_methods_named_like_an_association_keep_new() {
    emit_and_run::real_blog()
        .write(
            "app/lib/note.rb",
            "class Note\n  attr_reader :text\n  def initialize(text)\n    @text = text\n  end\nend\n",
        )
        .write(
            "app/models/memo.rb",
            "class Memo < Article\n  has_many :comments\n\n  def comments\n    Note\n  end\n\n  def self.comments\n    Note\n  end\n\n  def draft(text)\n    comments.new(text)\n  end\n\n  def class_draft(text)\n    self.class.comments.new(text)\n  end\n\n  def self.bare_class_draft(text)\n    comments.new(text)\n  end\nend\n",
        )
        .write(
            "app/models/class_memo.rb",
            "class ClassMemo < Article\n  has_many :comments\n\n  def self.comments\n    Note\n  end\n\n  def self.draft(text)\n    comments.new(text)\n  end\nend\n",
        )
        .run_ruby("raise 'wrong note' unless Memo.new.draft('hi').text == 'hi'\nraise 'wrong class note' unless Memo.new.class_draft('hi').text == 'hi'\nraise 'wrong bare class note' unless Memo.bare_class_draft('hi').text == 'hi'\nraise 'wrong class memo' unless ClassMemo.draft('hi').text == 'hi'")
        .assert_passes();
}
