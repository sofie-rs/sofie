use syn::Path;

use crate::read_model::Join;
use crate::shape::{NamedField, attributes_named};

pub(crate) fn join_of(field: &NamedField) -> syn::Result<Option<Join>> {
    let Some(attribute) = attributes_named(field.attrs, "read").next() else {
        return Ok(None);
    };
    let (mut via, mut joined) = (None, None);
    attribute.parse_nested_meta(|meta| {
        let target = if meta.path.is_ident("via") {
            &mut via
        } else if meta.path.is_ident("field") {
            &mut joined
        } else {
            return Err(meta.error("expected `via = Base::COLUMN` or `field = Other::COLUMN`"));
        };
        *target = Some(meta.value()?.parse::<Path>()?);
        Ok(())
    })?;
    match (via, joined) {
        (Some(via), Some(field)) => Ok(Some(Join { via, field })),
        _ => Err(syn::Error::new_spanned(
            attribute,
            "a joined field needs `#[read(via = Base::COLUMN, field = Other::COLUMN)]`",
        )),
    }
}
