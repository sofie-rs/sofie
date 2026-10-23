use std::path::Path;

use syn::{Expr, Stmt};

use crate::syntax::{builder_function, line_of, single_argument, string_literal};
use crate::{MenuEntry, ModelError};

const SIGNATURE: &str = "`pub fn menu(menu: &mut Menu<Area>)`";

pub(crate) fn read_menu(file: &Path, syntax: &syn::File) -> Result<Vec<MenuEntry>, ModelError> {
    let (function, receiver) = builder_function(file, &syntax.items, "menu", SIGNATURE)?;
    function
        .block
        .stmts
        .iter()
        .map(|statement| {
            entry_of(statement, &receiver).ok_or_else(|| {
                ModelError::new(
                    file,
                    line_of(statement),
                    format!(
                        "`sofie gen` reads menus without compiling, so this body may only hold \
                         `{receiver}.entry(\"Label\", route)` statements, each optionally \
                         followed by `.feature(FLAG)`"
                    ),
                )
            })
        })
        .collect()
}

fn entry_of(statement: &Stmt, receiver: &str) -> Option<MenuEntry> {
    let Stmt::Expr(Expr::MethodCall(outermost), _) = statement else {
        return None;
    };
    let (entry, feature) = match outermost.receiver.as_ref() {
        Expr::MethodCall(entry) if outermost.method == "feature" => {
            (entry, Some(single_argument(outermost)?))
        }
        _ => (outermost, None),
    };
    let is_on_receiver = matches!(
        entry.receiver.as_ref(),
        Expr::Path(path) if path.path.is_ident(receiver)
    );
    if entry.method != "entry" || !is_on_receiver || entry.args.len() != 2 {
        return None;
    }
    Some(MenuEntry {
        label: string_literal(entry.args.first()?)?,
        feature,
        line: line_of(statement),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::read_menu;

    fn entries(source: &str) -> Result<Vec<(String, Option<String>, usize)>, String> {
        let syntax = syn::parse_file(source).unwrap();
        read_menu(Path::new("menu.rs"), &syntax)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| (entry.label, entry.feature, entry.line))
                    .collect()
            })
            .map_err(|error| error.to_string())
    }

    #[test]
    fn reads_each_entry_s_label_and_flag() {
        let source = "pub fn menu(menu: &mut Menu<Console>) {\n\
                      menu.entry(\"Orders\", routes::console::list_orders());\n\
                      menu.entry(\"Refunds\", routes::console::list_refunds()).feature(crate::module::REFUNDS);\n}";

        assert_eq!(
            entries(source).unwrap(),
            [
                ("Orders".to_owned(), None, 2),
                (
                    "Refunds".to_owned(),
                    Some("crate::module::REFUNDS".to_owned()),
                    3
                ),
            ]
        );
    }

    #[test]
    fn refuses_other_statements() {
        let source = "pub fn menu(menu: &mut Menu<Console>) { let label = \"Orders\"; }";

        let error = entries(source).unwrap_err();

        assert!(error.contains("may only hold `menu.entry"), "{error}");
    }
}
