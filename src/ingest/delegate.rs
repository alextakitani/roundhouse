//! ActiveSupport's `delegate :a, :b, to: :target` → real methods.
//!
//! Rails defines these with `class_eval` at load time, so nothing about
//! them survives into an emitted tree: the declaration lands in
//! `unknown_calls` and every call to a delegated name is a bare send no
//! class defines. The failure is SILENT wherever the caller rescues —
//! campfire's `message_presentation` wraps its whole body in `rescue
//! Exception` and returns `""`, so a missing `fragment` rendered every
//! message with an EMPTY body and no error anywhere. The search page
//! looked like it had no results; it had one, drawn blank.
//!
//! [`super::current_attributes`] already expands the declaration for
//! `ActiveSupport::CurrentAttributes` subclasses, where the target is a
//! declared attribute and the forwarder can read its ivar directly.
//! This is the general case: the target is a METHOD (campfire's
//! `attr_reader :content` beside the declaration), so the forwarder
//! CALLS it, which is also what Rails' generated body does.
//!
//! ## Where it declines
//!
//! A delegated method that takes ARGUMENTS. Rails forwards them with
//! `*args, &block`; argument forwarding is the shape the strict targets
//! do not lower ([[project_kwarg_forwarding_strict_targets_gap]]), and
//! a zero-arg forwarder for a method that takes two is an arity error
//! standing in for a NameError — a different wrong answer, not a fix.
//! The test is the declaring class's own call sites: campfire's
//! `Messages::AttachmentPresentation` delegates `:tag`, `:link_to` and
//! four more `to: :context` and calls every one of them WITH arguments
//! in the same file, so that declaration stays in `unknown_calls`,
//! visible, exactly as it is today.
//!
//! The limitation that leaves: a declaration in a BASE class whose only
//! argument-passing callers are subclasses reads as zero-arg here.
//! campfire has no such case — `ActionText::Content::Filter`'s
//! `fragment` is a reader on both sides — and closing it properly means
//! the arity coming from the TARGET's own signature rather than from
//! call sites.

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, MethodVisibility};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// One expanded `delegate` entry: `<name>` forwards to `<target>.<method>`.
#[derive(Debug)]
struct Delegation {
    method: Symbol,
    target: Symbol,
    name: String,
    /// The real app file the `delegate` call was declared in, when the
    /// source registry can still resolve it (it can, at this point in
    /// the pipeline — see `ingest::sources`'s module doc). Empty when
    /// it can't; callers fall back to the pass's own `"<delegate>"`
    /// label for a synthesis-failure report rather than misattribute.
    file: String,
    visibility: MethodVisibility,
}

#[derive(Default)]
struct CallShapes {
    positional_arities: std::collections::HashSet<usize>,
    has_block: bool,
}

type CallsWithArguments = std::collections::HashMap<String, CallShapes>;

/// Expand every `delegate … to: …` the app's library classes declare.
///
/// Runs AFTER `lower_current_attributes`, which consumes the
/// declarations on its own classes — so what reaches here is the
/// general shape only.
pub fn lower_delegates(app: &mut crate::App) {
    let mut generated: Vec<(usize, Vec<MethodDef>)> = Vec::new();
    for (i, lc) in app.library_classes.iter_mut().enumerate() {
        // A concern's module body is evaluated once, before its eventual
        // includer is known. Model association delegates are therefore
        // expanded only after an `included do` declaration has been
        // spliced into each concrete model.
        if lc.is_module {
            continue;
        }
        let methods = expand_delegates_in_class(lc);
        if !methods.is_empty() {
            generated.push((i, methods));
        }
    }
    for (i, methods) in generated {
        app.library_classes[i].methods.extend(methods);
    }
}

pub(super) fn is_delegate_declaration(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "delegate")
}

/// `synthesized_source` produced Ruby Prism couldn't parse cleanly —
/// roundhouse's gap, not the app's. Record one survey entry naming
/// every delegated name in this class's declaration, attributed to the
/// real file when a delegated call's own span still resolves one.
fn record_synthesis_failure(delegates: &[Delegation], diags: &[crate::diagnostic::Diagnostic]) {
    let names = delegates
        .iter()
        .map(|d| d.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let file = delegates
        .iter()
        .map(|d| d.file.as_str())
        .find(|f| !f.is_empty())
        .unwrap_or("<delegate>")
        .to_string();
    super::survey::record_synthesis_failure(
        file,
        &format!("delegate forwarder for `{names}`"),
        diags,
    );
}

/// The per-class body of `lower_delegates`, factored out so a caller
/// with a single `LibraryClass` in hand — rather than a whole `App` to
/// loop over — can drive the same expansion. `lower_controller_to_
/// library_class` is exactly that caller: a controller isn't a
/// `LibraryClass` at the point `lower_delegates` runs over
/// `app.library_classes` (ingest time; controllers only become one
/// per-target at emit time — see `lower::controller_to_library`), so a
/// `delegate` call in a controller body never reached this file at
/// all until the controller lowering started collecting it into its
/// own `unknown_calls` and calling this directly.
///
/// Returns the synthesized forwarder methods; does NOT append them to
/// `lc.methods` itself (`lower_delegates` above batches that across the
/// whole app; a single-class caller can just extend its own `methods`
/// with the result).
pub(crate) fn expand_delegates_in_class(lc: &mut LibraryClass) -> Vec<MethodDef> {
    expand_delegates(
        &lc.name,
        &lc.methods,
        &mut lc.unknown_calls,
        &[],
        &std::collections::HashSet::new(),
        None,
        &[],
    )
}

pub(super) fn expand_delegates(
    name: &crate::ident::ClassId,
    methods: &[MethodDef],
    unknown_calls: &mut Vec<Expr>,
    additional_method_bodies: &[Expr],
    blocked_names: &std::collections::HashSet<String>,
    supported_target_methods: Option<&std::collections::HashSet<(String, String, usize)>>,
    sources: &[crate::span::SourceFile],
) -> Vec<MethodDef> {
    let called_with_args = names_called_with_arguments(methods, additional_method_bodies);
    let delegates = take_delegate_decls_from_calls(
        unknown_calls,
        &called_with_args,
        blocked_names,
        supported_target_methods,
        sources,
    );
    if delegates.is_empty() {
        return Vec::new();
    }
    let src = synthesized_source(name, methods, &delegates);

    // Isolated in its OWN scope — never the outer one that spans
    // the whole app's ingest — so a bug in `synthesized_source`
    // can't render its parse errors against an unrelated real
    // file (see `ingest::sources`'s module doc for how that
    // happened before this existed).
    let (parsed, diags) = crate::ingest::prism::scope(|| {
        crate::ingest::ingest_library_classes(src.as_bytes(), "<delegate>")
    });
    match parsed {
        Ok(classes) if diags.is_empty() => classes
            .into_iter()
            .flat_map(|class| class.methods)
            .map(|mut method| {
                if let Some(delegate) = delegates
                    .iter()
                    .find(|delegate| delegate.name == method.name.as_str())
                {
                    method.visibility = delegate.visibility;
                }
                method
            })
            .collect(),
        Ok(_) => {
            record_synthesis_failure(&delegates, &diags);
            Vec::new()
        }
        Err(err) => {
            super::survey::record(&err);
            Vec::new()
        }
    }
}

/// Consume the declarations this pass can reproduce EXACTLY, leaving
/// every other shape in `unknown_calls` rather than half-expanded.
fn take_delegate_decls_from_calls(
    unknown_calls: &mut Vec<Expr>,
    called_with_args: &CallsWithArguments,
    blocked_names: &std::collections::HashSet<String>,
    supported_target_methods: Option<&std::collections::HashSet<(String, String, usize)>>,
    sources: &[crate::span::SourceFile],
) -> Vec<Delegation> {
    let mut out: Vec<Delegation> = Vec::new();
    unknown_calls.retain(|call| {
        let ExprNode::Send {
            recv: None,
            method,
            args,
            ..
        } = &*call.node
        else {
            return true;
        };
        if call.diagnostic.is_some() {
            return true;
        }
        if method.as_str() != "delegate" {
            return true;
        }
        let mut names: Vec<Symbol> = Vec::new();
        let (mut to, mut prefix) = (None, None);
        let mut unknown_option = false;
        for a in args {
            match &*a.node {
                ExprNode::Lit {
                    value: Literal::Sym { value },
                } => names.push(value.clone()),
                ExprNode::Hash { entries, .. } => {
                    for (k, v) in entries {
                        let ExprNode::Lit {
                            value: Literal::Sym { value: key },
                        } = &*k.node
                        else {
                            unknown_option = true;
                            continue;
                        };
                        match key.as_str() {
                            "to" => {
                                if let ExprNode::Lit {
                                    value: Literal::Sym { value },
                                } = &*v.node
                                {
                                    to = Some(value.clone());
                                } else {
                                    unknown_option = true;
                                }
                            }
                            "prefix" => match &*v.node {
                                ExprNode::Lit {
                                    value: Literal::Bool { value: true },
                                } => {
                                    prefix = Some(None);
                                }
                                ExprNode::Lit {
                                    value: Literal::Bool { value: false },
                                } => {
                                    prefix = None;
                                }
                                ExprNode::Lit {
                                    value: Literal::Sym { value },
                                } => {
                                    prefix = Some(Some(value.as_str().to_string()));
                                }
                                ExprNode::Lit {
                                    value: Literal::Str { value },
                                } => {
                                    prefix = Some(Some(value.clone()));
                                }
                                _ => unknown_option = true,
                            },
                            // Returning nil for a missing target is only
                            // correct when nil does not implement the delegated
                            // method. This pass cannot currently model that
                            // runtime `respond_to?` check on every target.
                            "allow_nil" => match &*v.node {
                                ExprNode::Lit {
                                    value: Literal::Bool { value: false },
                                } => {}
                                _ => unknown_option = true,
                            },
                            // `private:` and the
                            // rest are shapes this does not reproduce.
                            _ => unknown_option = true,
                        }
                    }
                }
                _ => unknown_option = true,
            }
        }
        let Some(target) = to else { return true };
        if names.is_empty() || !valid_delegate_target(target.as_str()) {
            return true;
        }
        if names
            .iter()
            .any(|name| !valid_delegate_method(name.as_str()))
        {
            let file = super::sources::path_of(call.span.file).unwrap_or_default();
            let names = names
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            super::survey::record(&crate::ingest::IngestError::Unsupported {
                file,
                message: format!(
                    "delegate forwarder for `{names}` could not be synthesized: invalid method name"
                ),
            });
            return true;
        }
        let generated_names: Vec<_> = names
            .iter()
            .map(|method| {
                prefix
                    .as_ref()
                    .map(|explicit| explicit.as_deref().unwrap_or(target.as_str()))
                    .map_or_else(
                        || method.as_str().to_string(),
                        |prefix| format!("{prefix}_{}", method.as_str()),
                    )
            })
            .collect();
        if prefix.is_some()
            && generated_names
                .iter()
                .any(|name| !valid_prefixed_method_name(name))
        {
            return true;
        }
        // Even if an option is unsupported, Rails' later declaration still
        // replaces an earlier method with the same generated name.
        out.retain(|existing| !generated_names.contains(&existing.name));
        if unknown_option {
            return true;
        }
        // Arguments at a call site mean the forwarder needs to forward
        // them — see the module header. A setter is exempt: Ruby's own
        // assignment syntax fixes its arity at exactly one, so
        // `self.behavior = v` calling `behavior=` WITH an argument is
        // not evidence this pass can't cover it — it's what every
        // setter call looks like, delegated or not.
        let file = super::sources::path_of(call.span.file).unwrap_or_default();
        let visibility = super::sources::with_text(&file, |source| {
            super::visibility::Visibility::declaration_default(
                source,
                &file,
                call.span.start as usize,
            )
        })
        .or_else(|| {
            call.span
                .file
                .0
                .checked_sub(1)
                .and_then(|index| sources.get(index as usize))
                .map(|source| {
                    super::visibility::Visibility::declaration_default(
                        &source.text,
                        &source.path,
                        call.span.start as usize,
                    )
                })
        })
        .unwrap_or_default();
        let mut entries = Vec::new();
        for m in names {
            let prefix = prefix
                .as_ref()
                .map(|explicit| explicit.as_deref().unwrap_or(target.as_str()));
            let name = prefix.map_or_else(
                || m.as_str().to_string(),
                |prefix| format!("{prefix}_{}", m.as_str()),
            );
            if blocked_names.contains(&name) {
                unknown_option = true;
                break;
            }
            // Argument-bearing calls to the generated name need forwarding,
            // which this pass does not model. Compare after applying prefix:
            // `title(arg)` is unrelated to `parent_title` generated by
            // `delegate :title, prefix: :parent`.
            if !m.as_str().ends_with('=') {
                if let Some(calls) = called_with_args.get(&name) {
                    let forwardable_operator = operator_forwarder("", m.as_str())
                        .map(|(params, _)| params.split(", ").count())
                        .is_some_and(|arity| {
                            !calls.has_block
                                && calls
                                    .positional_arities
                                    .iter()
                                    .all(|call_arity| *call_arity == arity)
                        });
                    if !forwardable_operator {
                        unknown_option = true;
                        break;
                    }
                }
            }
            entries.push(Delegation {
                method: m,
                target: target.clone(),
                name,
                file: file.clone(),
                visibility,
            });
        }
        if unknown_option {
            return true;
        }
        if supported_target_methods.is_some_and(|supported| {
            entries.iter().any(|entry| {
                !supported.contains(&(
                    entry.target.as_str().to_string(),
                    entry.method.as_str().to_string(),
                    synthesized_arity(entry.method.as_str()),
                ))
            })
        }) {
            return true;
        }
        out.extend(entries);
        false
    });
    out
}

/// Bare names called WITH arguments in instance method bodies. Class-body
/// DSL expressions are not instance call sites and must not suppress a
/// forwarder. A delegated name in this set needs argument forwarding this
/// pass declines to synthesize.
fn names_called_with_arguments(
    methods: &[MethodDef],
    additional_method_bodies: &[Expr],
) -> CallsWithArguments {
    let mut out = CallsWithArguments::new();
    for m in methods {
        collect_calls_with_args(&m.body, &mut out);
    }
    for expr in additional_method_bodies {
        collect_calls_with_args(expr, &mut out);
    }
    out
}

fn valid_prefixed_method_name(name: &str) -> bool {
    let base = name
        .strip_suffix('?')
        .or_else(|| name.strip_suffix('!'))
        .or_else(|| name.strip_suffix('='))
        .unwrap_or(name);
    let mut chars = base.chars();
    matches!(chars.next(), Some('_' | 'a'..='z' | 'A'..='Z'))
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn valid_delegate_target(target: &str) -> bool {
    let name = target.strip_prefix('@').unwrap_or(target);
    valid_identifier(name)
}

fn valid_delegate_method(name: &str) -> bool {
    matches!(name, "!" | "~" | "+@" | "-@")
        || operator_forwarder("", name).is_some()
        || valid_identifier(
            name.strip_suffix('?')
                .or_else(|| name.strip_suffix('!'))
                .or_else(|| name.strip_suffix('='))
                .unwrap_or(name),
        )
}

fn synthesized_arity(method: &str) -> usize {
    if let Some((params, _)) = operator_forwarder("", method) {
        return params.split(", ").count();
    }
    usize::from(method.ends_with('='))
}

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('_' | 'a'..='z' | 'A'..='Z'))
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn collect_calls_with_args(expr: &Expr, out: &mut CallsWithArguments) {
    expr.node
        .for_each_child(&mut |c| collect_calls_with_args(c, out));
    let ExprNode::Send {
        recv,
        method,
        args,
        block,
        ..
    } = &*expr.node
    else {
        return;
    };
    let instance_call = recv
        .as_ref()
        .is_none_or(|recv| matches!(&*recv.node, ExprNode::SelfRef));
    if !instance_call {
        return;
    }
    if !args.is_empty() || block.is_some() {
        let calls = out.entry(method.as_str().to_string()).or_default();
        calls.positional_arities.insert(args.len());
        calls.has_block |= block.is_some();
    }
}

/// The forwarders, as Ruby source — parsed back through ingest so the
/// generated bodies are ordinary IR, indistinguishable from a method
/// the app wrote. Same construction `current_attributes` uses.
/// The receiver Rails writes for `to:`: `self.<to>` when the name is a
/// Ruby keyword (`to: :class`, a model's `return` association) or one of
/// the names its generated method uses itself, the name otherwise
/// (`DELEGATION_RESERVED_METHOD_NAMES` in active_support/delegation.rb).
fn receiver(target: &str) -> std::borrow::Cow<'_, str> {
    const RESERVED: &[&str] = &[
        "__ENCODING__",
        "__LINE__",
        "__FILE__",
        "alias",
        "and",
        "BEGIN",
        "begin",
        "break",
        "case",
        "class",
        "def",
        "defined?",
        "do",
        "else",
        "elsif",
        "END",
        "end",
        "ensure",
        "false",
        "for",
        "if",
        "in",
        "module",
        "next",
        "nil",
        "not",
        "or",
        "redo",
        "rescue",
        "retry",
        "return",
        "self",
        "super",
        "then",
        "true",
        "undef",
        "unless",
        "until",
        "when",
        "while",
        "yield",
        "_",
        "arg",
        "args",
        "block",
        "value",
        "key",
        "other",
        "__delegate_target",
    ];
    if RESERVED.contains(&target) {
        format!("self.{target}").into()
    } else {
        target.into()
    }
}

/// The forwarder for a fixed-one-argument binary operator, or `None` for
/// an ordinary name. Indexing operators are declined because Ruby methods
/// `[]` and `[]=` may take variable positional arities that this expander
/// cannot forward safely.
fn operator_forwarder(t: &str, m: &str) -> Option<(&'static str, String)> {
    const BINARY: &[&str] = &[
        "==", "!=", "<", ">", "<=", ">=", "<=>", "===", "=~", "!~", "+", "-", "*", "/", "%", "**",
        "<<", ">>", "&", "|", "^",
    ];
    match m {
        op if BINARY.contains(&op) => Some(("other", format!("{t} {op} other"))),
        _ => None,
    }
}

fn synthesized_source(
    name: &crate::ident::ClassId,
    methods: &[MethodDef],
    delegates: &[Delegation],
) -> String {
    let defines = |name: &str| {
        methods
            .iter()
            .any(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == name)
    };
    let mut body = String::new();
    let mut generated_names = std::collections::HashSet::new();
    for d in delegates.iter().rev() {
        if defines(&d.name) {
            continue;
        }
        if !generated_names.insert(d.name.as_str()) {
            continue;
        }
        let (target, m) = (receiver(d.target.as_str()), d.method.as_str());
        let t = target.as_ref();
        // A setter takes the one argument Ruby's own assignment syntax
        // supplies (with `prefix: true`, `d.name` is already
        // `<prefix>_name=`); an operator takes its fixed operands.
        let (params, call) = if let Some(forwarder) = operator_forwarder(t, m) {
            forwarder
        } else if let Some(attr) = m.strip_suffix('=') {
            ("value", format!("{t}.{attr} = value"))
        } else {
            ("", format!("{t}.{m}"))
        };
        let signature = if params.is_empty() {
            d.name.clone()
        } else {
            format!("{}({params})", d.name)
        };
        body.push_str(&format!("  def {signature}\n    {call}\n  end\n\n"));
    }
    // The class name is irrelevant — only the METHODS are lifted out of
    // the parse — but a wrapper is needed for the bodies to be methods.
    format!("class {}\n{body}end\n", name.0.as_str().replace("::", "__"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::AccessorKind;
    use crate::effect::EffectSet;
    use crate::span::Span;

    fn library_class(src: &str) -> LibraryClass {
        crate::ingest::ingest_library_class(src.as_bytes(), "app/lib/deprecation.rb")
            .unwrap()
            .unwrap()
    }

    /// `synthesized_source`'s output, re-parsed the same way
    /// `lower_delegates` re-parses it — the same round trip that
    /// catches Bug A (a setter `def` with no parameter and a bare
    /// `x.y=` call is two syntax errors, not zero).
    fn synthesized_methods(lc: &LibraryClass, delegates: &[Delegation]) -> Vec<MethodDef> {
        let src = synthesized_source(&lc.name, &lc.methods, delegates);
        crate::ingest::ingest_library_classes(src.as_bytes(), "<test>")
            .unwrap_or_else(|e| panic!("synthesized source failed to parse: {e}\n{src}"))
            .into_iter()
            .flat_map(|c| c.methods)
            .collect()
    }

    fn take_delegate_decls(lc: &mut LibraryClass) -> Vec<Delegation> {
        let called_with_args = names_called_with_arguments(&lc.methods, &[]);
        take_delegate_decls_from_calls(
            &mut lc.unknown_calls,
            &called_with_args,
            &std::collections::HashSet::new(),
            None,
            &[],
        )
    }

    #[test]
    fn module_scope_delegates_stay_unexpanded_without_a_known_includer() {
        let mut app = crate::App::default();
        let mut concern =
            library_class("class ProfileAccess\n  delegate :email, to: :profile\nend\n");
        concern.is_module = true;
        app.library_classes.push(concern);

        lower_delegates(&mut app);

        let concern = &app.library_classes[0];
        assert!(
            concern
                .methods
                .iter()
                .all(|method| method.name.as_str() != "email")
        );
        assert_eq!(concern.unknown_calls.len(), 1);
        assert!(is_delegate_declaration(&concern.unknown_calls[0]));
    }

    #[test]
    fn a_delegated_getter_and_setter_pair_both_synthesize() {
        let mut lc = library_class(
            "class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior, :behavior=, to: :deprecator\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(delegates.len(), 2);
        assert!(
            lc.unknown_calls.is_empty(),
            "the declaration should be consumed"
        );

        let methods = synthesized_methods(&lc, &delegates);
        let getter = methods
            .iter()
            .find(|m| m.name.as_str() == "behavior")
            .expect("getter should be synthesized");
        assert!(
            getter.params.is_empty(),
            "a getter forwarder takes no arguments"
        );

        let setter = methods
            .iter()
            .find(|m| m.name.as_str() == "behavior=")
            .expect("setter should be synthesized");
        assert_eq!(
            setter.params.len(),
            1,
            "a setter forwarder takes exactly the one argument Ruby's own assignment syntax supplies"
        );
    }

    #[test]
    fn prefix_true_composes_with_the_setter_name_too() {
        let mut lc = library_class(
            "class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior=, to: :deprecator, prefix: true\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(delegates.len(), 1);
        assert_eq!(
            delegates[0].name, "deprecator_behavior=",
            "prefix composes ahead of the delegated name, trailing `=` intact"
        );

        let methods = synthesized_methods(&lc, &delegates);
        let setter = methods
            .iter()
            .find(|m| m.name.as_str() == "deprecator_behavior=")
            .expect("prefixed setter should be synthesized under its prefixed name");
        assert_eq!(setter.params.len(), 1);
    }

    #[test]
    fn the_last_delegate_for_a_name_wins() {
        let mut lc = library_class(
            "class Deprecation\n  delegate :title, to: :first\n  delegate :title, to: :second\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(delegates.len(), 1);
        let source = synthesized_source(&lc.name, &lc.methods, &delegates);
        assert!(
            source.contains("second.title"),
            "later declaration should win:\n{source}"
        );
        assert!(
            !source.contains("first.title"),
            "earlier duplicate should be discarded:\n{source}"
        );
    }

    #[test]
    fn an_invalid_explicit_prefix_is_left_unexpanded() {
        let mut lc = library_class(
            "class Deprecation\n  delegate :title, to: :article, prefix: \"bad;raise\"\nend\n",
        );
        assert!(take_delegate_decls(&mut lc).is_empty());
        assert_eq!(
            lc.unknown_calls.len(),
            1,
            "the declaration must remain visible to later diagnostics"
        );
    }

    #[test]
    fn a_prefixed_delegate_is_checked_against_its_generated_name() {
        let mut lc = library_class(
            "class Filter\n  delegate :title, to: :article, prefix: :parent\n  def render\n    title(\"caption\")\n  end\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(
            delegates.len(),
            1,
            "the unrelated unprefixed call must not suppress `parent_title`"
        );
        assert_eq!(delegates[0].name, "parent_title");
    }

    #[test]
    fn a_self_call_with_arguments_declines_a_zero_argument_delegate() {
        let mut lc = library_class(
            "class Filter\n  delegate :title, to: :article\n  def render\n    self.title(\"caption\")\n  end\nend\n",
        );
        assert!(take_delegate_decls(&mut lc).is_empty());
        assert_eq!(
            lc.unknown_calls.len(),
            1,
            "the unsupported forwarding stays visible"
        );
    }

    #[test]
    fn a_concern_method_call_with_arguments_declines_model_delegate_expansion() {
        let mut model = library_class("class Post\n  delegate :title, to: :leaf\nend\n");
        let concern = library_class(
            "class Rendering\n  def render_title\n    title(\"caption\")\n  end\nend\n",
        );
        let concern_bodies: Vec<_> = concern
            .methods
            .iter()
            .map(|method| method.body.clone())
            .collect();
        let called_with_args = names_called_with_arguments(&model.methods, &concern_bodies);
        let delegates = take_delegate_decls_from_calls(
            &mut model.unknown_calls,
            &called_with_args,
            &std::collections::HashSet::new(),
            None,
            &[],
        );
        assert!(
            delegates.is_empty(),
            "the concern's call needs argument forwarding"
        );
        assert_eq!(
            model.unknown_calls.len(),
            1,
            "the declaration remains unexpanded"
        );
    }

    #[test]
    fn a_model_accessor_collision_is_left_unexpanded() {
        let mut model = library_class("class Post\n  delegate :title, to: :leaf\nend\n");
        let called_with_args = names_called_with_arguments(&model.methods, &[]);
        let delegates = take_delegate_decls_from_calls(
            &mut model.unknown_calls,
            &called_with_args,
            &["title".to_string()].into_iter().collect(),
            None,
            &[],
        );
        assert!(
            delegates.is_empty(),
            "a conflicting generated accessor must not be silently replaced"
        );
        assert_eq!(
            model.unknown_calls.len(),
            1,
            "the declaration remains unexpanded"
        );
    }

    #[test]
    fn allow_nil_true_is_left_unexpanded() {
        let mut lc = library_class(
            "class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior=, to: :deprecator, allow_nil: true\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert!(delegates.is_empty());
        assert_eq!(
            lc.unknown_calls.len(),
            1,
            "the declaration must remain visible"
        );
    }

    #[test]
    fn ruby_source_fragments_in_delegate_symbols_are_left_unexpanded() {
        for source in [
            "class Probe\n  delegate :\"bad; end; def injected\", to: :target\nend\n",
            "class Probe\n  delegate :title, to: :\"target; raise\"\nend\n",
        ] {
            let mut lc = library_class(source);
            assert!(take_delegate_decls(&mut lc).is_empty(), "{source}");
            assert_eq!(
                lc.unknown_calls.len(),
                1,
                "unsafe declaration was consumed: {source}"
            );
        }
    }

    #[test]
    fn variable_arity_bracket_assign_is_left_unexpanded() {
        // `[]=` takes a key and value, but Ruby also allows multiple keys;
        // this pass cannot reproduce its variadic forwarding semantics.
        let mut lc = library_class(
            "class Store\n  attr_accessor :backing\n\n  delegate :[]=, to: :backing\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert!(delegates.is_empty());
        assert_eq!(lc.unknown_calls.len(), 1);
    }

    /// Ordinary Ruby can't actually produce a `Send { recv: None,
    /// method: "behavior=", .. }` node — `self.behavior = v` always
    /// carries an explicit receiver; a bare `behavior = v` is *always*
    /// local-variable assignment in Ruby, never a method send,
    /// delegated or not. So `names_called_with_arguments` (which only
    /// collects receiverless sends) can never actually hold a
    /// `"name="` key from real source today, and this guard's setter
    /// exemption is not reachable through the parser as things stand.
    /// It is still the right rule to state — the arity a `def name=`
    /// forwarder needs is fixed by Ruby's own assignment grammar, not
    /// by anything a call site can be observed doing — and cheap
    /// insurance against a future IR change (or a metaprogrammed call
    /// site this ingest doesn't model yet) making such a node
    /// possible. Pinned directly against hand-built IR since the
    /// parser has no way to hand us this shape.
    #[test]
    fn a_setter_name_is_exempt_from_the_called_with_arguments_guard() {
        let mut lc = library_class(
            "class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior=, to: :deprecator\nend\n",
        );
        lc.methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name: Symbol::from("reset!"),
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            block_param: None,
            name_span: Span::synthetic(),
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("behavior="),
                    args: vec![Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit {
                            value: Literal::Sym {
                                value: Symbol::from("warn"),
                            },
                        },
                    )],
                    block: None,
                    parenthesized: false,
                },
            ),
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        });

        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(
            delegates.len(),
            1,
            "a setter's fixed arity exempts it from the call-site-arguments heuristic"
        );
    }
}
