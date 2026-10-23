use syn::{Expr, Item};

use crate::areas::{constant_value, implemented_trait};
use crate::syntax::{read_file, string_literal, type_last_name};
use crate::{AreaDefinition, ModelError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignInPaths {
    pub sign_in: String,
    pub sign_up: Option<String>,
}

impl AreaDefinition {
    /// As the area's own file declares them: sign-up by a hand-written `MemberArea`'s
    /// `SIGN_UP` or by sofie-identity's `SignUp`, which a generated `MemberArea` reads.
    pub fn sign_in_paths(&self) -> Result<Option<SignInPaths>, ModelError> {
        let Some(type_name) = self.type_path.last() else {
            return Ok(None);
        };
        let file = read_file(&self.file)?;
        let constant = |trait_name: &str, constant: &str| {
            file.items.iter().find_map(|item| match item {
                Item::Impl(implementation)
                    if implemented_trait(implementation).as_deref() == Some(trait_name)
                        && type_last_name(&implementation.self_ty).as_ref() == Some(type_name) =>
                {
                    constant_value(implementation, constant).cloned()
                }
                _ => None,
            })
        };
        let Some(sign_in) = constant("Area", "SIGN_IN")
            .as_ref()
            .and_then(string_literal)
        else {
            return Ok(None);
        };
        let sign_up = constant("MemberArea", "SIGN_UP")
            .as_ref()
            .and_then(some_literal)
            .or_else(|| constant("SignUp", "PATH").as_ref().and_then(string_literal));
        Ok(Some(SignInPaths { sign_in, sign_up }))
    }
}

fn some_literal(expression: &Expr) -> Option<String> {
    let Expr::Call(call) = expression else {
        return None;
    };
    let is_some = matches!(call.func.as_ref(), Expr::Path(path) if path.path.is_ident("Some"));
    match call.args.first() {
        Some(argument) if is_some && call.args.len() == 1 => string_literal(argument),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{TemporaryDirectory, define_area, module};

    use super::SignInPaths;

    const AREA: &str = "impl Area for Member {\n    const NAME: &'static str = \"member\";\n    \
                        const SIGN_IN: &'static str = \"/sign-in\";\n}\n";

    #[test]
    fn reads_sign_in_and_an_opened_sign_up_from_the_areas_file() {
        let directory = TemporaryDirectory::new("sofie-model-sign-in");
        let mut order = module("order", directory.path("order"));
        define_area(&mut order, "member");
        let definition = &order.defined_areas[0];
        let member = "impl MemberArea for Member {\n    const SIGN_UP: Option<&'static str> = Some(\"/join\");\n}\n";
        directory.write("order/contract/src/area.rs", &format!("{AREA}{member}"));

        let paths = definition.sign_in_paths().unwrap();

        let expected = SignInPaths {
            sign_in: "/sign-in".to_owned(),
            sign_up: Some("/join".to_owned()),
        };
        assert_eq!(paths, Some(expected));
    }

    #[test]
    fn reads_sign_up_declared_for_a_generated_member_area() {
        let directory = TemporaryDirectory::new("sofie-model-sign-up");
        let mut order = module("order", directory.path("order"));
        define_area(&mut order, "member");
        let sign_up = "impl SignUp for Member {\n    const PATH: &'static str = \"/join\";\n}\n";
        directory.write("order/contract/src/area.rs", &format!("{AREA}{sign_up}"));

        let paths = order.defined_areas[0].sign_in_paths().unwrap().unwrap();

        assert_eq!(paths.sign_up.as_deref(), Some("/join"));
    }

    #[test]
    fn a_member_area_without_sign_up_keeps_it_closed() {
        let directory = TemporaryDirectory::new("sofie-model-sign-in");
        let mut order = module("order", directory.path("order"));
        define_area(&mut order, "member");
        directory.write("order/contract/src/area.rs", AREA);

        let paths = order.defined_areas[0].sign_in_paths().unwrap().unwrap();

        assert_eq!(paths.sign_up, None);
    }
}
