use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, ExprMethodCall, FnArg, Pat, Signature, Type};

use super::{Edit, offset};

const MOVED: [&str; 2] = ["content_security", "content_security_all"];

/// `app.content_security::<A>(sources)` becomes
/// `app.serving(|serving| serving.content_security::<A>(sources))`. Only a
/// receiver known to be an `App` moves: the same calls on a `Serving` are the
/// new form.
pub(super) fn edits(text: &str, file: &syn::File) -> Vec<Edit> {
    let mut functions = AppFunctions(Vec::new());
    functions.visit_file(file);
    let mut finder = Finder {
        text,
        app_functions: functions.0,
        app_bindings: vec![Vec::new()],
        edits: Vec::new(),
    };
    finder.visit_file(file);
    finder.edits
}

/// The file's functions returning an `App`, as `app()` in `app().content_security(…)`.
struct AppFunctions(Vec<String>);

impl<'ast> Visit<'ast> for AppFunctions {
    fn visit_signature(&mut self, signature: &'ast Signature) {
        if let syn::ReturnType::Type(_, returned) = &signature.output
            && names_app(returned)
        {
            self.0.push(signature.ident.to_string());
        }
    }
}

struct Finder<'a> {
    text: &'a str,
    app_functions: Vec<String>,
    /// Each name bound and whether to an `App`, one scope per function or closure; the
    /// innermost binding of a name shadows the rest.
    app_bindings: Vec<Vec<(String, bool)>>,
    edits: Vec<Edit>,
}

impl<'ast> Visit<'ast> for Finder<'_> {
    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.app_bindings.push(parameter_names(&function.sig));
        visit::visit_item_fn(self, function);
        self.app_bindings.pop();
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        self.app_bindings.push(parameter_names(&function.sig));
        visit::visit_impl_item_fn(self, function);
        self.app_bindings.pop();
    }

    fn visit_expr_closure(&mut self, closure: &'ast syn::ExprClosure) {
        let parameters = closure.inputs.iter().map(|input| match input {
            Pat::Type(typed) => (&*typed.pat, Some(&*typed.ty)),
            untyped => (untyped, None),
        });
        self.app_bindings.push(bindings(parameters));
        visit::visit_expr_closure(self, closure);
        self.app_bindings.pop();
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        visit::visit_local(self, local);
        let (pattern, annotated) = match &local.pat {
            Pat::Type(typed) => (&*typed.pat, names_app(&typed.ty)),
            pattern => (pattern, false),
        };
        let bound = match pattern {
            Pat::Ident(binding) => {
                let initialized = local
                    .init
                    .as_ref()
                    .is_some_and(|init| self.is_app(&init.expr));
                vec![(binding.ident.to_string(), annotated || initialized)]
            }
            other => destructured_names(other),
        };
        let scope = self.app_bindings.last_mut().expect("a scope is open");
        scope.retain(|(name, _)| bound.iter().all(|(shadowing, _)| shadowing != name));
        scope.extend(bound);
    }

    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        // `?` and `.await` would fail inside the closure the arguments move into.
        if MOVED.iter().any(|moved| call.method == moved)
            && self.is_app(&call.receiver)
            && !tries_or_awaits(call)
        {
            self.edits.push(rewrite(self.text, call));
            // The edit spans the arguments, so one inside them would overlap it.
            self.visit_expr(&call.receiver);
            return;
        }
        visit::visit_expr_method_call(self, call);
    }
}

impl Finder<'_> {
    /// `App`'s builder methods return the `App`, so a chain is one when its start is.
    fn is_app(&self, expression: &Expr) -> bool {
        match expression {
            Expr::MethodCall(call) => self.is_app(&call.receiver),
            Expr::Paren(inner) => self.is_app(&inner.expr),
            Expr::Try(inner) => self.is_app(&inner.expr),
            Expr::Await(inner) => self.is_app(&inner.base),
            Expr::Call(call) => self.calls_app_function(&call.func),
            Expr::Path(path) => path.path.get_ident().is_some_and(|name| {
                self.app_bindings
                    .iter()
                    .rev()
                    .find_map(|scope| scope.iter().rev().find(|(bound, _)| name == bound))
                    .is_some_and(|(_, app)| *app)
            }),
            _ => false,
        }
    }

    fn calls_app_function(&self, function: &Expr) -> bool {
        let Expr::Path(path) = function else {
            return false;
        };
        let segments: Vec<String> = path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        match segments.as_slice() {
            [name] => self.app_functions.contains(name),
            [.., owner, _] => owner == "App",
            [] => false,
        }
    }
}

fn parameter_names(signature: &Signature) -> Vec<(String, bool)> {
    bindings(signature.inputs.iter().filter_map(|input| match input {
        FnArg::Typed(typed) => Some((&*typed.pat, Some(&*typed.ty))),
        FnArg::Receiver(_) => None,
    }))
}

fn bindings<'a>(
    parameters: impl Iterator<Item = (&'a Pat, Option<&'a Type>)>,
) -> Vec<(String, bool)> {
    parameters
        .flat_map(|(pattern, ty)| match pattern {
            Pat::Ident(binding) => vec![(binding.ident.to_string(), ty.is_some_and(names_app))],
            other => destructured_names(other),
        })
        .collect()
}

/// Every name a destructuring pattern binds, none of them known to be an `App`.
fn destructured_names(pattern: &Pat) -> Vec<(String, bool)> {
    struct Names(Vec<(String, bool)>);
    impl<'ast> Visit<'ast> for Names {
        fn visit_pat_ident(&mut self, binding: &'ast syn::PatIdent) {
            self.0.push((binding.ident.to_string(), false));
            visit::visit_pat_ident(self, binding);
        }
    }
    let mut names = Names(Vec::new());
    names.visit_pat(pattern);
    names.0
}

fn tries_or_awaits(call: &ExprMethodCall) -> bool {
    struct Found(bool);
    impl<'ast> Visit<'ast> for Found {
        fn visit_expr_try(&mut self, _: &'ast syn::ExprTry) {
            self.0 = true;
        }
        fn visit_expr_await(&mut self, _: &'ast syn::ExprAwait) {
            self.0 = true;
        }
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
        fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}
    }
    let mut found = Found(false);
    for argument in &call.args {
        found.visit_expr(argument);
    }
    found.0
}

/// `App`, or a type wrapping one, as `Result<App>`.
fn names_app(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(last) = path.path.segments.last() else {
        return false;
    };
    if last.ident == "App" {
        return true;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &last.arguments else {
        return false;
    };
    arguments.args.iter().any(|argument| match argument {
        syn::GenericArgument::Type(ty) => names_app(ty),
        _ => false,
    })
}

/// Replaces the method, its turbofish and its arguments, leaving the receiver.
fn rewrite(text: &str, call: &ExprMethodCall) -> Edit {
    let start = call.method.span().start();
    let end = call.span().end();
    let called = &text[offset(text, start)..offset(text, end)];
    Edit {
        start,
        end,
        text: format!("serving(|serving| serving.{called})"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::upgrade_text;

    #[test]
    fn moves_both_calls_onto_serving_and_keeps_the_rest_of_the_chain() {
        let text = "fn app(db: Db) -> App {\n    App::new(db)\n        .content_security::<Embeds>(provider())\n        \
                    .content_security_all(ContentSecurity::default())\n        .area(\"web\", routes)\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(
            upgraded,
            "fn app(db: Db) -> App {\n    App::new(db)\n        \
             .serving(|serving| serving.content_security::<Embeds>(provider()))\n        \
             .serving(|serving| serving.content_security_all(ContentSecurity::default()))\n        \
             .area(\"web\", routes)\n}\n"
        );
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn a_serving_helper_is_already_the_new_form() {
        let text = "fn sources(serving: Serving) -> Serving {\n    serving.content_security_all(everywhere())\n}\n\n\
                    fn web(serving: Serving) -> Serving {\n    sources(serving).content_security::<Web>(web())\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(upgraded, text);
        assert_eq!(changes, []);
    }

    #[test]
    fn moves_the_calls_of_an_app_bound_or_built_by_a_function() {
        let text = "fn app() -> App {\n    App::new(db())\n}\n\n\
                    async fn wire() -> Result<App> {\n    let built = app();\n    \
                    Ok(built.content_security_all(sources()))\n}\n\n\
                    fn more() -> App {\n    app().content_security_all(sources())\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(changes.len(), 2);
        assert!(
            upgraded
                .contains("Ok(built.serving(|serving| serving.content_security_all(sources())))")
        );
        assert!(
            upgraded.contains("app().serving(|serving| serving.content_security_all(sources()))")
        );
    }

    #[test]
    fn a_comment_before_the_call_stays_before_it() {
        let text = "fn app(app: App) -> App {\n    app // the embeds\n        .content_security_all(sources())\n}\n";

        let (upgraded, _) = upgrade_text(text).unwrap();

        assert_eq!(
            upgraded,
            "fn app(app: App) -> App {\n    app // the embeds\n        \
             .serving(|serving| serving.content_security_all(sources()))\n}\n"
        );
    }

    #[test]
    fn an_untyped_closure_parameter_shadows_an_outer_app() {
        let text = "fn app(app: App) -> App {\n    app.serving(|app| app.content_security_all(sources()))\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(upgraded, text);
        assert_eq!(changes, []);
    }

    #[test]
    fn a_tuple_or_reference_binding_shadows_an_outer_app() {
        let text = "fn app(app: App) -> App {\n    \
                    pairs.map(|(app, _)| app.content_security_all(a()));\n    \
                    refs.map(|&app| app.content_security_all(b()));\n    \
                    let (app, _) = pair;\n    app.content_security_all(c())\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(upgraded, text);
        assert_eq!(changes, []);
    }

    #[test]
    fn a_call_whose_arguments_try_or_await_is_left_for_the_compiler() {
        let text = "async fn app(app: App) -> Result<App> {\n    \
                    Ok(app.content_security_all(sources()?).content_security::<Web>(web().await))\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(upgraded, text);
        assert_eq!(changes, []);
    }

    #[test]
    fn a_call_inside_another_s_arguments_moves_with_it() {
        let text =
            "fn app(app: App) -> App {\n    app.content_security(other.content_security(x))\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(
            upgraded,
            "fn app(app: App) -> App {\n    \
             app.serving(|serving| serving.content_security(other.content_security(x)))\n}\n"
        );
        assert_eq!(changes.len(), 1);
    }
}
