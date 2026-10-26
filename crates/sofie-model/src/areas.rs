use std::path::Path;

use syn::{Expr, ImplItem, ItemImpl, Type};

use crate::syntax::{last_name, line_of, segments, string_literal, type_last_name};
use crate::{AreaClient, AreaDefinition, ModelError};

pub(crate) fn area_definition(
    implementation: &ItemImpl,
    file: &Path,
    module_path: &[String],
) -> Result<Option<AreaDefinition>, ModelError> {
    if implemented_trait(implementation).as_deref() != Some("Area") {
        return Ok(None);
    }
    let line = line_of(implementation);
    let error = |message: &str| ModelError::new(file, line, message);
    let Type::Path(self_type) = implementation.self_ty.as_ref() else {
        return Err(error("write `impl Area for TypeName` beside the type"));
    };
    let Some(type_name) = self_type.path.get_ident() else {
        return Err(error(
            "write `impl Area for TypeName` beside the type, naming it without a path",
        ));
    };
    let name = literal_constant(implementation, "NAME").ok_or_else(|| {
        error("an area needs `const NAME: &'static str = \"folder\";` with a literal name")
    })?;
    let client = match constant_value(implementation, "CLIENT") {
        None => AreaClient::Html,
        Some(Expr::Path(path)) => match last_name(&path.path).as_deref() {
            Some("Html") => AreaClient::Html,
            Some("Svelte") => AreaClient::Svelte,
            Some("React") => {
                return Err(error(
                    "`Client::React` is not supported: the framework's web toolchain has no \
                     React adapter; use `Client::Svelte`",
                ));
            }
            _ => return Err(error("`CLIENT` is `Client::Html` or `Client::Svelte`")),
        },
        Some(_) => return Err(error("write `CLIENT` as `Client::Svelte`")),
    };
    let mut type_path = module_path.to_vec();
    type_path.push(type_name.to_string());
    Ok(Some(AreaDefinition {
        name,
        type_path,
        prefix: literal_constant(implementation, "PREFIX"),
        client,
        principal: associated_type(implementation, "Principal")
            .map(segments)
            .unwrap_or_default(),
        implements_member_area: false,
        opens_sign_up: false,
        file: file.to_path_buf(),
        line,
    }))
}

pub(crate) fn implementing(implementation: &ItemImpl, trait_name: &str) -> Option<String> {
    if implemented_trait(implementation).as_deref() != Some(trait_name) {
        return None;
    }
    type_last_name(&implementation.self_ty)
}

pub(crate) fn implemented_trait(implementation: &ItemImpl) -> Option<String> {
    last_name(&implementation.trait_.as_ref()?.0)
}

fn literal_constant(implementation: &ItemImpl, name: &str) -> Option<String> {
    constant_value(implementation, name).and_then(string_literal)
}

pub(crate) fn constant_value<'file>(
    implementation: &'file ItemImpl,
    name: &str,
) -> Option<&'file Expr> {
    implementation.items.iter().find_map(|item| match item {
        ImplItem::Const(constant) if constant.ident == name => Some(&constant.expr),
        _ => None,
    })
}

pub(crate) fn associated_type<'file>(
    implementation: &'file ItemImpl,
    name: &str,
) -> Option<&'file syn::Path> {
    implementation.items.iter().find_map(|item| match item {
        ImplItem::Type(alias) if alias.ident == name => match &alias.ty {
            Type::Path(path) => Some(&path.path),
            _ => None,
        },
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::area_definition;
    use crate::AreaClient;

    fn client_of(constant: &str) -> Result<AreaClient, String> {
        let implementation: syn::ItemImpl = syn::parse_str(&format!(
            "impl Area for Member {{ const NAME: &'static str = \"member\"; {constant} }}"
        ))
        .unwrap();
        area_definition(&implementation, Path::new("area.rs"), &[])
            .map(|definition| definition.unwrap().client)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn an_area_names_its_client_or_is_html() {
        assert_eq!(client_of(""), Ok(AreaClient::Html));
        assert_eq!(
            client_of("const CLIENT: Client = Client::Svelte;"),
            Ok(AreaClient::Svelte)
        );
        let react = client_of("const CLIENT: sofie::http::Client = sofie::http::Client::React;");
        assert!(react.unwrap_err().contains("no React adapter"));
        assert!(client_of("const CLIENT: Client = Client::Vue;").is_err());
    }
}
