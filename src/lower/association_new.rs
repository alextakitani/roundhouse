//! Normalize CollectionProxy's `new` alias after analysis has typed the owner.
//!
//! A reader with the same name on an unrelated class must keep its `new`:
//! the association name alone says nothing about that receiver.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::dialect::{Association, MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

type Associations = HashMap<ClassId, HashSet<Symbol>>;

pub fn apply_association_new_lowering(app: &mut App) {
    let assocs: Associations = app
        .models
        .iter()
        .map(|model| {
            let overrides: HashSet<Symbol> = model
                .body
                .iter()
                .filter_map(|item| match item {
                    ModelBodyItem::Method { method, .. }
                        if method.receiver == MethodReceiver::Instance =>
                    {
                        Some(method.name.clone())
                    }
                    _ => None,
                })
                .collect();
            let names = model
                .associations()
                .filter_map(|assoc| match assoc {
                    Association::HasMany { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .filter(|name| !overrides.contains(name))
                .collect();
            (model.name.clone(), names)
        })
        .collect();
    if assocs.values().all(HashSet::is_empty) {
        return;
    }

    // A bare `comments` is an association only inside its declaring model.
    // Model concerns are walked under their model's name by this helper.
    super::for_each_model_body_named(app, &mut |name, body| {
        let owner = ClassId(Symbol::from(name));
        rewrite(body, &assocs, Some(&owner));
    });
    // All other bodies can still contain an explicit, typed model owner.
    super::for_each_hook_body(app, &mut |body| rewrite(body, &assocs, None));
    for view in &mut app.views {
        rewrite(&mut view.body, &assocs, None);
    }
    super::for_each_test_body(app, &mut |body| rewrite(body, &assocs, None));
}

fn rewrite(expr: &mut Expr, assocs: &Associations, self_model: Option<&ClassId>) {
    expr.node
        .for_each_child_mut(&mut |child| rewrite(child, assocs, self_model));
    let ExprNode::Send {
        recv: Some(read),
        method,
        ..
    } = &mut *expr.node
    else {
        return;
    };
    if method.as_str() != "new" {
        return;
    }
    let ExprNode::Send {
        recv: owner,
        method: name,
        args,
        block: None,
        ..
    } = &*read.node
    else {
        return;
    };
    if !args.is_empty() {
        return;
    }
    let model = match owner {
        None => self_model,
        Some(owner) if matches!(&*owner.node, ExprNode::SelfRef) => self_model,
        Some(owner) => match owner.ty.as_ref().map(Ty::peel_nilable) {
            Some(Ty::Class { id, .. }) => Some(id),
            _ => None,
        },
    };
    if model.is_some_and(|id| assocs.get(id).is_some_and(|names| names.contains(name))) {
        *method = Symbol::from("build");
    }
}
