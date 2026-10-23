use proc_macro2::{TokenStream, TokenTree};
use syn::{Attribute, Meta};

/// Each constraint the attribute declares, named with its bound as sofie-testing's `describe`
/// names it: `required`, `max 120`, and `min 2` and `max 8` for `#[length(min = 2, max = 8)]`.
pub(crate) fn described(attribute: &Attribute, name: &str) -> Vec<String> {
    let arguments = match &attribute.meta {
        Meta::List(list) => list.tokens.clone(),
        _ => TokenStream::new(),
    };
    match name {
        "required" | "unique" => vec![name.to_owned()],
        "min" | "max" => {
            let bound: Vec<TokenTree> = arguments.into_iter().collect();
            vec![format!("{name} {}", text_of(&bound))]
        }
        "length" | "range" => split_at_commas(arguments)
            .into_iter()
            .filter_map(|bound| {
                let (key, value) = bound.split_first()?;
                let value = value.get(1..)?;
                Some(format!("{key} {}", text_of(value)))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The bound as written, its tokens joined without spaces: `-5` for `- 5`.
fn text_of(trees: &[TokenTree]) -> String {
    trees.iter().map(ToString::to_string).collect()
}

/// A comma inside a generic argument list does not end a bound.
fn split_at_commas(tokens: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut parts = Vec::new();
    let mut part = Vec::new();
    let mut angles = 0_usize;
    for tree in tokens {
        if let TokenTree::Punct(punct) = &tree {
            match punct.as_char() {
                '<' => angles += 1,
                '>' => angles = angles.saturating_sub(1),
                ',' if angles == 0 => {
                    parts.push(std::mem::take(&mut part));
                    continue;
                }
                _ => {}
            }
        }
        part.push(tree);
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::described;

    fn facts(attribute: &str, name: &str) -> Vec<String> {
        let item: syn::ItemStruct = syn::parse_str(&format!("{attribute} struct Field;")).unwrap();
        described(&item.attrs[0], name)
    }

    #[test]
    fn names_each_bound_as_written() {
        assert_eq!(facts("#[required]", "required"), ["required"]);
        assert_eq!(facts("#[max(120)]", "max"), ["max 120"]);
        assert_eq!(
            facts("#[length(min = 2, max = 8)]", "length"),
            ["min 2", "max 8"]
        );
        assert_eq!(
            facts("#[range(min = -5, max = 5)]", "range"),
            ["min -5", "max 5"]
        );
    }
}
