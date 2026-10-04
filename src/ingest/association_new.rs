//! `owner.<has_many>.new(...)` → `owner.<has_many>.build(...)`.
//!
//! On an association, `new` is CollectionProxy's alias for `build`
//! (Rails: `alias_method :new, :build`). Every consumer downstream —
//! analysis, `scope_chain`'s foreign-key seeding, the controller's
//! permitted-params and nested-create rewrites, seeds, the per-target
//! test emitters — already recognizes `build`; none recognized `new`,
//! so `@project.tasks.new(task_params)` reached the reader's Array as
//! `Array#new` and failed at runtime. Normalizing the spelling once,
//! here, before anything reads it, keeps `build` the single form those
//! tables have to know.
//!
//! Only fires when the receiver is a bare read of a name some model
//! declares `has_many` (with or without an explicit owner: a model's
//! own `comments.new` reads `self`). The same by-name rule
//! `scope_chain::AssocRegistry::is_has_many_name` uses — ingest has no
//! types to resolve the owner with. `Model.new` has a `Const`
//! receiver and a local variable is not a `Send`, so neither matches.

use std::collections::HashSet;

use crate::app::App;
use crate::dialect::{Association, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub(super) fn normalize_association_new(app: &mut App) {
    let has_many: HashSet<Symbol> = app
        .models
        .iter()
        .flat_map(|m| &m.body)
        .filter_map(|item| match item {
            ModelBodyItem::Association { assoc: Association::HasMany { name, .. }, .. } => {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    if has_many.is_empty() {
        return;
    }
    let mut f = |e: &mut Expr| rewrite(e, &has_many);
    crate::lower::for_each_hook_body(app, &mut f);
    for view in &mut app.views {
        f(&mut view.body);
    }
    for module in &mut app.test_modules {
        if let Some(setup) = &mut module.setup {
            f(setup);
        }
        for test in &mut module.tests {
            f(&mut test.body);
        }
        for helper in &mut module.helpers {
            f(&mut helper.body);
        }
    }
}

fn rewrite(expr: &mut Expr, has_many: &HashSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, has_many));
    let ExprNode::Send { recv: Some(recv), method, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "new" {
        return;
    }
    let ExprNode::Send { method: reader, args, block: None, .. } = &*recv.node else {
        return;
    };
    if args.is_empty() && has_many.contains(reader) {
        *method = Symbol::from("build");
    }
}
