//! Read from syntax, so a value whose type only inference knows is recorded without one, never guessed.

use std::collections::BTreeMap;

use syn::visit::{self, Visit};
use syn::{Block, Expr, ExprCall, ExprMethodCall, FnArg, Macro, Pat, Signature};

use crate::code::TypeName;
use crate::handlers::HandlerScope;
use crate::syntax::{last_name, line_of, macro_expressions, segments};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub calls: Vec<Call>,
    pub method_calls: Vec<MethodCall>,
    pub raised: Vec<Raised>,
    pub queued: Vec<Queued>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub path: Vec<String>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodCall {
    pub receiver: Option<String>,
    pub method: String,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raised {
    pub event: Option<String>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    pub job: Option<Vec<String>>,
    pub line: usize,
}

impl Effects {
    pub(crate) fn of(
        signature: &Signature,
        body: &Block,
        owner: Option<&str>,
        scope: &HandlerScope,
    ) -> Self {
        let mut visitor = EffectVisitor {
            typed: typed_parameters(signature, owner),
            scope,
            owner,
            found: Self::default(),
        };
        visitor.visit_block(body);
        visitor.found
    }
}

fn typed_parameters(signature: &Signature, owner: Option<&str>) -> BTreeMap<String, String> {
    let mut typed: BTreeMap<String, String> = signature
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Typed(typed) => match typed.pat.as_ref() {
                Pat::Ident(name) => {
                    Some((name.ident.to_string(), referenced(TypeName::of(&typed.ty))))
                }
                _ => None,
            },
            FnArg::Receiver(_) => None,
        })
        .collect();
    if let Some(owner) = owner {
        typed.insert("self".to_owned(), owner.to_owned());
    }
    typed
}

fn referenced(mut ty: TypeName) -> String {
    while ty.name == "&" && ty.arguments.len() == 1 {
        ty = ty.arguments.remove(0);
    }
    ty.name
}

struct EffectVisitor<'s> {
    typed: BTreeMap<String, String>,
    scope: &'s HandlerScope,
    owner: Option<&'s str>,
    found: Effects,
}

impl EffectVisitor<'_> {
    fn callee(&self, path: &syn::Path) -> Vec<String> {
        let written = segments(path);
        match (written.first().map(String::as_str), self.owner) {
            (Some("Self"), Some(owner)) => [&[owner.to_owned()], &written[1..]].concat(),
            _ => self.scope.callee(&written),
        }
    }

    fn receiver_type(&self, receiver: &Expr) -> Option<String> {
        let Expr::Path(path) = receiver else {
            return None;
        };
        let name = path.path.get_ident()?.to_string();
        self.typed.get(&name).cloned()
    }
}

impl Visit<'_> for EffectVisitor<'_> {
    fn visit_expr_call(&mut self, call: &ExprCall) {
        if let Expr::Path(function) = call.func.as_ref() {
            self.found.calls.push(Call {
                path: self.callee(&function.path),
                line: line_of(call),
            });
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &ExprMethodCall) {
        let line = line_of(&call.method);
        let first = call.args.first();
        match call.method.to_string().as_str() {
            "raise" => self.found.raised.push(Raised {
                event: first.and_then(event_name),
                line,
            }),
            "queue" => self.found.queued.push(Queued {
                job: first.and_then(|job| match job {
                    Expr::Path(path) => Some(self.callee(&path.path)),
                    _ => None,
                }),
                line,
            }),
            method => self.found.method_calls.push(MethodCall {
                receiver: self.receiver_type(&call.receiver),
                method: method.to_owned(),
                line,
            }),
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, call: &Macro) {
        for expression in macro_expressions(call) {
            self.visit_expr(&expression);
        }
    }
}

fn event_name(argument: &Expr) -> Option<String> {
    match argument {
        Expr::Struct(literal) => last_name(&literal.path),
        Expr::Path(path) => last_name(&path.path),
        _ => None,
    }
}
