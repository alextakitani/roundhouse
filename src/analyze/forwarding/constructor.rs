//! Source-level `Class.new` lookup and the effective `initialize` contract.
//!
//! Constructor lowering must use Ruby's source dispatch, not the analyzer's
//! flattened parameter signature: custom class-side `new`, mixins, mutation
//! hooks and method-lookup edits can all invalidate the default forwarder.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};

use super::SourceContractIndex;

pub(crate) enum ConstructorContract<'a> {
    CustomNew(Option<&'a MethodDef>),
    UnknownLookup,
    Initialize(&'a MethodDef),
}

pub(crate) fn constructor_contracts(app: &App) -> HashMap<ClassId, ConstructorContract<'_>> {
    let contracts = SourceContractIndex::new(app);
    constructor_contracts_with_index(app, &contracts)
}

pub(super) fn constructor_contracts_with_index<'a>(
    app: &'a App,
    contracts: &SourceContractIndex<'a>,
) -> HashMap<ClassId, ConstructorContract<'a>> {
    app.library_classes
        .iter()
        .map(|class| &class.name)
        .chain(app.models.iter().map(|model| &model.name))
        .filter_map(|owner| {
            constructor_contract(contracts, owner).map(|contract| (owner.clone(), contract))
        })
        .collect()
}

fn constructor_contract<'a>(
    contracts: &SourceContractIndex<'a>,
    owner: &ClassId,
) -> Option<ConstructorContract<'a>> {
    if !contracts.verified_hierarchy(owner, &mut HashSet::new()) {
        return Some(ConstructorContract::UnknownLookup);
    }
    if inherits_unmodeled_constructor_lookup(contracts, owner, &mut HashSet::new()) {
        return Some(ConstructorContract::UnknownLookup);
    }
    if let Some((method, _)) = contracts.declaration(
        owner,
        &Symbol::from("new"),
        MethodReceiver::Class,
        &mut HashSet::new(),
    ) {
        return Some(ConstructorContract::CustomNew(Some(method)));
    }
    let (method, _) =
        contracts.effective_call(owner, &Symbol::from("new"), MethodReceiver::Class)?;
    if method.name.as_str() == "new" {
        Some(ConstructorContract::CustomNew(None))
    } else {
        Some(ConstructorContract::Initialize(method))
    }
}

fn inherits_unmodeled_constructor_lookup(
    contracts: &SourceContractIndex<'_>,
    owner: &ClassId,
    seen: &mut HashSet<ClassId>,
) -> bool {
    if !seen.insert(owner.clone()) {
        return false;
    }
    contracts.unmodeled_constructor_lookup.contains(owner)
        // `class << self; include NewMethods; end` is represented by the
        // same include edge as an ordinary instance mixin. If that module
        // defines `new`, do not assume the receiver's constructor is the
        // default `Class#new` forwarder.
        || contracts.includes(owner).iter().any(|included| {
            contracts
                .instance
                .contains_key(&(included.clone(), Symbol::from("new")))
        })
        || contracts
            .includes(owner)
            .iter()
            .any(|included| inherits_unmodeled_constructor_lookup(contracts, included, seen))
        || contracts
            .parent(owner)
            .is_some_and(|parent| inherits_unmodeled_constructor_lookup(contracts, parent, seen))
}

pub(super) fn methods_mutate_constructor_lookup<'a>(
    methods: impl Iterator<Item = &'a MethodDef>,
) -> bool {
    methods.into_iter().any(|method| {
        (method.receiver == MethodReceiver::Class
            && matches!(
                method.name.as_str(),
                "method_added"
                    | "method_removed"
                    | "method_undefined"
                    | "singleton_method_added"
                    | "singleton_method_removed"
                    | "singleton_method_undefined"
                    | "included"
                    | "prepended"
                    | "extended"
                    | "inherited"
            ))
            || (method.receiver == MethodReceiver::Class
                && is_constructor_lookup_mutation(&method.body))
            || (method.receiver == MethodReceiver::Class
                && method
                    .params
                    .iter()
                    .filter_map(|param| param.default.as_ref())
                    .any(is_constructor_lookup_mutation))
    })
}

pub(super) fn is_constructor_lookup_mutation(expr: &Expr) -> bool {
    fn mutates(expr: &Expr) -> bool {
        if let ExprNode::Send {
            recv, method, args, ..
        } = &*expr.node
            && recv
                .as_ref()
                .is_none_or(|receiver| matches!(&*receiver.node, ExprNode::SelfRef))
        {
            match method.as_str() {
                "extend"
                | "prepend"
                | "define_method"
                | "define_singleton_method"
                | "class_eval"
                | "module_eval"
                | "instance_eval"
                | "class_exec"
                | "instance_exec"
                | "using"
                | "remove_method"
                | "undef_method" => return true,
                "alias_method" | "send" | "public_send" => {
                    if args.iter().any(|arg| {
                        let method_name = match &*arg.node {
                            ExprNode::Lit {
                                value: crate::expr::Literal::Sym { value },
                            } => value.as_str(),
                            ExprNode::Lit {
                                value: crate::expr::Literal::Str { value },
                            } => value.as_str(),
                            _ => return false,
                        };
                        matches!(method_name, "new" | "initialize")
                    }) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        let mut found = false;
        expr.node
            .for_each_child(&mut |child| found |= mutates(child));
        found
    }
    mutates(expr)
}
