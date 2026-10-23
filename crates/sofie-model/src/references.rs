use std::path::Path;

use proc_macro2::{Ident, Punct, Spacing, TokenStream, TokenTree};

use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Expr, ItemExternCrate, ItemUse, Macro, Token};

use crate::manifest::is_manifest_constant;
use crate::syntax::{line_of, read_file, segments, string_literal, use_paths};
use crate::usage::{Mentions, public_items};
use crate::{MacroCall, ModelError, PathReference, Source};

impl Source {
    pub fn read(file: &Path) -> Result<Self, ModelError> {
        Ok(Self::of_syntax(file, &read_file(file)?))
    }

    pub(crate) fn of_syntax(file: &Path, syntax: &syn::File) -> Self {
        let mut references = References::default();
        references.visit_file(syntax);
        Self {
            file: file.to_path_buf(),
            paths: references.paths,
            macros: references.macros,
            mentions: Mentions::of(syntax),
            // Tooling reads a manifest's constant; no code has to name it.
            public_items: public_items(syntax)
                .into_iter()
                .filter(|item| !is_manifest_constant(file, &item.name))
                .collect(),
        }
    }
}

#[derive(Default)]
struct References {
    paths: Vec<PathReference>,
    macros: Vec<MacroCall>,
}

impl<'ast> Visit<'ast> for References {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        // A single-segment path names a local item or binding, never a crate.
        if path.segments.len() > 1 {
            self.paths.push(PathReference {
                segments: segments(path),
                line: line_of(path),
            });
        }
        visit::visit_path(self, path);
    }

    /// A glob ends in `*`, so a rule sees that `use std::net::*` brings in the socket types.
    fn visit_item_use(&mut self, item_use: &'ast ItemUse) {
        self.paths
            .extend(use_paths(&item_use.tree).into_iter().map(|used| {
                let mut segments = used.segments;
                if used.binding.is_none() {
                    segments.push("*".to_owned());
                }
                PathReference {
                    segments,
                    line: used.line,
                }
            }));
    }

    fn visit_item_extern_crate(&mut self, item: &'ast ItemExternCrate) {
        self.paths.push(PathReference {
            segments: vec![item.ident.to_string()],
            line: line_of(&item.ident),
        });
    }

    fn visit_macro(&mut self, macro_call: &'ast Macro) {
        let arguments = macro_call
            .parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
            .ok();
        self.macros.push(MacroCall {
            path: segments(&macro_call.path),
            first_string: arguments
                .as_ref()
                .and_then(|arguments| arguments.first())
                .and_then(string_literal),
            line: line_of(&macro_call.path),
        });
        match arguments {
            Some(arguments) => arguments
                .iter()
                .for_each(|argument| self.visit_expr(argument)),
            // `info!(key = %value)`, `vec![x; n]` and DSL bodies are no comma list of expressions.
            None => self.paths.extend(token_paths(macro_call.tokens.clone())),
        }
    }
}

/// Every `a::b…` run of identifiers in the tokens, groups included.
fn token_paths(tokens: TokenStream) -> Vec<PathReference> {
    let mut paths = Vec::new();
    let mut current: Vec<Ident> = Vec::new();
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Ident(ident) => {
                push_path(&mut paths, &mut current);
                current.push(ident);
            }
            TokenTree::Punct(punct) if is_path_separator(&punct, tokens.peek()) => {
                tokens.next();
                match tokens.next() {
                    Some(TokenTree::Ident(ident)) => current.push(ident),
                    _ => push_path(&mut paths, &mut current),
                }
            }
            TokenTree::Group(group) => {
                push_path(&mut paths, &mut current);
                paths.extend(token_paths(group.stream()));
            }
            TokenTree::Punct(_) | TokenTree::Literal(_) => push_path(&mut paths, &mut current),
        }
    }
    push_path(&mut paths, &mut current);
    paths
}

fn is_path_separator(punct: &Punct, next: Option<&TokenTree>) -> bool {
    let next_is_colon = matches!(next, Some(TokenTree::Punct(next)) if next.as_char() == ':');
    punct.as_char() == ':' && punct.spacing() == Spacing::Joint && next_is_colon
}

/// A single identifier names a local item or binding, never a crate.
fn push_path(paths: &mut Vec<PathReference>, current: &mut Vec<Ident>) {
    if current.len() > 1 {
        paths.push(PathReference {
            segments: current.iter().map(Ident::to_string).collect(),
            line: line_of(&current[0]),
        });
    }
    current.clear();
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::Source;

    #[test]
    fn finds_paths_in_macro_bodies_that_are_no_comma_list() {
        let syntax: syn::File = syn::parse_str(
            "fn a() {\n\
             info!(key = %std::env::var(\"KEY\").unwrap());\n\
             let _ = vec![std::net::TcpStream::connect(address); 2];\n\
             }\n",
        )
        .unwrap();

        let source = Source::of_syntax(Path::new("jobs.rs"), &syntax);

        let paths: Vec<(String, usize)> = source
            .paths
            .iter()
            .map(|path| (path.segments.join("::"), path.line))
            .collect();
        assert_eq!(
            paths,
            [
                ("std::env::var".to_owned(), 2),
                ("std::net::TcpStream::connect".to_owned(), 3)
            ]
        );
    }

    #[test]
    fn a_glob_import_ends_in_a_star() {
        let syntax: syn::File = syn::parse_str(
            "use std::net::*;
use std::net::{self, IpAddr};
",
        )
        .unwrap();

        let source = Source::of_syntax(Path::new("lib.rs"), &syntax);

        let paths: Vec<String> = source
            .paths
            .iter()
            .map(|path| path.segments.join("::"))
            .collect();
        assert_eq!(paths, ["std::net::*", "std::net", "std::net::IpAddr"]);
    }
}
