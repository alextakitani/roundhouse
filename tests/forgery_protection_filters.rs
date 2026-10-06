//! `protect_from_forgery` / `skip_forgery_protection` become the
//! `verify_authenticity_token` filter Rails registers. (Rails' implicit
//! default on every ActionController::Base chain is gated off; see
//! `rails_implicit_default_is_not_applied_yet`.)
//!
//! Campfire's shape is the one that matters: the macro sits in the
//! `Authentication` concern's `included do`, AFTER `require_authentication`,
//! with `unless: -> { authenticated_by.bot_key? }` — so bots posting with a
//! bot key are exempt, and only because the check runs after sign-in has
//! decided who they are. `PwaController` opts out with
//! `skip_forgery_protection`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const AUTHENTICATION: &str = r#"module Authentication
  extend ActiveSupport::Concern

  included do
    before_action :require_authentication
    protect_from_forgery with: :exception, unless: -> { authenticated_by.bot_key? }
  end

  private
    def require_authentication
      @signed_in = true
    end

    def authenticated_by
      @authenticated_by ||= "".inquiry
    end
end
"#;

fn emit(files: Vec<(&str, &str)>) -> Vec<(String, String)> {
    let mut all: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"rooms\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
    ];
    all.extend(files);
    let tree: HashMap<PathBuf, Vec<u8>> =
        all.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers(&app)
        .into_iter()
        .map(|f| (f.path.to_string_lossy().to_string(), f.content))
        .collect()
}

fn get(files: &[(String, String)], name: &str) -> String {
    files
        .iter()
        .find(|(p, _)| p.ends_with(name))
        .map(|(_, c)| c.clone())
        .unwrap_or_else(|| panic!("{name}"))
}

fn campfire_shape() -> Vec<(String, String)> {
    emit(vec![
        ("app/controllers/concerns/authentication.rb", AUTHENTICATION),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include Authentication\nend\n",
        ),
        (
            "app/controllers/rooms_controller.rb",
            "class RoomsController < ApplicationController\n  def create\n  end\nend\n",
        ),
        (
            "app/controllers/pwa_controller.rb",
            "class PwaController < ApplicationController\n  skip_forgery_protection\n\n  def manifest\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms, only: [:create]\n  get \"/manifest\" => \"pwa#manifest\"\nend\n",
        ),
    ])
}

#[test]
fn the_concern_macro_runs_after_sign_in_under_its_guard_and_halts() {
    let rooms = get(&campfire_shape(), "rooms_controller.rb");
    let auth = rooms.find("require_authentication").expect("require_authentication");
    let verify = rooms.find("verify_authenticity_token").expect("verify_authenticity_token");
    assert!(auth < verify, "the check runs after sign-in:\n{rooms}");
    // Once, at the declared position.
    assert_eq!(rooms.matches("verify_authenticity_token").count(), 1, "{rooms}");
    let tail = &rooms[verify..];
    // The guard is kept, and folded the way `deny_bots`' body is: the
    // `StringInquirer` predicate is a comparison with its label. Left as
    // `bot_key?` it is a NoMethodError on every request, which is what
    // the CRuby lane served before the inquirer fact reached controllers.
    assert!(
        tail.contains(r#"verify_authenticity_token if !(authenticated_by == "bot_key")"#),
        "the `unless:` guard is kept and folded:\n{rooms}"
    );
    assert!(tail.contains("performed?"), "a refused request halts the chain:\n{rooms}");
}

#[test]
fn skip_forgery_protection_removes_the_check() {
    let pwa = get(&campfire_shape(), "pwa_controller.rb");
    assert!(!pwa.contains("verify_authenticity_token"), "{pwa}");
}

#[test]
fn rails_implicit_default_is_not_applied() {
    // Rails puts `verify_authenticity_token` at the head of every chain
    // rooted at ActionController::Base. Still gated off: bare
    // `protect_from_forgery` is `:null_session` and must not become 422.
    // Shared Base now defines the method for apps that write
    // `with: :exception`. Pinned so turning the default on is a
    // decision with a test to update.
    let files = emit(vec![
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  before_action :load_room\n\n  private\n    def load_room\n      @room = 1\n    end\nend\n",
        ),
        (
            "app/controllers/rooms_controller.rb",
            "class RoomsController < ApplicationController\n  def create\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :rooms, only: [:create]\nend\n"),
    ]);
    let rooms = get(&files, "rooms_controller.rb");
    assert!(!rooms.contains("verify_authenticity_token"), "{rooms}");
}

#[test]
fn an_api_controller_is_not_protected() {
    let files = emit(vec![
        (
            "app/controllers/rooms_controller.rb",
            "class RoomsController < ActionController::API\n  def create\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :rooms, only: [:create]\nend\n"),
    ]);
    let rooms = get(&files, "rooms_controller.rb");
    assert!(!rooms.contains("verify_authenticity_token"), "{rooms}");
}

#[test]
fn null_session_is_not_lowered_as_the_exception_filter() {
    // Rails' bare `protect_from_forgery` is `with: :null_session`: an
    // unverified request runs with an empty session instead of failing.
    // That strategy is not modeled, so the macro must not become the
    // `:exception` filter — it stays an unrecognized class-body macro,
    // which the survey reports (lobsters writes exactly this).
    let files = emit(vec![
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  protect_from_forgery\nend\n",
        ),
        (
            "app/controllers/rooms_controller.rb",
            "class RoomsController < ApplicationController\n  def create\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :rooms, only: [:create]\nend\n"),
    ]);
    let rooms = get(&files, "rooms_controller.rb");
    assert!(!rooms.contains("verify_authenticity_token"), "{rooms}");
}
