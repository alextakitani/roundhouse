//! Project explicit keyword producer provenance only after the source
//! destination has been classified. Native full forwarders retain `**value`;
//! ordinary calls rejoin the existing keyword lowerings without a new carrier.

use crate::App;
use crate::analyze::forwarding::{
    KeywordPolicy, keyword_calls_and_constructor_contracts, keyword_refusal,
};
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode};
use std::collections::HashSet;

pub(super) fn apply(app: &mut App) -> Vec<Diagnostic> {
    let (plans, constructors) = keyword_calls_and_constructor_contracts(app);
    let constructor_splat_classes: HashSet<String> = constructors
        .into_iter()
        .filter_map(|(class, contract)| match contract {
            crate::analyze::forwarding::ConstructorContract::Initialize(method)
                if !method
                    .params
                    .iter()
                    .any(|p| p.rest || p.keyword || p.forwarding)
                    && method.params.iter().any(|p| p.from_keyword) =>
            {
                Some(class.0.as_str().to_string())
            }
            crate::analyze::forwarding::ConstructorContract::CustomNew(Some(_))
            | crate::analyze::forwarding::ConstructorContract::UnknownLookup => {
                Some(class.0.as_str().to_string())
            }
            _ => None,
        })
        .collect();
    let mut diagnostics = Vec::new();
    for (span, policy) in &plans {
        if matches!(
            policy,
            KeywordPolicy::Refuse | KeywordPolicy::RefuseOrdinarySuper
        ) {
            diagnostics.push(keyword_refusal(*span, *policy));
        }
    }
    fn project(
        e: &mut Expr,
        plans: &std::collections::HashMap<crate::span::Span, KeywordPolicy>,
        constructor_splat_classes: &HashSet<String>,
    ) {
        if plans.get(&e.span) == Some(&KeywordPolicy::Legacy) {
            let (args, constructor_splat) = match &mut *e.node {
                ExprNode::Send {
                    recv, method, args, ..
                } => {
                    let constructor_splat = method.as_str() == "new"
                        && args
                            .iter()
                            .any(|arg| matches!(&*arg.node, ExprNode::KeywordSplat { .. }))
                        && recv.as_ref().is_some_and(|recv| {
                            matches!(&*recv.node, ExprNode::Const { .. })
                                && (matches!(
                                    &e.diagnostic,
                                    Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. })
                                        if construct.as_str() == crate::diagnostic::CONSTRUCTOR_KEYWORD_ARGUMENTS
                                ) || matches!(
                                    &recv.ty,
                                    Some(crate::ty::Ty::Class { id, .. })
                                        if constructor_splat_classes.contains(id.0.as_str())
                                ))
                        });
                    (Some(args), constructor_splat)
                }
                ExprNode::Super { args: Some(args) } => (Some(args), false),
                _ => (None, false),
            };
            if let Some(args) = args.filter(|_| !constructor_splat) {
                for arg in args {
                    if let ExprNode::KeywordSplat { value } = &mut *arg.node {
                        *arg = std::mem::replace(value, super::typing::nil_lit());
                    }
                }
            }
        }
        e.node
            .for_each_child_mut(&mut |c| project(c, plans, constructor_splat_classes));
    }
    super::for_each_forwarding_body(app, &mut |e| project(e, &plans, &constructor_splat_classes));
    diagnostics
}
